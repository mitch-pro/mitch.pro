//! Background email + housekeeping workers (plan Step 13, batch 1).
//! Ports the four email timers registered at server.js:5061-5064 plus the
//! three housekeeping intervals:
//! - `weeklyDigestWorker` 600s   (server.js:4853-4907)
//! - `dailyPuzzleWorker` 600s    (server.js:4913-4947)
//! - `clockWarnWorker` 600s      (server.js:4948-4982)
//! - `dmDigestWorker` 1800s      (server.js:4984-5058)
//! - DM prune 60s                (server.js:26352-26364)
//! - `cleanExpiredE2eAttachments` 300s + one immediate call (server.js:25227+)
//! - rlLog sweep 300s            (server.js:3477-3483)
//!
//! The four email timers are plain `setInterval(fn, ms)` in the JS — the
//! first run lands one period in — so each email task here consumes
//! `interval`'s immediate first tick before its loop. The housekeeping
//! intervals that JS pairs with an immediate call run once right away,
//! exactly like the JS.

use crate::state::AppState;
use chrono::{Datelike, Timelike};
use mitch_lib::jsval;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;

/// Starts every Step 13 batch-1 worker task. Call once from `workers::spawn`.
pub fn spawn(state: Arc<AppState>) {
    // Email workers — setInterval semantics: first run one period in.
    type EmailWorker = (u64, fn(&Arc<AppState>));
    let email_workers: Vec<EmailWorker> = vec![
        (600, weekly_digest_worker),
        (600, daily_puzzle_worker),
        (600, clock_warn_worker),
        (1800, dm_digest_worker),
    ];
    for (secs, run) in email_workers {
        let state = state.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(secs));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick.tick().await; // setInterval's first fire is one period in
            loop {
                tick.tick().await;
                run(&state);
            }
        });
    }

    // DM prune — every 60s (server.js:26352-26364).
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                dm_prune_tick(&state);
            }
        });
    }

    // cleanExpiredE2eAttachments — every 300s plus one immediate call at
    // startup (server.js:25227-25233).
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(300));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await; // first tick is immediate — the JS startup call
                crate::routes::dm::clean_expired(&state);
            }
        });
    }

    // rlLog sweep — every 300s (server.js:3477-3483).
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(300));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                state.rate_limiter.sweep_log();
            }
        });
    }
}

// ── Shared helpers (server.js:4811-4852) ─────────────────────────────────────

/// `loadEmailLog()` / `saveEmailLog()` (server.js:4811-4813) —
/// `data/email_log.json` through the shared store.
fn email_log_path(state: &AppState) -> std::path::PathBuf {
    state.data_dir().join("email_log.json")
}

fn load_email_log(state: &AppState) -> Value {
    state.store.read_document(&email_log_path(state), json!({}))
}

fn save_email_log(state: &AppState, log: &Value) {
    let p = email_log_path(state);
    let _ = state.store.write_document(&p, log);
    let _ = std::fs::write(&p, mitch_lib::data::js_stringify_pretty(log));
}

/// Helper to check if a user's stored userdata preferences indicate they unsubscribed.
pub(crate) fn check_userdata_unsubscribed(ud: &Value) -> bool {
    if !ud.is_object() {
        return false;
    }
    // Check top-level boolean preferences
    if ud.get("newsletter") == Some(&json!(false))
        || ud.get("puzzle") == Some(&json!(false))
        || ud.get("dailyPuzzle") == Some(&json!(false))
        || ud.get("chessPuzzle") == Some(&json!(false))
        || ud.get("unsubscribed") == Some(&json!(true))
    {
        return true;
    }
    // Check _snapshot._prefPrivacy and _prefPrivacy
    let pref_raw = ud
        .get("_snapshot")
        .and_then(|s| s.get("_prefPrivacy"))
        .or_else(|| ud.get("_prefPrivacy"));
    if let Some(val) = pref_raw {
        let pref_obj: Option<Value> = match val {
            Value::String(s) => serde_json::from_str(s).ok(),
            Value::Object(_) => Some(val.clone()),
            _ => None,
        };
        if let Some(p) = pref_obj {
            if p.get("newsletter") == Some(&json!(false))
                || p.get("puzzle") == Some(&json!(false))
                || p.get("dailyPuzzle") == Some(&json!(false))
                || p.get("chessPuzzle") == Some(&json!(false))
                || p.get("unsubscribed") == Some(&json!(true))
            {
                return true;
            }
        }
    }
    // Check preferences object
    if let Some(p) = ud.get("preferences").and_then(|p| p.as_object()) {
        if p.get("newsletter") == Some(&json!(false))
            || p.get("puzzle") == Some(&json!(false))
            || p.get("dailyPuzzle") == Some(&json!(false))
            || p.get("chessPuzzle") == Some(&json!(false))
            || p.get("unsubscribed") == Some(&json!(true))
        {
            return true;
        }
    }
    // Check _prefNotification / notification preferences
    let notif_raw = ud
        .get("_snapshot")
        .and_then(|s| s.get("_prefNotification"))
        .or_else(|| ud.get("_prefNotification"));
    if let Some(val) = notif_raw {
        let notif_obj: Option<Value> = match val {
            Value::String(s) => serde_json::from_str(s).ok(),
            Value::Object(_) => Some(val.clone()),
            _ => None,
        };
        if let Some(n) = notif_obj {
            if n.get("newsletter") == Some(&json!(false))
                || n.get("puzzle") == Some(&json!(false))
            {
                return true;
            }
        }
    }
    false
}

/// Loads the set of unsubscribed email addresses from both the data store, disk,
/// and pending unsubscribe requests.
/// Contains both raw lowercased addresses and normalized forms so that
/// variants (e.g. dotted vs non-dotted or domain aliases) match properly.
pub(crate) fn newsletter_unsub_set(state: &AppState) -> HashSet<String> {
    let mut set = HashSet::new();

    let mut add_from_val = |val: Value| {
        if let Some(arr) = val.as_array() {
            for v in arr {
                if let Some(s) = v.as_str() {
                    let low = s.to_lowercase().trim().to_string();
                    if !low.is_empty() {
                        let norm = mitch_lib::auth::normalize_email(&low);
                        set.insert(low);
                        if !norm.is_empty() {
                            set.insert(norm);
                        }
                    }
                }
            }
        } else if let Some(obj) = val.as_object() {
            for (k, v) in obj {
                if k == "emails" || k == "unsub" || k == "list" {
                    if let Some(arr) = v.as_array() {
                        for item in arr {
                            if let Some(s) = item.as_str() {
                                let low = s.to_lowercase().trim().to_string();
                                if !low.is_empty() {
                                    set.insert(low.clone());
                                    let norm = mitch_lib::auth::normalize_email(&low);
                                    if !norm.is_empty() {
                                        set.insert(norm);
                                    }
                                }
                            }
                        }
                    }
                } else if k.contains('@') {
                    let low = k.to_lowercase().trim().to_string();
                    set.insert(low.clone());
                    let norm = mitch_lib::auth::normalize_email(&low);
                    if !norm.is_empty() {
                        set.insert(norm);
                    }
                }
            }
        }
    };

    // 1. Read newsletter_unsub.json from store and disk (data_dir and base_dir/data)
    let p1 = state.data_dir().join("newsletter_unsub.json");
    let doc1 = state.store.read_document(&p1, json!([]));
    add_from_val(doc1);
    if let Ok(raw) = std::fs::read_to_string(&p1) {
        if let Ok(val) = serde_json::from_str::<Value>(&raw) {
            add_from_val(val);
        }
    }

    let p2 = state.cfg.base_dir.join("data/newsletter_unsub.json");
    if p2 != p1 {
        let doc2 = state.store.read_document(&p2, json!([]));
        add_from_val(doc2);
        if let Ok(raw) = std::fs::read_to_string(&p2) {
            if let Ok(val) = serde_json::from_str::<Value>(&raw) {
                add_from_val(val);
            }
        }
    }

    // 2. Read unsub_requests.json (pending / submitted unsub requests)
    let reqs_file = state.data_dir().join("unsub_requests.json");
    let reqs = state.store.read_document(&reqs_file, json!([]));
    if let Some(arr) = reqs.as_array() {
        for item in arr {
            if let Some(em) = item.get("email").and_then(|v| v.as_str()) {
                let low = em.to_lowercase().trim().to_string();
                if !low.is_empty() {
                    set.insert(low.clone());
                    let norm = mitch_lib::auth::normalize_email(&low);
                    if !norm.is_empty() {
                        set.insert(norm);
                    }
                }
            }
        }
    }

    set
}

/// Checks whether an email address (or its delivery / canonical counterpart)
/// is in the unsubscribed set or has opt-out preferences configured in userdata.
pub(crate) fn is_unsubscribed(state: &AppState, unsub_set: &HashSet<String>, email: &str) -> bool {
    let low = email.to_lowercase().trim().to_string();
    if low.is_empty() {
        return false;
    }
    if unsub_set.contains(&low) {
        return true;
    }
    let norm = mitch_lib::auth::normalize_email(&low);
    if !norm.is_empty() && unsub_set.contains(&norm) {
        return true;
    }
    let target = canonical_email(state, &low);
    if !target.is_empty() {
        let target_low = target.to_lowercase().trim().to_string();
        if unsub_set.contains(&target_low) {
            return true;
        }
        let target_norm = mitch_lib::auth::normalize_email(&target_low);
        if !target_norm.is_empty() && unsub_set.contains(&target_norm) {
            return true;
        }
    }

    // Check userdata preferences for opt-outs
    let ud = userdata_for_email(state, &low);
    if check_userdata_unsubscribed(&ud) {
        return true;
    }
    if !target.is_empty() && target != low {
        let ud_target = userdata_for_email(state, &target);
        if check_userdata_unsubscribed(&ud_target) {
            return true;
        }
    }

    false
}

/// `enrolledUsers()` (server.js:4815-4830) — tokens.json entries with an
/// email, deduped, minus newsletter unsubscribes and keeping only records
/// with `claimed_domains` or `used`.
fn enrolled_users(state: &AppState) -> Vec<String> {
    let tokens = state
        .store
        .read_document(&state.data_dir().join("tokens.json"), json!({}));
    let unsub = newsletter_unsub_set(state);
    let mut seen: HashSet<String> = HashSet::new();
    let mut users = Vec::new();
    if let Some(map) = tokens.as_object() {
        for data in map.values() {
            let email = data
                .get("email")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase()
                .trim()
                .to_string();
            if email.is_empty()
                || seen.contains(&email)
                || is_unsubscribed(state, &unsub, &email)
                || data.get("unsubscribed") == Some(&json!(true))
                || data.get("unsub") == Some(&json!(true))
                || data.get("newsletter") == Some(&json!(false))
            {
                continue;
            }
            let target_email = canonical_email(state, &email);
            if !target_email.is_empty() && is_unsubscribed(state, &unsub, &target_email) {
                continue;
            }
            if !truthy_of(data.get("claimed_domains")) && !truthy_of(data.get("used")) {
                continue;
            }
            seen.insert(email.clone());
            users.push(email);
        }
    }
    users
}

/// `userdataForEmail(email)` (server.js:4832-4844) — names table lookup
/// (first uid whose name matches case-insensitively) → that user's
/// `/opt/userdata/<sha256(uid)[..32]>/data.json`. Missing file or bad JSON
/// → `{}` (the JS catch).
fn userdata_for_email(state: &AppState, email: &str) -> Value {
    let Some(uid) = mitch_lib::auth::names_find_sid_by_email_ci(&state.store, email) else {
        return json!({});
    };
    let Some(fpath) = crate::routes::members::userdata_path(&uid) else {
        return json!({});
    };
    if !fpath.exists() {
        return json!({});
    }
    std::fs::read_to_string(&fpath)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| json!({}))
}

/// `emailSlot(email, slots)` (server.js:4846-4849) — `Math.imul` hash over
/// UTF-16 code units with `>>> 0` after each step (`wrapping_*` is bit-equal).
fn email_slot(email: &str, slots: u32) -> u32 {
    let mut h: u32 = 0;
    for unit in email.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(u32::from(unit));
    }
    h % slots
}

/// `siteUrl(email)` (server.js:1684-1687) — lowercased email ending in
/// `@student.rjuhsd.us` picks `alternate`, else `primary`. `site()`
/// (server.js:1675-1677) defaults the whole doc when the file is empty; an
/// existing doc missing a key renders `undefined` in the template, like JS.
pub(crate) fn site_url(state: &AppState, email: &str) -> String {
    let site = state
        .store
        .read_document(&state.data_dir().join("site.json"), Value::Null);
    let get = |key: &str, default: &str| -> String {
        match &site {
            // site.json empty/missing → site()'s default object.
            Value::Null => default.to_string(),
            v => match v.get(key) {
                // Missing key → template literal sees `undefined`.
                None => "undefined".to_string(),
                Some(val) => jsval::string(val),
            },
        }
    };
    let (primary, alternate) = (
        get("primary", "https://mitch.pro"),
        get("alternate", "https://mitch.88chan.me"),
    );
    if email.to_lowercase().ends_with("@student.rjuhsd.us") {
        alternate
    } else {
        primary
    }
}

/// `canonicalDeliveryEmail(recip)` wrapper with the state handles it needs.
fn canonical_email(state: &AppState, raw: &str) -> String {
    mitch_lib::profile::canonical_delivery_email(
        &state.store,
        state.data_dir(),
        &state.id_secret,
        raw,
    )
}

/// `truthy` for an optional key read — a missing key is `undefined` → falsy.
fn truthy_of(v: Option<&Value>) -> bool {
    v.map(jsval::truthy).unwrap_or(false)
}

/// `${n}` inside a template literal — the JS String(n) rendering. NaN/∞
/// cannot ride a JSON number, so they are rendered here instead.
fn fmt_js_num(n: f64) -> String {
    if n.is_nan() {
        return "NaN".to_string();
    }
    if n.is_infinite() {
        return if n > 0.0 {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        };
    }
    jsval::string(&jsval::num_value(n))
}

/// `log[key] = { ...ulog, field: value }` (insertion-ordered spread; NaN
/// values are dropped rather than written as null — the Step 5 lesson).
fn set_log_field(log: &mut Value, key: &str, field: &str, value: Value) {
    let Some(obj) = log.as_object_mut() else {
        return;
    };
    let mut entry = obj.get(key).cloned().unwrap_or_else(|| json!({}));
    if let Some(e) = entry.as_object_mut() {
        if value.is_null() {
            // A NaN numeric was rendered as null upstream; JS assignment of
            // NaN keeps the key in memory but JSON.stringify drops it.
            e.shift_remove(field);
        } else {
            e.insert(field.to_string(), value);
        }
    }
    obj.insert(key.to_string(), entry);
}

/// `log[key]` entry clone (`log[key] || {}`).
fn log_entry(log: &Value, key: &str) -> Value {
    log.get(key).cloned().unwrap_or_else(|| json!({}))
}

/// `Date.parse(entry.timestamp || '')` — ISO-8601 with an offset, or no zone
/// meaning local time, like JS. `None` = NaN.
fn js_date_parse(v: Option<&Value>) -> Option<f64> {
    let s = jsval::str_or(v, "");
    if s.is_empty() {
        return None;
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&s) {
        return Some(dt.timestamp_millis() as f64);
    }
    chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S%.f")
        .ok()
        .and_then(|nd| nd.and_local_timezone(chrono::Local).single())
        .map(|dt| dt.timestamp_millis() as f64)
}

/// `new URL(pg, 'http://x').pathname` for the shapes session pages take:
/// absolute or root-relative paths, query/hash stripped. Unparseable input
/// keeps the pre-URL value in the JS (the try/catch), modeled by `None`.
fn url_pathname(pg: &str) -> Option<String> {
    let cut = |s: &str| -> String {
        match s.find(['?', '#']) {
            Some(i) => s[..i].to_string(),
            None => s.to_string(),
        }
    };
    if let Some(rest) = pg.strip_prefix('/') {
        if let Some(rest) = rest.strip_prefix('/') {
            // Protocol-relative "//host/path" — the authority is dropped from
            // the pathname, like `new URL('//host/path', 'http://x')`.
            let path = match rest.find('/') {
                Some(i) => cut(&rest[i..]),
                None => "/".to_string(),
            };
            return Some(path);
        }
        // Root-relative: the '/' stripped above comes back.
        let path = cut(rest);
        return Some(if path.is_empty() {
            "/".to_string()
        } else {
            format!("/{path}")
        });
    }
    if let Some(scheme_end) = pg.find("://") {
        let rest = &pg[scheme_end + 3..];
        let path_start = rest.find('/').map(|i| scheme_end + 3 + i);
        let path = match path_start {
            Some(i) => cut(&pg[i..]),
            None => "/".to_string(),
        };
        return Some(path);
    }
    // Relative reference resolved against http://x → '/' + pg.
    let path = cut(pg);
    Some(if path.is_empty() {
        "/".to_string()
    } else {
        format!("/{path}")
    })
}

// ── weeklyDigestWorker (server.js:4853-4907) ─────────────────────────────────

fn weekly_digest_worker(state: &Arc<AppState>) {
    match weekly_digest_impl(state) {
        Ok(()) => {}
        Err(e) => tracing::info!("[weekly] error: {e}"),
    }
}

// `!(cookies > 0.0)` is deliberate — it is `!cookies` in the JS, and NaN
// (a junk truthy cookie string) must fail the gate exactly like JS does.
#[allow(clippy::neg_cmp_op_on_partial_ord)]
fn weekly_digest_impl(state: &Arc<AppState>) -> Result<(), String> {
    let now = chrono::Local::now();
    // Monday only (getDay() === 1).
    if now.weekday().num_days_from_sunday() != chrono::Weekday::Mon.num_days_from_sunday() {
        return Ok(());
    }
    let current_slot = now.minute() / 10;
    // `${now.getFullYear()}-W${String(Math.ceil(now.getDate() / 7)).padStart(2,'0')}`
    let week_key = format!(
        "{}-W{:02}",
        now.year(),
        ((now.day() as f64) / 7.0).ceil() as u32
    );
    let mut log = load_email_log(state);
    let logs = state
        .store
        .read_document(&state.data_dir().join("sessions.json"), json!([]));
    let week_ago = (mitch_lib::school::now_millis() - 7 * 86_400_000) as f64;

    // gamesByEmail — ping session-log entries carry no `page`, so today this
    // stays empty and the digest fires for chess-ELO / cookie users only.
    // Ported faithfully for the day the log grows a page field.
    let mut games_by_email: std::collections::HashMap<String, indexmap::IndexMap<String, f64>> =
        std::collections::HashMap::new();
    if let Some(entries) = logs.as_array() {
        for entry in entries {
            let Some(ts) = js_date_parse(entry.get("timestamp")) else {
                continue;
            };
            if ts < week_ago {
                continue;
            }
            let mut pg = jsval::str_or(entry.get("page"), "");
            for dom in ["https://mitch.pro", "https://mitch.dnswish.com"] {
                if pg.starts_with(dom) {
                    pg = pg[dom.len()..].to_string();
                }
            }
            if !pg.contains("/games/") {
                continue;
            }
            let uid = jsval::str_or(entry.get("id"), "");
            let Some(email) = mitch_lib::auth::names_get_email(&state.store, &uid) else {
                continue;
            };
            let email = email.to_lowercase();
            if email.is_empty() {
                continue;
            }
            // `pg = new URL(pg, 'http://x').pathname` inside try/catch.
            let Some(pg) = url_pathname(&pg) else {
                continue;
            };
            let slot = games_by_email.entry(email).or_default();
            *slot.entry(pg).or_insert(0.0) += 1.0;
        }
    }

    let unsub = newsletter_unsub_set(state);
    for email in enrolled_users(state) {
        if is_unsubscribed(state, &unsub, &email) {
            continue;
        }
        if email_slot(&email, 6) != current_slot {
            continue;
        }
        let ulog = log_entry(&log, &email);
        if ulog.get("weekly_digest").and_then(|v| v.as_str()) == Some(week_key.as_str()) {
            continue;
        }
        let target_email = canonical_email(state, &email);
        if is_unsubscribed(state, &unsub, &target_email) {
            continue;
        }
        let ud = userdata_for_email(state, &email);
        // `ud.chess_elo || null`, `ud.CookieClickerGame || ud['cookie-clicker'] || null`.
        let elo_data = jsval::or(ud.get("chess_elo"), Value::Null);
        let cc_raw = jsval::or(
            ud.get("CookieClickerGame"),
            jsval::or(ud.get("cookie-clicker"), Value::Null),
        );
        // `ccRaw ? (ccRaw.cookies || ccRaw.cookieCount || 0) : 0`.
        let cookies = if jsval::truthy(&cc_raw) {
            jsval::number(&jsval::or(
                cc_raw.get("cookies"),
                jsval::or(cc_raw.get("cookieCount"), json!(0)),
            ))
            .unwrap_or(f64::NAN)
        } else {
            0.0
        };
        let visits = games_by_email.get(&email);
        let total_visits = visits.map(|m| m.values().sum::<f64>()).unwrap_or(0.0);
        // `if (!totalVisits && !eloData && !cookies)`.
        if total_visits == 0.0 && !jsval::truthy(&elo_data) && !(cookies > 0.0) {
            continue;
        }

        // `Object.entries(visits).sort((a, b) => b[1] - a[1])[0]` — stable
        // descending by count, insertion order preserved for ties.
        let top_game = visits.and_then(|m| {
            let mut pairs: Vec<(&String, &f64)> = m.iter().collect();
            pairs.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));
            pairs.first().map(|(k, _)| (*k).clone())
        });

        // The JS builds a dead `body` text variable before the template send —
        // it is never used, so it is not ported.
        let target_email = canonical_email(state, &email);
        let html = make_weekly_digest_html(
            state,
            &target_email,
            total_visits,
            top_game.as_ref(),
            &elo_data,
            cookies,
        );
        crate::routes::push::send_email_bg(
            state,
            &target_email,
            "Your mitch.pro week in review",
            &html,
        );
        set_log_field(&mut log, &email, "weekly_digest", json!(week_key.clone()));
        tracing::info!("[weekly] sent to {target_email}");
    }
    // Unconditional save (server.js:4905) — even on a no-op pass.
    save_email_log(state, &log);
    Ok(())
}

/// `makeWeeklyDigestHtml` (server.js:1780-1833).
fn make_weekly_digest_html(
    state: &AppState,
    email: &str,
    total_visits: f64,
    top_game: Option<&String>,
    elo_data: &Value,
    cookies: f64,
) -> String {
    let dashboard_url = site_url(state, email);
    let mut stats_html = String::new();

    if total_visits != 0.0 {
        // `topGame ? topGame[0].split('/').filter(Boolean).pop() : ''`.
        let game_name = top_game
            .map(|t| {
                t.rsplit('/')
                    .find(|seg| !seg.is_empty())
                    .unwrap_or("")
                    .to_string()
            })
            .unwrap_or_default();
        let plural = if total_visits != 1.0 { "s" } else { "" };
        let most = if game_name.is_empty() {
            String::new()
        } else {
            format!("<br><span style=\"font-size: 12px;\">Most played: <em>{game_name}</em></span>")
        };
        stats_html += &format!(
            "\n      <div style=\"background-color: rgba(56, 189, 248, 0.05); border: 1px solid rgba(56, 189, 248, 0.15); border-radius: 12px; padding: 16px; margin-bottom: 16px;\">\n        <span style=\"font-size: 18px; margin-right: 8px;\">🎮</span>\n        <strong style=\"color: #38bdf8;\">Games Played</strong>\n        <p style=\"margin: 6px 0 0 28px; font-size: 14px; color: #94a3b8;\">\n          <strong>{}</strong> session{} this week.\n          {}\n        </p>\n      </div>\n    ",
            fmt_js_num(total_visits),
            plural,
            most,
        );
    }

    if jsval::truthy(elo_data) && truthy_of(elo_data.get("elo")) {
        let elo = jsval::string_of(elo_data.get("elo"));
        let wins = jsval::str_or(elo_data.get("wins"), "0");
        let losses = jsval::str_or(elo_data.get("losses"), "0");
        stats_html += &format!(
            "\n      <div style=\"background-color: rgba(251, 191, 36, 0.05); border: 1px solid rgba(251, 191, 36, 0.15); border-radius: 12px; padding: 16px; margin-bottom: 16px;\">\n        <span style=\"font-size: 18px; margin-right: 8px;\">♟</span>\n        <strong style=\"color: #fbbf24;\">Chess ELO</strong>\n        <p style=\"margin: 6px 0 0 28px; font-size: 14px; color: #94a3b8;\">\n          Current Rating: <strong>{}</strong> ({}W / {}L)\n        </p>\n      </div>\n    ",
            elo, wins, losses,
        );
    }

    if cookies > 0.0 {
        // `Math.floor(cookies).toLocaleString()` — grouping, per toLocaleString.
        let floored = if cookies.is_nan() {
            f64::NAN
        } else {
            cookies.floor()
        };
        let cookies_str = jsval::number_to_locale_string(floored);
        stats_html += &format!(
            "\n      <div style=\"background-color: rgba(168, 85, 247, 0.05); border: 1px solid rgba(168, 85, 247, 0.15); border-radius: 12px; padding: 16px; margin-bottom: 16px;\">\n        <span style=\"font-size: 18px; margin-right: 8px;\">🍪</span>\n        <strong style=\"color: #c084fc;\">Cookie Clicker</strong>\n        <p style=\"margin: 6px 0 0 28px; font-size: 14px; color: #94a3b8;\">\n          Cookies Collected: <strong>{}</strong>\n        </p>\n      </div>\n    ",
            cookies_str,
        );
    }

    let content = format!(
        "\n    <h2 style=\"margin: 0 0 16px; font-size: 20px; font-weight: 700; color: #f4f4f5; text-align: center;\">⚡ Your Week in Review</h2>\n    <p style=\"margin: 0 0 24px; text-align: center; color: #94a3b8;\">Here is a summary of your stats and accomplishments on mitch.pro this week:</p>\n    <div style=\"margin-bottom: 24px;\">\n      {}\n    </div>\n    <div style=\"text-align: center; margin-bottom: 8px;\">\n      <a href=\"{}\" style=\"display: inline-block; background: linear-gradient(135deg, #a855f7, #6366f1); color: #ffffff; text-decoration: none; padding: 12px 24px; border-radius: 10px; font-weight: 700;\">Visit mitch.pro</a>\n    </div>\n  ",
        stats_html, dashboard_url,
    );
    crate::routes::admin::legacy::html_base_template(
        state,
        email,
        "Your mitch.pro week in review",
        &content,
    )
}

// ── dailyPuzzleWorker (server.js:4913-4947) ──────────────────────────────────

/// `puzzles` (server.js:1640-1646) — loaded once at boot from
/// webserver/games/chess-bot/puzzles.json; a failed read leaves the pool
/// empty and the worker returns (`if (!puzzles.length)`).
fn puzzles(state: &AppState) -> &[Value] {
    static PUZZLES: std::sync::OnceLock<Vec<Value>> = std::sync::OnceLock::new();
    PUZZLES.get_or_init(|| {
        let path = state
            .cfg
            .base_dir
            .join("webserver/games/chess-bot/puzzles.json");
        match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    })
}

fn daily_puzzle_worker(state: &Arc<AppState>) {
    match daily_puzzle_impl(state) {
        Ok(()) => {}
        Err(e) => tracing::info!("[puzzle] error: {e}"),
    }
}

fn daily_puzzle_impl(state: &Arc<AppState>) -> Result<(), String> {
    let all = puzzles(state);
    if all.is_empty() {
        return Ok(());
    }
    let now = chrono::Local::now();
    // Fires between 12-1am local (getHours() === 0).
    if now.hour() != 0 {
        return Ok(());
    }
    let current_slot = now.minute() / 10;
    // `now.toISOString().slice(0, 10)` — UTC day.
    let day_key = now
        .with_timezone(&chrono::Utc)
        .format("%Y-%m-%d")
        .to_string();
    let mut log = load_email_log(state);
    let mut changed = false;
    // `p[2] >= 800 && p[2] <= 1400` — non-numeric ratings compare NaN and drop.
    let pool: Vec<&Value> = all
        .iter()
        .filter(|p| {
            matches!(
                p.get(2).and_then(|v| v.as_f64()),
                Some(r) if (800.0..=1400.0).contains(&r)
            )
        })
        .collect();
    if pool.is_empty() {
        return Ok(());
    }
    let unsub = newsletter_unsub_set(state);
    for email in enrolled_users(state) {
        if is_unsubscribed(state, &unsub, &email) {
            continue;
        }
        if email_slot(&email, 6) != current_slot {
            continue;
        }
        let ulog = log_entry(&log, &email);
        if ulog.get("puzzle").and_then(|v| v.as_str()) == Some(day_key.as_str()) {
            continue;
        }
        let target_email = canonical_email(state, &email);
        if is_unsubscribed(state, &unsub, &target_email) {
            continue;
        }
        // The log is marked BEFORE the 1-in-10 send gate (server.js:4934-4936).
        set_log_field(&mut log, &email, "puzzle", json!(day_key.clone()));
        changed = true;
        // 1 in 10 chance of sending.
        if rand::random::<f64>() < 0.1 {
            let p = pool[(rand::random::<f64>() * pool.len() as f64) as usize];
            let fen = jsval::string_of(p.get(0));
            // `p[0].split(' ')[1] === 'w' ? 'White' : 'Black'`.
            let turn = if fen.split(' ').nth(1) == Some("w") {
                "White"
            } else {
                "Black"
            };
            let rating = p.get(2).and_then(|v| v.as_f64()).unwrap_or(f64::NAN);
            // `String(p[3] || '').split(/\s+/).filter(Boolean).slice(0, 3).join(', ')`.
            let themes = jsval::str_or(p.get(3), "")
                .split_whitespace()
                .take(3)
                .collect::<Vec<_>>()
                .join(", ");
            let target_email = canonical_email(state, &email);
            let html = make_chess_puzzle_html(state, &target_email, turn, rating, &fen, &themes);
            crate::routes::push::send_email_bg(
                state,
                &target_email,
                "Today's chess puzzle — mitch.pro",
                &html,
            );
            tracing::info!("[puzzle] sent to {target_email}");
        }
    }
    if changed {
        save_email_log(state, &log);
    }
    Ok(())
}

/// `makeChessPuzzleHtml` (server.js:1835-1851).
fn make_chess_puzzle_html(
    state: &AppState,
    email: &str,
    turn: &str,
    rating: f64,
    fen: &str,
    themes: &str,
) -> String {
    let solve_url = format!("{}/games/chess-bot/", site_url(state, email));
    let content = format!(
        "\n    <h2 style=\"margin: 0 0 16px; font-size: 20px; font-weight: 700; color: #fbbf24; text-align: center;\">♟ Daily Chess Puzzle</h2>\n    <div style=\"background-color: rgba(251, 191, 36, 0.08); border: 1px solid rgba(251, 191, 36, 0.25); border-radius: 12px; padding: 20px; margin-bottom: 24px; text-align: center;\">\n      <p style=\"margin: 0 0 12px; font-size: 16px; font-weight: 700; color: #f4f4f5;\">{} to move and find the best continuation.</p>\n      <div style=\"background: #1e293b; padding: 8px 12px; border-radius: 6px; font-family: monospace; font-size: 13px; color: #94a3b8; word-break: break-all; margin-bottom: 12px;\">\n        FEN: {}\n      </div>\n      <p style=\"margin: 0; font-size: 13px; color: #64748b;\">Rating: <strong>~{}</strong> | Themes: <em>{}</em></p>\n    </div>\n    <div style=\"text-align: center; margin-bottom: 8px;\">\n      <a href=\"{}\" style=\"display: inline-block; background: linear-gradient(135deg, #a855f7, #6366f1); color: #ffffff; text-decoration: none; padding: 14px 28px; border-radius: 10px; font-weight: 700; box-shadow: 0 10px 20px rgba(168, 85, 247, 0.3);\">Solve on mitch.pro</a>\n    </div>\n  ",
        turn, fen, fmt_js_num(rating), themes, solve_url,
    );
    crate::routes::admin::legacy::html_base_template(
        state,
        email,
        "Today's chess puzzle — mitch.pro",
        &content,
    )
}

// ── clockWarnWorker (server.js:4948-4982) ────────────────────────────────────

const CLOCK_WARN_HOURS: [f64; 2] = [12.0, 2.0];

fn clock_warn_worker(state: &Arc<AppState>) {
    match clock_warn_impl(state) {
        Ok(()) => {}
        Err(e) => tracing::info!("[clock-warn] error: {e}"),
    }
}

fn clock_warn_impl(state: &Arc<AppState>) -> Result<(), String> {
    let mut log = load_email_log(state);
    let mut changed = false;
    let now = mitch_lib::school::now_millis() as f64;
    // Clone under the lock; email I/O happens outside it.
    let games: Vec<(String, Value)> = {
        let games = state.cv_games.lock().unwrap_or_else(|e| e.into_inner());
        games.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    };
    for (game_id, g) in games {
        if jsval::string_of(g.get("status")) != "active"
            || jsval::string_of(g.get("type")) != "corr"
            || !truthy_of(g.get("clockStartedAt"))
        {
            continue;
        }
        let turn = jsval::string_of(g.get("turn"));
        // `g.turn === 'w' ? g.white : g.black`; `if (!turnEmail) continue`.
        let te = if turn == "w" {
            g.get("white")
        } else {
            g.get("black")
        };
        let Some(te) = te else {
            continue;
        };
        if !jsval::truthy(te) {
            continue;
        }
        let turn_email = jsval::string(te);
        // `g.clocks[g.turn] - (now - g.clockStartedAt)` — a missing clock or
        // junk `clockStartedAt` propagates NaN, and every comparison below
        // fails exactly like JS.
        let started = g
            .get("clockStartedAt")
            .and_then(jsval::number)
            .unwrap_or(f64::NAN);
        let remaining = g
            .get("clocks")
            .and_then(|c| c.get(&turn))
            .and_then(jsval::number)
            .map(|c| c - (now - started))
            .unwrap_or(f64::NAN);
        let ulog = log_entry(&log, &turn_email);
        // `new Set(ulog.clock_warn?.[gameId] || [])` — raw values kept for the
        // strict-number membership check (`"12"` in the file never matches 12).
        let mut warned: Vec<Value> = ulog
            .get("clock_warn")
            .and_then(|cw| cw.get(&game_id))
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        let mut new_warn = false;
        for h in CLOCK_WARN_HOURS {
            if remaining < h * 3_600_000.0
                && remaining > 0.0
                && !warned.iter().any(|w| w.as_f64() == Some(h))
            {
                let opp_raw = if turn == "w" {
                    g.get("black")
                } else {
                    g.get("white")
                };
                // `(opp || '').split('@')[0]`.
                let opp_name = jsval::str_or(opp_raw, "")
                    .split('@')
                    .next()
                    .unwrap_or("")
                    .to_string();
                let target_email = canonical_email(state, &turn_email);
                let html = make_chess_clock_warning_html(state, &target_email, h, &opp_name);
                crate::routes::push::send_email_bg(
                    state,
                    &target_email,
                    &format!("⏰ {}h left to move — mitch.pro chess", fmt_js_num(h)),
                    &html,
                );
                warned.push(jsval::num_value(h));
                new_warn = true;
                tracing::info!(
                    "[clock-warn] {}h → {target_email} game {game_id}",
                    fmt_js_num(h)
                );
            }
        }
        if new_warn {
            let mut cw = ulog.get("clock_warn").cloned().unwrap_or_else(|| json!({}));
            if let Some(cwo) = cw.as_object_mut() {
                cwo.insert(game_id.clone(), Value::Array(warned));
            }
            set_log_field(&mut log, &turn_email, "clock_warn", cw);
            changed = true;
        }
    }
    if changed {
        save_email_log(state, &log);
    }
    Ok(())
}

/// `makeChessClockWarningHtml` (server.js:1853-1869).
fn make_chess_clock_warning_html(state: &AppState, email: &str, h: f64, opp_name: &str) -> String {
    let game_url = format!("{}/games/chess-bot/", site_url(state, email));
    let plural = if h != 1.0 { "s" } else { "" };
    let content = format!(
        "\n    <h2 style=\"margin: 0 0 16px; font-size: 20px; font-weight: 700; color: #ef4444; text-align: center;\">⏰ Time is running out!</h2>\n    <div style=\"background-color: rgba(239, 68, 68, 0.08); border: 1px solid rgba(239, 68, 68, 0.25); border-radius: 12px; padding: 20px; text-align: center; margin-bottom: 24px;\">\n      <p style=\"margin: 0 0 12px; font-size: 16px; font-weight: 700; color: #f4f4f5;\">Your Turn — Chess Clock Alert</p>\n      <p style=\"margin: 0 0 16px; color: #cbd5e1;\">You have less than <strong>{} hour{}</strong> remaining to make your move against <strong>{}</strong>, or your clock will run out.</p>\n    </div>\n    <div style=\"text-align: center; margin-bottom: 8px;\">\n      <a href=\"{}\" style=\"display: inline-block; background-color: #ef4444; color: #ffffff; text-decoration: none; padding: 14px 28px; border-radius: 10px; font-weight: 700; box-shadow: 0 10px 20px rgba(239, 68, 68, 0.3);\">Go to Game</a>\n    </div>\n  ",
        fmt_js_num(h), plural, opp_name, game_url,
    );
    crate::routes::admin::legacy::html_base_template(
        state,
        email,
        &format!("⏰ {}h left to move — mitch.pro chess", fmt_js_num(h)),
        &content,
    )
}

// ── dmDigestWorker (server.js:4984-5058) ─────────────────────────────────────

fn dm_digest_worker(state: &Arc<AppState>) {
    match dm_digest_impl(state) {
        Ok(()) => {}
        Err(e) => tracing::info!("[dm-digest] error: {e}"),
    }
}

fn dm_digest_impl(state: &Arc<AppState>) -> Result<(), String> {
    let dms = state.store.read_document(
        &state.data_dir().join(mitch_lib::dm::DMS_MAIN.dms),
        json!([]),
    );
    let mut log = load_email_log(state);
    let now_ms = mitch_lib::school::now_millis();
    let offline_cutoff = now_ms - 3_600_000;
    let msg_age = (now_ms - 3_600_000) as f64;
    let mut changed = false;

    // unreadByRecip — keyed by String(m.to) (an absent key becomes the JS
    // property "undefined"), senders in first-seen order, latestTs only
    // advanced by a comparable ts (NaN comparisons never win, like JS).
    let mut unread: indexmap::IndexMap<String, (indexmap::IndexMap<String, f64>, f64)> =
        indexmap::IndexMap::new();
    if let Some(list) = dms.as_array() {
        for m in list {
            if truthy_of(m.get("read")) {
                continue;
            }
            let ts = m.get("ts").and_then(jsval::number);
            if let Some(t) = ts {
                if t > msg_age {
                    continue;
                }
            }
            let to = jsval::string_of(m.get("to"));
            let from = jsval::string_of(m.get("from"));
            let entry = unread
                .entry(to)
                .or_insert_with(|| (indexmap::IndexMap::new(), 0.0));
            *entry.0.entry(from).or_insert(0.0) += 1.0;
            if let Some(t) = ts {
                if t > entry.1 {
                    entry.1 = t;
                }
            }
        }
    }

    let unsub = newsletter_unsub_set(state);
    let day_key = chrono::Utc::now().format("%Y-%m-%d").to_string();

    for (recip, (senders, latest_ts)) in unread {
        // `(recip in e2eUsers) && (e2eUsers[recip].last_seen > offlineCutoff)`.
        let online = state
            .e2e_users
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|(k, u)| *k == recip && u.last_seen > offline_cutoff);
        if online {
            continue;
        }
        if !crate::routes::dm::notif_allowed(state, &recip, "digest") {
            continue;
        }
        let target_email = canonical_email(state, &recip);
        if is_unsubscribed(state, &unsub, &recip) || (!target_email.is_empty() && is_unsubscribed(state, &unsub, &target_email)) {
            continue;
        }

        let ulog = log_entry(&log, &recip);
        let ulog_target = log_entry(&log, &target_email);

        // Limit DM digest to 1 maximum per person per day
        let last_dm_sent = ulog
            .get("dm_digest_sent_at")
            .or_else(|| ulog_target.get("dm_digest_sent_at"))
            .and_then(jsval::number);
        if let Some(sent_at) = last_dm_sent {
            if (now_ms as f64) - sent_at < 86_400_000.0 {
                continue;
            }
        }
        let last_dm_day = ulog
            .get("dm_digest_day")
            .or_else(|| ulog_target.get("dm_digest_day"))
            .and_then(|v| v.as_str());
        if let Some(day) = last_dm_day {
            if day == day_key {
                continue;
            }
        }

        // `if (ulog.dm_digest_ts && latestTs <= ulog.dm_digest_ts) continue;`
        if truthy_of(ulog.get("dm_digest_ts")) {
            let prev = ulog.get("dm_digest_ts").and_then(jsval::number);
            match (latest_ts, prev) {
                (lt, Some(p)) if lt <= p => continue,
                // Truthy junk ("abc") compares NaN and does not skip.
                _ => {}
            }
        }
        let total = senders.values().sum::<f64>();
        // Sender display names — profiles.json is hoisted out of the JS
        // per-sender loadJson (nothing mutates it mid-run).
        let profiles = state
            .store
            .read_document(&state.data_dir().join("profiles.json"), json!({}));
        let names = senders
            .keys()
            .map(|e| {
                let norm = mitch_lib::auth::normalize_email(e);
                let prof = profiles.get(&norm).cloned().unwrap_or_else(|| json!({}));
                let pick = ["displayName", "nickname", "username"]
                    .iter()
                    .find(|k| truthy_of(prof.get(**k)))
                    .and_then(|k| prof.get(*k).map(jsval::string));
                let fallback = {
                    let c = canonical_email(state, e);
                    c.split('@').next().unwrap_or("").to_string()
                };
                pick.unwrap_or(fallback)
            })
            .collect::<Vec<_>>()
            .join(", ");
        let total_str = fmt_js_num(total);
        let plural = if total != 1.0 { "s" } else { "" };
        let html = make_unread_messages_html(state, &target_email, &total_str, &names);
        crate::routes::push::send_email_bg(
            state,
            &target_email,
            &format!("💬 {total_str} unread message{plural} on mitch.pro"),
            &html,
        );
        set_log_field(
            &mut log,
            &recip,
            "dm_digest_ts",
            jsval::num_value(latest_ts),
        );
        set_log_field(
            &mut log,
            &recip,
            "dm_digest_sent_at",
            jsval::num_value(now_ms as f64),
        );
        set_log_field(
            &mut log,
            &recip,
            "dm_digest_day",
            json!(day_key),
        );
        if &target_email != &recip {
            set_log_field(
                &mut log,
                &target_email,
                "dm_digest_sent_at",
                jsval::num_value(now_ms as f64),
            );
            set_log_field(
                &mut log,
                &target_email,
                "dm_digest_day",
                json!(day_key),
            );
        }
        changed = true;
        tracing::info!(
            "[dm-digest] {} msgs from {} senders → {target_email}",
            fmt_js_num(total),
            senders.len()
        );
    }

    // ── Matrix Chat Unread Messages Digest (server.js:5020-5055) ────────────
    // Policy: MATRIX_MESSAGE_EMAIL_INTERVAL_MS = 7 * 24 * 60 * 60 * 1000 — a
    // once-stuck-unread recipient who never comes back should get reminded
    // weekly, not every single day forever.
    const MATRIX_MESSAGE_EMAIL_INTERVAL_MS: f64 = 7.0 * 86_400_000.0;
    let matrix = state.store.read_document(
        &state.data_dir().join("matrix_notifications.json"),
        json!({}),
    );
    let matrix_sent_file = state.data_dir().join("matrix_email_sent.json");
    let matrix_sent_doc = state.store.read_document(&matrix_sent_file, json!({}));

    if let Some(map) = matrix.as_object() {
        for (recip, notifs) in map {
            let Some(list) = notifs.as_array() else {
                continue;
            };
            // `n.ts <= msgAge` with a missing/junk ts is false, like NaN.
            let unread_list: Vec<&Value> = list
                .iter()
                .filter(|n| {
                    !truthy_of(n.get("read"))
                        && matches!(
                            n.get("ts").and_then(jsval::number),
                            Some(t) if t <= msg_age
                        )
                })
                .collect();
            if unread_list.is_empty() {
                continue;
            }
            let last_seen = state
                .matrix_user_last_seen
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(recip)
                .copied()
                .unwrap_or(0);
            // Literal JS quirk (server.js:5023): the *duration*
            // `Date.now() - (lastSeen || 0)` is compared against the
            // *timestamp* cutoff `now - 3_600_000`, which reduces to
            // online ⟺ lastSeen > 3_600_000 — any real matrix lastSeen
            // entry counts as online. Preserved verbatim.
            if (now_ms as i64) - last_seen < offline_cutoff {
                continue;
            }
            if !crate::routes::dm::notif_allowed(state, recip, "digest") {
                continue;
            }
            let target_email = canonical_email(state, recip);
            if target_email.is_empty() || !target_email.contains('@') {
                continue;
            }

            if is_unsubscribed(state, &unsub, recip)
                || is_unsubscribed(state, &unsub, &target_email)
            {
                continue;
            }

            let norm_recip = mitch_lib::auth::normalize_email(recip);
            let norm_target = mitch_lib::auth::normalize_email(&target_email);

            // 1. Check matrix_email_sent.json (strict 24-hour interval)
            if let Some(mobj) = matrix_sent_doc.as_object() {
                let sent_at = mobj
                    .get(recip)
                    .or_else(|| mobj.get(&target_email))
                    .or_else(|| if !norm_recip.is_empty() { mobj.get(&norm_recip) } else { None })
                    .or_else(|| if !norm_target.is_empty() { mobj.get(&norm_target) } else { None })
                    .and_then(jsval::number);
                if let Some(st) = sent_at {
                    if (now_ms as f64) - st < MATRIX_MESSAGE_EMAIL_INTERVAL_MS {
                        continue;
                    }
                }
            }

            // 2. Check email_log.json across all key variants
            let ulog = log_entry(&log, recip);
            let ulog_target = log_entry(&log, &target_email);
            let ulog_norm_r = if !norm_recip.is_empty() { log_entry(&log, &norm_recip) } else { json!({}) };
            let ulog_norm_t = if !norm_target.is_empty() { log_entry(&log, &norm_target) } else { json!({}) };

            let last_sent = ulog
                .get("matrix_digest_sent_at")
                .or_else(|| ulog_target.get("matrix_digest_sent_at"))
                .or_else(|| ulog_norm_r.get("matrix_digest_sent_at"))
                .or_else(|| ulog_norm_t.get("matrix_digest_sent_at"))
                .and_then(jsval::number);
            if let Some(sent_at) = last_sent {
                if (now_ms as f64) - sent_at < MATRIX_MESSAGE_EMAIL_INTERVAL_MS {
                    continue;
                }
            }
            let last_day = ulog
                .get("matrix_digest_day")
                .or_else(|| ulog_target.get("matrix_digest_day"))
                .or_else(|| ulog_norm_r.get("matrix_digest_day"))
                .or_else(|| ulog_norm_t.get("matrix_digest_day"))
                .and_then(|v| v.as_str());
            if let Some(day) = last_day {
                if day == day_key {
                    continue;
                }
            }

            let mut latest = f64::NEG_INFINITY;
            for n in &unread_list {
                // `n.ts || 0`, then Math.max.
                let t = jsval::number(&jsval::or(n.get("ts"), json!(0))).unwrap_or(f64::NAN);
                if t > latest {
                    latest = t;
                }
            }
            // `if (ulog.matrix_digest_ts && latestMatrixTs <= ...) continue;`
            // — NaN latest never skips, so a junk ts re-sends every run, like JS.
            if truthy_of(ulog.get("matrix_digest_ts")) {
                let prev = ulog.get("matrix_digest_ts").and_then(jsval::number);
                match (latest, prev) {
                    (lt, Some(p)) if lt <= p => continue,
                    _ => {}
                }
            }
            // `unreadList.reduce((acc, n) => acc + (n.count || 1), 0)`.
            let total = unread_list
                .iter()
                .map(|n| jsval::number(&jsval::or(n.get("count"), json!(1))).unwrap_or(f64::NAN))
                .sum::<f64>();
            // `Array.from(new Set(unreadList.map(n => n.sender).filter(Boolean)))`.
            let mut seen: HashSet<String> = HashSet::new();
            let mut sender_names = Vec::new();
            for n in &unread_list {
                let s = n.get("sender");
                if s.map(jsval::truthy).unwrap_or(false) {
                    let s = jsval::string(s.unwrap_or(&Value::Null));
                    if seen.insert(s.clone()) {
                        sender_names.push(s);
                    }
                }
            }
            let sender_names = if sender_names.is_empty() {
                "Matrix users".to_string()
            } else {
                sender_names.join(", ")
            };
            let total_str = fmt_js_num(total);
            let plural = if total != 1.0 { "s" } else { "" };
            let first = unread_list.first();
            let room_title = jsval::str_or(first.and_then(|n| n.get("roomTitle")), "");
            let preview_text = jsval::str_or(
                first.and_then(|n| n.get("detail")),
                "You have unread chat messages waiting on Mitch.pro.",
            );
            let title = format!("💬 You have {total_str} unread Matrix message{plural}");
            let html = make_matrix_notification_email_html(
                state,
                &target_email,
                &title,
                &sender_names,
                &room_title,
                &preview_text,
            );
            crate::routes::push::send_email_bg(
                state,
                &target_email,
                &format!("💬 {total_str} unread Matrix message{plural} on mitch.pro"),
                &html,
            );

            // Record sent stamp across all persistent layers and key aliases
            crate::routes::matrix::record_matrix_email_sent(state, recip);
            crate::routes::matrix::record_matrix_email_sent(state, &target_email);
            if !norm_recip.is_empty() {
                crate::routes::matrix::record_matrix_email_sent(state, &norm_recip);
            }
            if !norm_target.is_empty() {
                crate::routes::matrix::record_matrix_email_sent(state, &norm_target);
            }

            let keys_to_stamp = vec![
                recip.to_string(),
                target_email.clone(),
                norm_recip.clone(),
                norm_target.clone(),
            ];
            for k in keys_to_stamp {
                if k.is_empty() {
                    continue;
                }
                set_log_field(
                    &mut log,
                    &k,
                    "matrix_digest_ts",
                    jsval::num_value(latest),
                );
                set_log_field(
                    &mut log,
                    &k,
                    "matrix_digest_sent_at",
                    jsval::num_value(now_ms as f64),
                );
                set_log_field(
                    &mut log,
                    &k,
                    "matrix_digest_day",
                    json!(day_key),
                );
            }

            changed = true;
            tracing::info!("[matrix-digest] {} msgs → {target_email}", total_str);
        }
    }

    if changed {
        save_email_log(state, &log);
    }
    Ok(())
}

/// `makeUnreadMessagesHtml` (server.js:1871-1887).
fn make_unread_messages_html(
    state: &AppState,
    email: &str,
    total: &str,
    sender_names: &str,
) -> String {
    let chat_url = format!("{}/encrypt.html", site_url(state, email));
    let plural = if total != "1" { "s" } else { "" };
    let content = format!(
        "\n    <h2 style=\"margin: 0 0 16px; font-size: 20px; font-weight: 700; color: #a855f7; text-align: center;\">💬 Unread Messages</h2>\n    <div style=\"background-color: rgba(168, 85, 247, 0.08); border: 1px solid rgba(168, 85, 247, 0.25); border-radius: 12px; padding: 20px; text-align: center; margin-bottom: 24px;\">\n      <p style=\"margin: 0 0 12px; font-size: 16px; font-weight: 700; color: #f4f4f5;\">You have new mail in your inbox!</p>\n      <p style=\"margin: 0; color: #cbd5e1;\">You have <strong>{} unread message{}</strong> waiting for you from <strong>{}</strong>.</p>\n    </div>\n    <div style=\"text-align: center; margin-bottom: 8px;\">\n      <a href=\"{}\" style=\"display: inline-block; background: linear-gradient(135deg, #a855f7, #6366f1); color: #ffffff; text-decoration: none; padding: 14px 28px; border-radius: 10px; font-weight: 700; box-shadow: 0 10px 20px rgba(168, 85, 247, 0.3);\">Open Chat Room</a>\n    </div>\n  ",
        total, plural, sender_names, chat_url,
    );
    crate::routes::admin::legacy::html_base_template(
        state,
        email,
        &format!("💬 {total} unread message{plural} on mitch.pro"),
        &content,
    )
}

/// `makeMatrixNotificationEmailHtml` (server.js:8517-8568) with the fixed
/// isCall=false / isInvite=false the digest passes.
fn make_matrix_notification_email_html(
    state: &AppState,
    email: &str,
    title: &str,
    sender_name: &str,
    room_title: &str,
    preview_text: &str,
) -> String {
    // roomUrl `/matrix/` never starts with http → siteUrl(email) + roomUrl.
    let dest_url = format!("{}{}", site_url(state, email), "/matrix/");
    let accent_color = "#a855f7";
    let icon_emoji = "💬";
    let action_text = "Open Matrix Chat";
    // `String(previewText).replace(/</g, '&lt;').replace(/>/g, '&gt;')` — only
    // when previewText is truthy; empty falls to 'New chat activity'.
    let preview = if !preview_text.is_empty() {
        preview_text.replace('<', "&lt;").replace('>', "&gt;")
    } else {
        "New chat activity".to_string()
    };
    let room = if room_title.is_empty() {
        String::new()
    } else {
        format!(
            "<span style=\"color: #64748b; margin-left: 8px; font-size: 13px;\">in {room_title}</span>"
        )
    };
    let content = format!(
        "\n    <div style=\"text-align: center; margin-bottom: 20px;\">\n      <div style=\"display: inline-block; width: 56px; height: 56px; line-height: 56px; border-radius: 16px; background: rgba(168, 85, 247, 0.15); border: 1px solid rgba(168, 85, 247, 0.3); font-size: 28px;\">\n        {icon_emoji}\n      </div>\n      <h2 style=\"margin: 14px 0 6px; font-size: 22px; font-weight: 800; color: #f4f4f5;\">{title}</h2>\n      <p style=\"margin: 0; color: #94a3b8; font-size: 14px;\">Mitch.pro Decentralized Matrix Chat</p>\n    </div>\n\n    <div style=\"background-color: rgba(255, 255, 255, 0.04); border: 1px solid rgba(255, 255, 255, 0.08); border-radius: 14px; padding: 18px 20px; margin-bottom: 24px;\">\n      <div style=\"display: flex; align-items: center; margin-bottom: 10px;\">\n        <strong style=\"color: #f4f4f5; font-size: 15px;\">{sender_name}</strong>\n        {room}\n      </div>\n      <div style=\"color: #e2e8f0; font-size: 14px; line-height: 1.5; white-space: pre-wrap; word-break: break-word; background: rgba(0, 0, 0, 0.25); padding: 12px 14px; border-radius: 8px; border-left: 3px solid {accent_color};\">\n        {preview}\n      </div>\n    </div>\n\n    <div style=\"text-align: center; margin-bottom: 12px;\">\n      <a href=\"{dest_url}\" style=\"display: inline-block; background: linear-gradient(135deg, {accent_color}, #6366f1); color: #ffffff; text-decoration: none; padding: 14px 28px; border-radius: 12px; font-weight: 700; font-size: 15px; box-shadow: 0 10px 24px rgba(99, 102, 241, 0.35);\">\n        {action_text}\n      </a>\n    </div>\n    <p style=\"text-align: center; margin: 0; font-size: 12px; color: #64748b;\">\n      Tip: Turn on web push notifications on your device to receive instant incoming rings and chat alerts!\n    </p>\n  ",
        icon_emoji = icon_emoji,
        title = title,
        sender_name = sender_name,
        room = room,
        accent_color = accent_color,
        preview = preview,
        dest_url = dest_url,
        action_text = action_text,
    );
    crate::routes::admin::legacy::html_base_template(state, email, title, &content)
}

// ── dmPruneWorker (server.js:26352-26364) ────────────────────────────────────

fn dm_prune_tick(state: &AppState) {
    let now = mitch_lib::school::now_millis() as f64;
    for store in [mitch_lib::dm::DMS_MAIN, mitch_lib::dm::DMS_PICKLE] {
        let path = state.data_dir().join(store.dms);
        let dms = state.store.read_document(&path, json!([]));
        let len = dms.as_array().map(|a| a.len()).unwrap_or(0);
        let empty: Vec<Value> = Vec::new();
        let pruned = mitch_lib::dm::prune_dms(
            &state.store,
            &state.cfg.data_dir,
            dms.as_array().unwrap_or(&empty),
            store.pickle,
            now,
        );
        if pruned.len() != len {
            state
                .store
                .write_document(&path, &Value::Array(pruned))
                .ok();
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────
// Pure-helper parity against live JS reference values (computed with bun:
// Math.imul hash, Date.parse, `new URL(..., 'http://x').pathname`).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_slot_matches_math_imul_hash() {
        // bun: `h = (Math.imul(h, 31) + email.charCodeAt(i)) >>> 0` per char.
        assert_eq!(email_slot("probe_idle_b@student.rjuhsd.us", 6), 5);
        assert_eq!(email_slot("probe_idle_r@student.rjuhsd.us", 6), 3);
        assert_eq!(email_slot("a@b.c", 6), 2);
        assert_eq!(email_slot("mitchell.fogler@student.rjuhsd.us", 6), 2);
        assert_eq!(email_slot("x", 6), 0);
        assert_eq!(email_slot("anything@anywhere.org", 1), 0);
    }

    #[test]
    fn js_date_parse_matches_date_parse() {
        // Z-suffixed (TZ-independent) — exact JS Date.parse values.
        assert_eq!(
            js_date_parse(Some(&json!("2026-09-15T12:00:00Z"))),
            Some(1_789_473_600_000.0)
        );
        assert_eq!(
            js_date_parse(Some(&json!("2026-09-15T19:00:00Z"))),
            Some(1_789_498_800_000.0)
        );
        // No zone → local time (the dev/prod hosts are America/Los_Angeles,
        // where Date.parse('2026-09-15T12:00:00') === Date.parse('...T19:00:00Z')).
        // Asserted against chrono's own local resolution so CI in any TZ passes.
        let naive =
            chrono::NaiveDateTime::parse_from_str("2026-09-15T12:00:00", "%Y-%m-%dT%H:%M:%S")
                .unwrap()
                .and_local_timezone(chrono::Local)
                .single()
                .unwrap();
        assert_eq!(
            js_date_parse(Some(&json!("2026-09-15T12:00:00"))),
            Some(naive.timestamp_millis() as f64)
        );
        // NaN shapes.
        assert_eq!(js_date_parse(None), None); // undefined || ''
        assert_eq!(js_date_parse(Some(&json!(""))), None);
        assert_eq!(js_date_parse(Some(&json!("garbage"))), None);
        // Non-string truthy values are String()-coerced first.
        assert_eq!(js_date_parse(Some(&json!(12345))), None); // "12345" → NaN
    }

    #[test]
    fn url_pathname_matches_new_url_base() {
        assert_eq!(
            url_pathname("/games/chess-bot/?a=1"),
            Some("/games/chess-bot/".into())
        );
        assert_eq!(url_pathname("/games/x#frag"), Some("/games/x".into()));
        assert_eq!(url_pathname(""), Some("/".into())); // new URL('', 'http://x')
        assert_eq!(url_pathname("games/x"), Some("/games/x".into())); // relative
        assert_eq!(
            url_pathname("https://mitch.pro/games/casino"),
            Some("/games/casino".into())
        );
        assert_eq!(url_pathname("https://mitch.pro"), Some("/".into()));
        assert_eq!(url_pathname("//evil.com/x"), Some("/x".into())); // protocol-relative
    }

    #[test]
    fn game_name_is_last_nonempty_segment() {
        // topGame[0].split('/').filter(Boolean).pop()
        let name = |t: &str| {
            t.rsplit('/')
                .find(|seg| !seg.is_empty())
                .unwrap_or("")
                .to_string()
        };
        assert_eq!(name("/games/chess-bot/"), "chess-bot");
        assert_eq!(name("/games/casino"), "casino");
        assert_eq!(name("/"), "");
    }

    #[test]
    fn fmt_js_num_renders_js_strings() {
        assert_eq!(fmt_js_num(3.0), "3");
        assert_eq!(fmt_js_num(1200.0), "1200");
        assert_eq!(fmt_js_num(f64::NAN), "NaN");
        assert_eq!(fmt_js_num(f64::INFINITY), "Infinity");
    }

    #[test]
    fn set_log_field_drops_nan_as_json_stringify_does() {
        // Assigning NaN keeps the key in memory but JSON.stringify drops it —
        // num_value renders NaN as Value::Null and the field is removed.
        let mut log = json!({"a@b.c": {"dm_digest_ts": 5}});
        set_log_field(&mut log, "a@b.c", "dm_digest_ts", Value::Null);
        assert!(log["a@b.c"].get("dm_digest_ts").is_none());
        assert!(log["a@b.c"].as_object().unwrap().is_empty());
        set_log_field(&mut log, "a@b.c", "dm_digest_ts", json!(123));
        assert_eq!(log["a@b.c"]["dm_digest_ts"], 123);
    }

    #[test]
    fn truthy_of_models_undefined_keys() {
        assert!(!truthy_of(None));
        assert!(!truthy_of(Some(&Value::Null)));
        assert!(!truthy_of(Some(&json!(0))));
        assert!(!truthy_of(Some(&json!(""))));
        assert!(!truthy_of(Some(&json!(false))));
        assert!(truthy_of(Some(&json!(1))));
        assert!(truthy_of(Some(&json!("x"))));
        assert!(truthy_of(Some(&json!({}))));
    }

    fn test_state() -> (Arc<AppState>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "mitch-server-workers-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap_or_default();
        let cfg = crate::hosts::SiteConfig::load();
        let cfg = crate::hosts::SiteConfig {
            data_dir: dir.join("data"),
            ..cfg
        };
        let store = Arc::new(
            mitch_lib::data::DataStore::open(&dir, &dir.join("data"))
                .unwrap_or_else(|e| panic!("store: {e}")),
        );
        (Arc::new(AppState::new(cfg, Arc::clone(&store))), dir)
    }

    #[test]
    fn test_newsletter_unsub_from_store_and_disk() {
        let (state, dir) = test_state();

        // 1. Store write (simulating API / DB unsubscribe)
        let _ = state.store.write_document(
            &state.data_dir().join("newsletter_unsub.json"),
            &json!(["store_unsub@example.com", "first.last@student.rjuhsd.us"]),
        );

        // 2. Disk write (simulating manual edit on disk)
        let _ = std::fs::write(
            state.data_dir().join("newsletter_unsub.json"),
            serde_json::to_string(&json!(["disk_unsub@example.com"])).unwrap(),
        );

        let set = newsletter_unsub_set(&state);
        // Direct matches
        assert!(set.contains("store_unsub@example.com"));
        assert!(set.contains("first.last@student.rjuhsd.us"));
        assert!(set.contains("disk_unsub@example.com"));

        // Normalized match: firstlast@student.rjuhsd.us should also be present
        assert!(set.contains("firstlast@student.rjuhsd.us"));

        // is_unsubscribed helper checks
        assert!(is_unsubscribed(&state, &set, "store_unsub@example.com"));
        assert!(is_unsubscribed(&state, &set, "disk_unsub@example.com"));
        assert!(is_unsubscribed(&state, &set, "firstlast@student.rjuhsd.us"));
        assert!(is_unsubscribed(
            &state,
            &set,
            "first.last@student.rjuhsd.us"
        ));
        assert!(!is_unsubscribed(&state, &set, "active_user@example.com"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn test_enrolled_users_filters_unsubscribed() {
        let (state, dir) = test_state();

        // Populate tokens.json with 3 enrolled users
        let tokens = json!({
            "tok1": { "email": "active@example.com", "used": true },
            "tok2": { "email": "unsubbed@example.com", "used": true },
            "tok3": { "email": "alice.student@student.rjuhsd.us", "claimed_domains": ["mitch.pro"] },
        });
        let _ = state
            .store
            .write_document(&state.data_dir().join("tokens.json"), &tokens);

        // Unsubscribe tok2 directly, and tok3 via normalized address (without dot)
        let _ = state.store.write_document(
            &state.data_dir().join("newsletter_unsub.json"),
            &json!(["unsubbed@example.com", "alicestudent@student.rjuhsd.us"]),
        );

        let enrolled = enrolled_users(&state);
        assert_eq!(enrolled, vec!["active@example.com"]);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_matrix_digest_daily_limit() {
        let (state, dir) = test_state();

        let email = "tester@mitch.pro";
        let now = mitch_lib::school::now_millis();
        let old_ts = (now - 2 * 3600 * 1000) as f64; // 2 hours ago (eligible: > 1h)

        // 1. Write matrix notification
        let notifs = json!({
            email: [
                {
                    "id": "matrix:!room:1",
                    "type": "matrix",
                    "roomId": "!room:test",
                    "sender": "Alice",
                    "roomTitle": "General",
                    "detail": "Hello world",
                    "read": false,
                    "count": 1,
                    "ts": old_ts,
                }
            ]
        });
        let _ = state.store.write_document(&state.data_dir().join("matrix_notifications.json"), &notifs);

        // Run digest worker
        let res = dm_digest_impl(&state);
        assert!(res.is_ok());

        // Check email_log was written with sent_at and day
        let log = load_email_log(&state);
        let ulog = log_entry(&log, email);
        assert!(truthy_of(ulog.get("matrix_digest_ts")));
        assert!(truthy_of(ulog.get("matrix_digest_sent_at")));
        let day = ulog.get("matrix_digest_day").and_then(|v| v.as_str()).unwrap();
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        assert_eq!(day, today);

        // 2. Add a new unread notification with newer timestamp
        let newer_ts = (now - 3600 * 1000) as f64;
        let notifs2 = json!({
            email: [
                {
                    "id": "matrix:!room:2",
                    "type": "matrix",
                    "roomId": "!room:test",
                    "sender": "Bob",
                    "roomTitle": "General",
                    "detail": "Second message",
                    "read": false,
                    "count": 1,
                    "ts": newer_ts,
                }
            ]
        });
        let _ = state.store.write_document(&state.data_dir().join("matrix_notifications.json"), &notifs2);

        // Run digest worker again
        let res2 = dm_digest_impl(&state);
        assert!(res2.is_ok());

        // Verify matrix_digest_ts was NOT updated to newer_ts because of the daily limit!
        let log2 = load_email_log(&state);
        let ulog2 = log_entry(&log2, email);
        assert_eq!(ulog2.get("matrix_digest_ts").and_then(jsval::number), Some(old_ts));

        let _ = std::fs::remove_dir_all(dir);
    }
}

