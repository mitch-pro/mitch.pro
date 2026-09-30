//! `/api/games/*` route group (plan Step 12).
//!
//! Batch 1 — game-portal rewards + game stats/categories:
//! - `GET /api/game-categories` — server.js:19081-19084 (`getGameCategories`)
//! - `GET /api/game-stats` — server.js:19107-19110 (`globalGameStats`)
//! - `POST /api/game-portal/heartbeat` — server.js:16338-16385, settle logic
//!   ported from `lib/game_portal_rewards.js:18-48`
//!
//! `GET /api/game-portal/status` is deliberately NOT served: the bun handler
//! (server.js:16319-16336) is dead code — it sits inside the
//! `if (method === 'POST')` block (13991-17779) and bun 404s the path.
//!
//! Later batches add the idle-game endpoints (adrian-clicker, kodys-keyboard,
//! pennys-piano-keys, sebastians-piccolo, lillians-logic, richard-riches).

use axum::http::{HeaderMap, Method};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::errors::json_resp;
use crate::state::AppState;
use mitch_lib::jsval;

mod idle;
mod mini;

/// `GAME_PORTAL_REWARD_PER_MINUTE` (lib/game_portal_rewards.js:1).
pub const GAME_PORTAL_REWARD_PER_MINUTE: i64 = 2;
/// `GAME_PORTAL_DAILY_CAP` (lib/game_portal_rewards.js:2).
pub const GAME_PORTAL_DAILY_CAP: i64 = 240;
/// `GAME_PORTAL_HEARTBEAT_MIN_MS` (lib/game_portal_rewards.js:3).
const HEARTBEAT_MIN_MS: i64 = 5_000;
/// `GAME_PORTAL_HEARTBEAT_MAX_MS` (lib/game_portal_rewards.js:4).
const HEARTBEAT_MAX_MS: i64 = 45_000;

/// One in-memory reward session (`gamePortalSessions` map value). Lives in
/// `AppState`, keyed by normalized email — same lifecycle as JS.
#[derive(Clone, Debug, Default)]
pub struct GamePortalSession {
    pub game: String,
    pub active: bool,
    pub last_seen: i64,
    pub accrued_ms: i64,
}

pub fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body: &Value,
    body_bytes: &[u8],
) -> Option<axum::response::Response> {
    if path == "/api/game-categories" && *method == Method::GET {
        return Some(game_categories(state));
    }
    if path == "/api/game-stats" && *method == Method::GET {
        return Some(game_stats(state));
    }
    if path == "/api/game-portal/heartbeat" && *method == Method::POST {
        return Some(game_portal_heartbeat(state, headers, body, body_bytes));
    }
    if let Some(resp) = idle::handle(state, method, path, headers, body, body_bytes) {
        return Some(resp);
    }
    if let Some(resp) = mini::handle(state, method, path, headers, body, body_bytes) {
        return Some(resp);
    }
    if path == "/api/game-portal/status" && *method == Method::GET {
        return Some(game_portal_status(state, headers));
    }
    None
}

/// `GET /api/game-categories` — the `getGameCategories()` merge (server.js
/// 7805-7812): base categories, then local, then external override later
/// keys. JS caches the merge in `gameCategoriesCache` until `/api/games`
/// reload; the port reads the files fresh every call, which serves identical
/// content and matches how the /api/games port (Step 7) reads the same files.
fn game_categories(state: &Arc<AppState>) -> axum::response::Response {
    let mut cats = serde_json::Map::new();
    for name in [
        "game_categories.json",
        "game_categories_local.json",
        "game_categories_external.json",
    ] {
        let doc = state
            .store
            .read_document(&state.cfg.data_dir.join(name), json!({}));
        if let Some(obj) = doc.as_object() {
            for (k, v) in obj {
                cats.insert(k.clone(), v.clone());
            }
        }
    }
    json_resp(200, json!({ "categories": Value::Object(cats) }))
}

/// `GET /api/game-stats` — the `globalGameStats` map (server.js 1211-1252).
/// JS keeps it in memory, seeded from `data/global_game_stats.json` (and the
/// master log when the file is empty at boot); the port reads the file
/// directly — the ping endpoint (proxy.rs) updates the same file, so the
/// served map is identical.
fn game_stats(state: &Arc<AppState>) -> axum::response::Response {
    let stats = state.store.read_document(
        &state.cfg.data_dir.join("global_game_stats.json"),
        json!({}),
    );
    json_resp(200, json!({ "stats": stats }))
}

/// The sid auth ladder shared by both game-portal endpoints (server.js
/// 16322-16326 and 16341-16345): `studentId || id`, validId + isRevoked, then
/// emailFromSid — each failure is `401 {authenticated:false}`.
fn portal_auth(
    state: &Arc<AppState>,
    headers: &HeaderMap,
) -> Result<String, Box<axum::response::Response>> {
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cookies.get("id").unwrap_or(""))
        .to_string();
    if !mitch_lib::auth::valid_id(&sid, &state.id_secret) || is_revoked_id(state, &sid) {
        return Err(Box::new(json_resp(401, json!({ "authenticated": false }))));
    }
    match mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid) {
        Some(email) => Ok(email),
        None => Err(Box::new(json_resp(401, json!({ "authenticated": false })))),
    }
}

fn is_revoked_id(state: &AppState, sid: &str) -> bool {
    state
        .store
        .read_document(&state.data_dir().join("revoked.json"), json!({}))
        .get(sid)
        .is_some()
}

/// `GET /api/game-portal/status` — returns authentication status, current coins,
/// daily earned amount, daily cap, and reward per minute.
fn game_portal_status(state: &Arc<AppState>, headers: &HeaderMap) -> axum::response::Response {
    let email = match portal_auth(state, headers) {
        Ok(e) => e,
        Err(resp) => return *resp,
    };
    let norm = mitch_lib::auth::normalize_email(&email);
    let now = now_millis();
    let today = game_portal_day_key(now);
    let stats_file = state.data_dir().join("user_stats.json");
    let stats = state.store.read_document(&stats_file, json!({}));
    let user = stats.get(&norm);
    let daily_earned = if user
        .and_then(|u| u.get("game_portal_reward_day"))
        .and_then(|v| v.as_str())
        == Some(today.as_str())
    {
        jsval::number(user.and_then(|u| u.get("game_portal_reward_today")).unwrap_or(&json!(0)))
            .map(|n| n.max(0.0))
            .unwrap_or(0.0)
    } else {
        0.0
    };
    let coins = mitch_lib::coins::get_coins(&state.store, &state.cfg.data_dir, &email);
    json_resp(
        200,
        json!({
            "authenticated": true,
            "coins": coins,
            "dailyEarned": daily_earned,
            "dailyCap": GAME_PORTAL_DAILY_CAP,
            "rewardPerMinute": GAME_PORTAL_REWARD_PER_MINUTE,
        }),
    )
}

/// Which stage of the inline `studentId || id` ladder failed. The idle/mini
/// game endpoints each write their own 401 bodies (unlike the game-portal
/// pair's shared `{authenticated:false}`), so the shared helper reports the
/// stage and lets the caller build the exact response.
pub(crate) enum SidFail {
    /// `validId(sid)` false (incl. missing cookie).
    InvalidId,
    /// `emailFromSid(sid)` falsy.
    MissingIdentity,
}

/// The ladder the idle/mini game endpoints share (server.js 22425-22429,
/// 22868-22871, …): `cookies['studentId'] || cookies['id'] || ''`, then
/// `validId(sid)` and `emailFromSid(sid)`. Note bun does NOT check the
/// revoked-id store on this ladder (unlike the game-portal pair).
pub(crate) fn sid_email(state: &Arc<AppState>, headers: &HeaderMap) -> Result<String, SidFail> {
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cookies.get("id").unwrap_or(""))
        .to_string();
    if !mitch_lib::auth::valid_id(&sid, &state.id_secret) {
        return Err(SidFail::InvalidId);
    }
    match mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid) {
        Some(email) => Ok(email),
        None => Err(SidFail::MissingIdentity),
    }
}

/// The adrian/richard ladder (server.js:22868-22871, same bodies at 22734+):
/// `401 {error:'auth required'}` / `401 {error:'identity missing'}`.
pub(crate) fn games_email(
    state: &Arc<AppState>,
    headers: &HeaderMap,
) -> Result<String, Box<axum::response::Response>> {
    match sid_email(state, headers) {
        Ok(email) => Ok(email),
        Err(SidFail::InvalidId) => Err(Box::new(json_resp(
            401,
            json!({ "error": "auth required" }),
        ))),
        Err(SidFail::MissingIdentity) => Err(Box::new(json_resp(
            401,
            json!({ "error": "identity missing" }),
        ))),
    }
}

/// `gamePortalDayKey(now)` — UTC ISO date (lib/game_portal_rewards.js:6-8).
fn game_portal_day_key(now_ms: i64) -> String {
    let full = mitch_lib::coins::js_iso_date_from(now_ms);
    full.chars().take(10).collect()
}

/// `POST /api/game-portal/heartbeat` — server.js:16338-16385.
fn game_portal_heartbeat(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    _body: &Value,
    body_bytes: &[u8],
) -> axum::response::Response {
    let email = match portal_auth(state, headers) {
        Ok(e) => e,
        Err(resp) => return *resp,
    };
    let Some(body) = crate::routes::me::parse_body_strict(body_bytes) else {
        return json_resp(400, json!({ "error": "bad json" }));
    };

    let norm = mitch_lib::auth::normalize_email(&email);
    let game = normalize_game_portal_title(&jsval::or(body.get("game"), json!("")));
    let active = body.get("active").and_then(|v| v.as_bool()) == Some(true) && !game.is_empty();
    let now = now_millis();
    let today = game_portal_day_key(now);

    // stats = loadUserStats(); if (!stats[norm]) stats[norm] = {};
    // if (stats[norm].game_portal_reward_day !== today) { reset + save }
    let stats_file = state.data_dir().join("user_stats.json");
    let mut stats = state.store.read_document(&stats_file, json!({}));
    let user = stats
        .as_object_mut()
        .map(|m| m.entry(norm.clone()).or_insert_with(|| json!({})))
        .cloned()
        .unwrap_or(json!({}));
    let mut user = match user {
        Value::Object(o) => o,
        _ => serde_json::Map::new(),
    };
    let day_matches =
        user.get("game_portal_reward_day").and_then(|v| v.as_str()) == Some(today.as_str());
    if !day_matches {
        user.insert("game_portal_reward_day".into(), json!(today.clone()));
        user.insert("game_portal_reward_today".into(), json!(0));
        stats[norm.clone()] = Value::Object(user.clone());
        let _ = state.store.write_document(&stats_file, &stats);
    }

    let daily_earned = jsval::number(user.get("game_portal_reward_today").unwrap_or(&json!(0)))
        .map(|n| n.max(0.0))
        .unwrap_or(0.0);

    // settled = settleGamePortalHeartbeat(gamePortalSessions.get(norm), ...)
    let previous = state
        .game_portal_sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&norm)
        .cloned();
    let settled = settle_game_portal_heartbeat(previous.as_ref(), now, &game, active, daily_earned);
    state
        .game_portal_sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(norm.clone(), settled.next.clone());

    if settled.earned > 0 {
        mitch_lib::coins::add_coins(
            &state.store,
            &state.cfg.data_dir,
            &email,
            settled.earned as f64,
            state.coin_multiplier(),
            "",
        );
        // stats[norm].game_portal_reward_today = Number(...||0) + earned
        let cur =
            jsval::number(user.get("game_portal_reward_today").unwrap_or(&json!(0))).unwrap_or(0.0);
        user.insert(
            "game_portal_reward_today".into(),
            jsval::num_value(cur + settled.earned as f64),
        );
        let minutes_cur =
            jsval::number(user.get("game_portal_minutes").unwrap_or(&json!(0))).unwrap_or(0.0);
        user.insert(
            "game_portal_minutes".into(),
            jsval::num_value(minutes_cur + settled.minutes as f64),
        );
        let coins_cur =
            jsval::number(user.get("game_portal_coins").unwrap_or(&json!(0))).unwrap_or(0.0);
        user.insert(
            "game_portal_coins".into(),
            jsval::num_value(coins_cur + settled.earned as f64),
        );
        stats[norm.clone()] = Value::Object(user.clone());
        let _ = state.store.write_document(&stats_file, &stats);
    }

    // touchUserPresence(email, active ? `Playing ${game}` : 'Browsing games')
    let playing = if active {
        format!("Playing {game}")
    } else {
        "Browsing games".to_string()
    };
    crate::ws::touch_user_presence(state, &email, &playing);

    let coins = mitch_lib::coins::get_coins(&state.store, state.data_dir(), &email);
    let daily_now =
        jsval::number(user.get("game_portal_reward_today").unwrap_or(&json!(0))).unwrap_or(0.0);
    json_resp(
        200,
        json!({
            "ok": true,
            "authenticated": true,
            "earned": settled.earned,
            "coins": jsval::num_value(coins),
            "dailyEarned": jsval::num_value(daily_now),
            "dailyCap": GAME_PORTAL_DAILY_CAP,
            "rewardPerMinute": GAME_PORTAL_REWARD_PER_MINUTE,
        }),
    )
}

/// `normalizeGamePortalTitle` (lib/game_portal_rewards.js:10-16): strip
/// `<`, `>` and control chars (U+0000-U+001F, U+007F), collapse JS `\s` runs
/// to a single space, trim, `slice(0, 60)` (UTF-16 units).
fn normalize_game_portal_title(value: &Value) -> String {
    fn is_js_ws(c: char) -> bool {
        matches!(
            c,
            '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
                ..='\u{200a}'
                    | '\u{2028}'
                    | '\u{2029}'
                    | '\u{202f}'
                    | '\u{205f}'
                    | '\u{3000}'
                    | '\u{feff}'
        )
    }
    let raw = jsval::string(value);
    let mut out = String::with_capacity(raw.len());
    let mut in_ws = false;
    for c in raw.chars() {
        if c == '<' || c == '>' || c == '\u{7f}' || (c as u32) <= 0x1f {
            continue;
        }
        if is_js_ws(c) {
            in_ws = true;
            continue;
        }
        if in_ws && !out.is_empty() {
            out.push(' ');
        }
        in_ws = false;
        out.push(c);
    }
    // JS trim() whitespace set == the JS \s set; use it (not Rust's) so chars
    // like NEL survive, as in JS.
    let collapsed = out.trim_matches(|c: char| is_js_ws(c)).to_string();
    jsval::js_slice_utf16(&collapsed, 60)
}

pub(crate) struct SettleResult {
    pub next: GamePortalSession,
    pub earned: i64,
    pub minutes: i64,
}

/// `settleGamePortalHeartbeat(previous, now, input)` — lib/game_portal_rewards.js:18-48.
pub(crate) fn settle_game_portal_heartbeat(
    previous: Option<&GamePortalSession>,
    now: i64,
    game: &str,
    active: bool,
    daily_earned: f64,
) -> SettleResult {
    let prev_accrued = previous.map(|p| p.accrued_ms).unwrap_or(0).max(0);
    let mut next = GamePortalSession {
        game: game.to_string(),
        active,
        last_seen: now,
        accrued_ms: if active { prev_accrued } else { 0 },
    };

    if !active {
        return SettleResult {
            next,
            earned: 0,
            minutes: 0,
        };
    }
    let continued = match previous {
        Some(p) => p.active && p.game == game,
        None => false,
    };
    if !continued {
        next.accrued_ms = 0;
        return SettleResult {
            next,
            earned: 0,
            minutes: 0,
        };
    }

    let delta = now - previous.map(|p| p.last_seen).unwrap_or(0);
    if !(HEARTBEAT_MIN_MS..=HEARTBEAT_MAX_MS).contains(&delta) {
        if delta > HEARTBEAT_MAX_MS {
            next.accrued_ms = 0;
        }
        return SettleResult {
            next,
            earned: 0,
            minutes: 0,
        };
    }

    next.accrued_ms += delta;
    let complete_minutes = (next.accrued_ms / 60_000) as f64;
    let available = (GAME_PORTAL_DAILY_CAP as f64 - daily_earned).max(0.0);
    let paid_minutes =
        complete_minutes.min((available / GAME_PORTAL_REWARD_PER_MINUTE as f64).floor());
    let earned = paid_minutes as i64 * GAME_PORTAL_REWARD_PER_MINUTE;
    next.accrued_ms = if available > 0.0 {
        next.accrued_ms - (paid_minutes as i64) * 60_000
    } else {
        0
    };
    SettleResult {
        next,
        earned,
        minutes: paid_minutes as i64,
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sess(game: &str, active: bool, last_seen: i64, accrued_ms: i64) -> GamePortalSession {
        GamePortalSession {
            game: game.to_string(),
            active,
            last_seen,
            accrued_ms,
        }
    }

    #[test]
    fn first_heartbeat_pays_nothing() {
        let r = settle_game_portal_heartbeat(None, 10_000, "Chess", true, 0.0);
        assert_eq!(r.earned, 0);
        assert_eq!(r.minutes, 0);
        assert_eq!(r.next.accrued_ms, 0);
        assert!(r.next.active);
        assert_eq!(r.next.game, "Chess");
    }

    #[test]
    fn inactive_resets_accrual() {
        let prev = sess("Chess", true, 5_000, 90_000);
        let r = settle_game_portal_heartbeat(Some(&prev), 10_000, "Chess", false, 0.0);
        assert_eq!(r.earned, 0);
        assert_eq!(r.next.accrued_ms, 0);
        assert!(!r.next.active);
    }

    #[test]
    fn game_switch_resets_accrual() {
        let prev = sess("Chess", true, 5_000, 90_000);
        let r = settle_game_portal_heartbeat(Some(&prev), 10_000, "Kody's Keyboard", true, 0.0);
        assert_eq!(r.earned, 0);
        assert_eq!(r.next.accrued_ms, 0);
    }

    #[test]
    fn too_fast_pays_nothing_and_keeps_accrual() {
        let prev = sess("Chess", true, 9_000, 60_000);
        let r = settle_game_portal_heartbeat(Some(&prev), 10_000, "Chess", true, 0.0);
        assert_eq!(r.earned, 0);
        assert_eq!(r.next.accrued_ms, 60_000); // carries over, no reset
    }

    #[test]
    fn too_slow_pays_nothing_and_resets_accrual() {
        let prev = sess("Chess", true, 0, 60_000);
        let r = settle_game_portal_heartbeat(Some(&prev), 50_000, "Chess", true, 0.0);
        assert_eq!(r.earned, 0);
        assert_eq!(r.next.accrued_ms, 0);
    }

    #[test]
    fn pays_two_coins_per_complete_minute() {
        // 9 previous accrual seconds + 11s delta = 20s → 0 complete minutes.
        let prev = sess("Chess", true, 0, 9_000);
        let r = settle_game_portal_heartbeat(Some(&prev), 11_000, "Chess", true, 0.0);
        assert_eq!(r.earned, 0);
        assert_eq!(r.next.accrued_ms, 20_000);

        // +45s delta (max allowed) → 65s total → 1 complete minute → 2 coins,
        // 5s carried.
        let prev = sess("Chess", true, 0, 20_000);
        let r = settle_game_portal_heartbeat(Some(&prev), 45_000, "Chess", true, 0.0);
        assert_eq!(r.earned, 2);
        assert_eq!(r.minutes, 1);
        assert_eq!(r.next.accrued_ms, 5_000);
    }

    #[test]
    fn daily_cap_limits_payout() {
        // dailyEarned 238 → available 2 → floor(2/2)=1 paid minute. 30s
        // previous accrual + 30s delta = 60s = 1 complete minute, fully paid.
        let prev = sess("Chess", true, 0, 30_000);
        let r = settle_game_portal_heartbeat(Some(&prev), 30_000, "Chess", true, 238.0);
        assert_eq!(r.earned, 2);
        assert_eq!(r.minutes, 1);
        assert_eq!(r.next.accrued_ms, 0);

        // Fully capped → nothing at all (accrued also zeroed).
        let prev = sess("Chess", true, 0, 30_000);
        let r = settle_game_portal_heartbeat(Some(&prev), 30_000, "Chess", true, 240.0);
        assert_eq!(r.earned, 0);
        assert_eq!(r.next.accrued_ms, 0);
    }

    #[test]
    fn title_normalize_strips_controls_and_clamps() {
        assert_eq!(
            normalize_game_portal_title(&json!("  a\x00b<>c\u{7f}  d ")),
            "abc d"
        );
        let long = "x".repeat(70);
        assert_eq!(normalize_game_portal_title(&json!(long)), "x".repeat(60));
        assert_eq!(normalize_game_portal_title(&json!("")), "");
        // JS \s collapse: nbsp is whitespace.
        assert_eq!(normalize_game_portal_title(&json!("a\u{a0}b")), "a b");
        // UTF-16 slice: an emoji counts as 2 units.
        assert_eq!(
            normalize_game_portal_title(&json!("😀".repeat(31))),
            "😀".repeat(30)
        );
    }

    #[test]
    fn game_portal_status_unauthorized_without_cookie() {
        let dir = std::env::temp_dir().join(format!("mitch-test-portal-status-{}", now_millis()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(mitch_lib::data::DataStore::open(&dir, &dir).unwrap());
        let mut cfg = crate::hosts::SiteConfig::load();
        cfg.base_dir = dir.clone();
        cfg.data_dir = dir.clone();
        let state = Arc::new(AppState::new(cfg, store));
        let headers = HeaderMap::new();
        let resp = handle(&state, &Method::GET, "/api/game-portal/status", &headers, &json!({}), &[]);
        assert!(resp.is_some());
        let resp = resp.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
    }
}
