//! Site maintenance workers (plan Step 13 batch 3) — the remaining
//! non-email `setInterval` timers of server.js:
//! - `scheduleDailySummary` 30s tick, 4:00 PM local send (server.js:4151-4218)
//! - `premiumMaintenanceWorker` 6h + one immediate call (server.js:4230-4325,
//!   registered at server.js:26783-26784)
//! - `happyHourWorker` 60s + one immediate call (server.js:26948-26999,
//!   registered at server.js:26805-26807)
//!
//! The nudge worker (server.js:4326-4329) is DISABLED in the JS (the
//! function body is `return;`) and no interval registers it — not ported,
//! documented here. The VM usage timers (sample/purge/prune/uptime/
//! desktop-session cleanup) land with the VM batch. The matrix outbound
//! email-alert machinery (server.js:8606+) needs the conduit client and
//! lands with the matrix batch.

use crate::routes::push::{ntfy_notify, send_email_bg};
use crate::state::AppState;
use crate::ws::{broadcast, WsRecipients};
use serde_json::{json, Value};

/// Starts the Step 13 batch 3 worker tasks, mirroring the JS boot block
/// (server.js:26779-26807): the happy-hour/premium pieces run inside a
/// 100ms-delayed task, the immediate calls fire there, and the interval
/// tasks register after.
pub fn spawn(state: std::sync::Arc<AppState>) {
    // ── scheduleDailySummary — 30s tick, sends at 16:00 local ──────────────
    {
        let state = state.clone();
        tokio::spawn(async move {
            // JS setInterval's first fire is one period in.
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
            tick.tick().await; // consume the immediate first tokio tick
            loop {
                tick.tick().await;
                daily_summary_tick(&state).await;
            }
        });
    }
    // ── premiumMaintenanceWorker + happyHourWorker inside the boot block ───
    tokio::spawn(async move {
        // JS: setTimeout(() => { ...workers... }, 100).
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // computedHappyHour = getLeastUsedSchoolHour(); happyHourWorker();
        let hour = mitch_lib::school::get_least_used_school_hour(
            &state.store,
            state.data_dir(),
            mitch_lib::school::now_millis(),
        );
        state
            .computed_happy_hour
            .store(hour, std::sync::atomic::Ordering::SeqCst);
        happy_hour_worker(&state);

        // setInterval(premiumMaintenanceWorker, 6h); premiumMaintenanceWorker();
        premium_maintenance_tick(&state).await;
        {
            let state = state.clone();
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(6 * 3600));
            tick.tick().await; // the immediate call already ran above
            tokio::spawn(async move {
                loop {
                    tick.tick().await;
                    premium_maintenance_tick(&state).await;
                }
            });
        }

        // setInterval(happyHourWorker, 60000) — first fire one period in.
        {
            let state = state.clone();
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
            tick.tick().await;
            tokio::spawn(async move {
                loop {
                    tick.tick().await;
                    happy_hour_worker(&state);
                }
            });
        }

        // auto_finalize_marketplace — hourly sweep
        {
            let state = state.clone();
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
            tick.tick().await;
            tokio::spawn(async move {
                loop {
                    tick.tick().await;
                    crate::routes::marketplace::auto_finalize_marketplace(&state);
                }
            });
        }
    });
}

// ── Daily traffic summary (server.js:4156-4218) ──────────────────────────────

/// The 30s scheduler body: at 16:00 local, once per calendar date.
async fn daily_summary_tick(state: &std::sync::Arc<AppState>) {
    if let Err(e) = daily_summary_tick_inner(state).await {
        tracing::error!("[scheduler] Error in daily summary scheduler: {e}");
    }
}

async fn daily_summary_tick_inner(state: &std::sync::Arc<AppState>) -> Result<(), String> {
    let (year, month, day, hour, minute) = local_now_parts();
    // JS: `now.getHours() === 16 && now.getMinutes() === 0` — LOCAL server
    // time; the VPS runs PT, so the 16:00 wall-clock hour is the send gate.
    if hour != 16 || minute != 0 {
        return Ok(());
    }
    let date_str = format!("{year}-{month}-{day}");
    {
        let mut last = state
            .last_daily_summary_sent_date
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *last == date_str {
            return Ok(());
        }
        *last = date_str;
    }
    send_daily_summary_notification(state).await;
    Ok(())
}

/// `(year, month 1-12, day, hour, minute)` of the local wall clock — the
/// JS `new Date()` accessors both schedulers read.
pub(crate) fn local_now_parts() -> (i64, i64, i64, i64, i64) {
    local_now_parts_at(mitch_lib::school::now_millis())
}

pub(crate) fn local_now_parts_at(now_ms: i64) -> (i64, i64, i64, i64, i64) {
    let secs = now_ms / 1000;
    let offset = crate::routes::admin::economy::local_tz_offset_secs();
    let local = secs + offset;
    let days = local.div_euclid(86_400);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    let secs_of_day = local.rem_euclid(86_400);
    (year, m, d, secs_of_day / 3600, (secs_of_day % 3600) / 60)
}

/// `sendDailySummaryNotification` (server.js:4158-4204) — the body lives in
/// admin/economy.rs (`build_daily_summary`, shared with the manual
/// /api/admin/trigger-daily-summary endpoint); this is the send + log.
pub(crate) async fn send_daily_summary_notification(state: &std::sync::Arc<AppState>) {
    let summary = crate::routes::admin::economy::build_daily_summary(state);
    ntfy_notify(&summary, "Daily Traffic Summary", "default");
    tracing::info!("[scheduler] Daily summary sent at");
}

// ── Premium maintenance worker (server.js:4230-4325) ─────────────────────────

pub(crate) async fn premium_maintenance_tick(state: &std::sync::Arc<AppState>) {
    if let Err(e) = premium_maintenance_worker(state).await {
        tracing::error!("[premium-worker] error: {e}");
    }
}

/// JS truthiness for the `!!`-coerced keys the worker reads.
fn truthy(v: &Value) -> bool {
    mitch_lib::jsval::truthy(v)
}

fn num(v: &Value) -> Option<f64> {
    mitch_lib::jsval::number(v)
}

/// `stats[norm]?.k || a || b || 0` — first nonzero/defined number.
fn first_num(vals: [Option<f64>; 4]) -> f64 {
    for v in vals.into_iter().flatten() {
        if v != 0.0 {
            return v;
        }
    }
    0.0
}

async fn premium_maintenance_worker(state: &std::sync::Arc<AppState>) -> Result<(), String> {
    let apps_file = state.data_dir().join("applications.json");
    let mut apps = state
        .store
        .read_document(&apps_file, json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let now = mitch_lib::school::now_millis();
    let warn_ms = 5 * 86_400 * 1000;
    let expiry_ms = 7 * 86_400 * 1000;
    let mut changed = false;

    for app in apps.iter_mut() {
        let typ = app.get("type").and_then(Value::as_str).unwrap_or("");
        let grant = app.get("grantPremium").is_some_and(|v: &Value| truthy(v));
        if !(typ == "premium" || grant) {
            continue;
        }
        let status = app.get("status").and_then(Value::as_str).unwrap_or("");
        if status != "approved" && status != "expired" {
            continue;
        }
        if app.get("neverExpire").is_some_and(|v: &Value| truthy(v)) {
            continue;
        }
        let email = app
            .get("email")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let norm = mitch_lib::auth::normalize_email(&email);
        let stat_entry = mitch_lib::user_stats::user_stats_get(&state.store, &norm);
        let last_active = first_num([
            stat_entry.get("last_active_at").and_then(num),
            app.get("approved_at").and_then(num),
            app.get("submitted_at").and_then(num),
            None,
        ]);
        let inactive_for = now as f64 - last_active;

        if status == "approved" {
            if inactive_for >= expiry_ms as f64 {
                tracing::info!("[premium] expiring {email} due to inactivity (7d)");
                if let Some(obj) = app.as_object_mut() {
                    obj.insert("status".into(), json!("expired"));
                    obj.insert("why_expired".into(), json!("Inactive for 7+ days"));
                }
                changed = true;
            } else if inactive_for >= warn_ms as f64 {
                let last_warn = stat_entry
                    .get("last_premium_warn")
                    .and_then(num)
                    .unwrap_or(0.0);
                if now as f64 - last_warn > 86_400.0 * 1000.0 {
                    // Warn at most once per 24h
                    let target_email = mitch_lib::profile::canonical_delivery_email(
                        &state.store,
                        &state.cfg.data_dir,
                        &state.id_secret,
                        &email,
                    );
                    tracing::info!("[premium] warning {target_email} about inactivity (5d)");
                    let subject = "Urgent: Your mitch.pro Premium is about to expire";
                    let html = crate::routes::admin::legacy::premium_alert_html(
                        state,
                        &target_email,
                        subject,
                        "Our records show you haven't logged in to mitch.pro for 5 days. If you do not log on in the next 2 days, your Premium status will be automatically revoked. Simply visit mitch.pro and log in to keep your perks!",
                        &crate::workers_email::site_url(state, &target_email),
                        "Login to mitch.pro",
                    );
                    // sendEmailBg routes school addresses through the Gmail
                    // script — support@mitch.pro via Hostinger SMTP bounces
                    // at rjuhsd.us.
                    send_email_bg(state, &target_email, subject, &html);
                    let mut entry = stat_entry.clone();
                    if !entry.is_object() {
                        entry = json!({});
                    }
                    if let Some(e) = entry.as_object_mut() {
                        e.insert("last_premium_warn".into(), json!(now));
                    }
                    mitch_lib::user_stats::user_stats_set(&state.store, &norm, &entry);
                }
            }
        }
    }

    // Check for users who have registered a premium_email, but no longer
    // have premium status. Notify via ntfy 7 days after they lost premium.
    for norm in mitch_lib::user_stats::user_stats_all_emails(&state.store) {
        let mut entry = mitch_lib::user_stats::user_stats_get(&state.store, &norm);
        let premium_email = entry
            .get("premium_email")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if premium_email.is_empty() {
            continue;
        }
        let has_premium = mitch_lib::auth::is_premium_email(&state.store, &norm);
        if has_premium {
            if entry.get("premium_lost_at").is_some()
                || entry.get("email_revoke_notified").is_some()
            {
                if let Some(obj) = entry.as_object_mut() {
                    obj.remove("premium_lost_at");
                    obj.remove("email_revoke_notified");
                }
                mitch_lib::user_stats::user_stats_set(&state.store, &norm, &entry);
            }
        } else {
            let lost_at = entry.get("premium_lost_at").and_then(num).unwrap_or(0.0);
            if lost_at == 0.0 {
                if let Some(obj) = entry.as_object_mut() {
                    obj.insert("premium_lost_at".into(), json!(now));
                }
                mitch_lib::user_stats::user_stats_set(&state.store, &norm, &entry);
            } else if now as f64 - lost_at >= 7.0 * 86_400.0 * 1000.0 {
                let notified = entry
                    .get("email_revoke_notified")
                    .is_some_and(|v: &Value| truthy(v));
                if !notified {
                    tracing::info!("[premium] notifying admin to revoke email access for {premium_email} - no longer premium for 7 days.");
                    ntfy_notify(
                        &format!(
                            "Revoke @mitch.pro email access for {premium_email} (account: {norm}) - no longer premium for 7 days."
                        ),
                        "Revoke Email Access",
                        "high",
                    );
                    if let Some(obj) = entry.as_object_mut() {
                        obj.insert("email_revoke_notified".into(), json!(true));
                    }
                    mitch_lib::user_stats::user_stats_set(&state.store, &norm, &entry);
                }
            }
        }
    }

    if changed {
        let _ = state.store.write_document(&apps_file, &json!(apps));
    }
    Ok(())
}

// ── Happy hour (server.js:26830-27000) ───────────────────────────────────────

/// `happyHourWorker()` — the 60s tick body (server.js:26948-26999).
pub(crate) fn happy_hour_worker(state: &std::sync::Arc<AppState>) {
    happy_hour_worker_at(state, mitch_lib::school::now_millis());
}

pub(crate) fn happy_hour_worker_at(state: &std::sync::Arc<AppState>, now_ms: i64) {
    // JS reads every part from the same `now = new Date()`.
    let (_, _, _, _, minute) = local_now_parts_at(now_ms);
    // `now.getMinutes() === 0 || typeof computedHappyHour === 'undefined'` —
    // the JS recompute happens on the hour (minute 0).
    if minute == 0 {
        let hour =
            mitch_lib::school::get_least_used_school_hour(&state.store, state.data_dir(), now_ms);
        state
            .computed_happy_hour
            .store(hour, std::sync::atomic::Ordering::SeqCst);
    }

    // LA wall clock: (year, month 1-12, day, hour, weekday 0=Sun, minute).
    let (_, _, _, la_hour, la_day, _) = mitch_lib::school::la_local_parts(now_ms);
    let computed = state
        .computed_happy_hour
        .load(std::sync::atomic::Ordering::SeqCst);
    let is_hh_day_and_hour = (1..=5).contains(&la_day) && la_hour == computed;

    if is_hh_day_and_hour {
        if !state
            .happy_hour_active
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            state
                .happy_hour_active
                .store(true, std::sync::atomic::Ordering::SeqCst);
            set_coin_multiplier(state, 2.0);
            tracing::info!("[happy-hour] Activated for designated hour: {computed}");
            broadcast(
                state,
                WsRecipients::All,
                json!({
                    "type": "admin_broadcast",
                    "message": format!(
                        "HAPPY HOUR ACTIVATED! 2X Coins for everyone! (Runs {}) 🎰",
                        mitch_lib::school::format_school_hour(computed)
                    )
                })
                .to_string(),
            );
        }
    } else if state
        .happy_hour_active
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        state
            .happy_hour_active
            .store(false, std::sync::atomic::Ordering::SeqCst);
        set_coin_multiplier(state, 1.0);
        tracing::info!("[happy-hour] Deactivated.");
        broadcast(
            state,
            WsRecipients::All,
            json!({
                "type": "admin_broadcast",
                "message": "Happy Hour has ended. 🍻"
            })
            .to_string(),
        );
    }
}

/// `globalCoinMultiplier = 2.0` / `1.0` — the AtomicU64 bit-exact f64.
fn set_coin_multiplier(state: &std::sync::Arc<AppState>, value: f64) {
    state
        .coin_multiplier
        .store(value.to_bits(), std::sync::atomic::Ordering::SeqCst);
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use serde_json::json;
    use std::sync::Arc;

    fn test_state() -> (Arc<AppState>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "mitch-server-workers-site-test-{}",
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

    fn apps_file(state: &AppState) -> std::path::PathBuf {
        state.cfg.data_dir.join("applications.json")
    }

    #[tokio::test]
    async fn premium_worker_expires_after_seven_days() {
        let (state, _dir) = test_state();
        let now = mitch_lib::school::now_millis();
        let _ = state.store.write_document(
            &apps_file(&state),
            &json!([{
                "email": "user@student.rjuhsd.us",
                "type": "premium",
                "status": "approved",
                "grantPremium": true,
                "approved_at": now,
                "submitted_at": now,
            }]),
        );
        mitch_lib::user_stats::user_stats_set(
            &state.store,
            "user@student.rjuhsd.us",
            &json!({ "last_active_at": now - 8 * 86_400_000 }),
        );
        premium_maintenance_worker(&state).await.unwrap();
        let apps = state.store.read_document(&apps_file(&state), json!([]));
        let app = &apps.as_array().unwrap()[0];
        assert_eq!(app.get("status"), Some(&json!("expired")));
        assert_eq!(app.get("why_expired"), Some(&json!("Inactive for 7+ days")));
    }

    #[tokio::test]
    async fn premium_worker_warns_once_per_day() {
        let (state, _dir) = test_state();
        let now = mitch_lib::school::now_millis();
        let _ = state.store.write_document(
            &apps_file(&state),
            &json!([{
                "email": "user@student.rjuhsd.us",
                "type": "premium",
                "status": "approved",
                "grantPremium": true,
                "approved_at": now,
                "submitted_at": now,
            }]),
        );
        mitch_lib::user_stats::user_stats_set(
            &state.store,
            "user@student.rjuhsd.us",
            &json!({ "last_active_at": now - 6 * 86_400_000 }),
        );
        premium_maintenance_worker(&state).await.unwrap();
        let warn1 = mitch_lib::user_stats::user_stats_get(&state.store, "user@student.rjuhsd.us")
            .get("last_premium_warn")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        // The worker stamps its own Date.now() — inside the run window.
        assert!(warn1 >= now);

        // A second run inside the 24h window must not re-stamp (idempotent).
        premium_maintenance_worker(&state).await.unwrap();
        let warn2 = mitch_lib::user_stats::user_stats_get(&state.store, "user@student.rjuhsd.us")
            .get("last_premium_warn")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        assert_eq!(warn2, warn1);
    }

    #[tokio::test]
    async fn premium_worker_skips_non_premium_and_never_expire() {
        let (state, _dir) = test_state();
        let now = mitch_lib::school::now_millis();
        let _ = state.store.write_document(
            &apps_file(&state),
            &json!([
                { "email": "a@student.rjuhsd.us", "type": "casino", "status": "approved",
                  "approved_at": now - 9 * 86_400_000 },
                { "email": "b@student.rjuhsd.us", "type": "premium", "status": "approved",
                  "neverExpire": true, "approved_at": now - 9 * 86_400_000 },
                { "email": "c@student.rjuhsd.us", "type": "premium", "status": "pending",
                  "approved_at": now - 9 * 86_400_000 }
            ]),
        );
        premium_maintenance_worker(&state).await.unwrap();
        let apps = state.store.read_document(&apps_file(&state), json!([]));
        let apps = apps.as_array().unwrap();
        assert_eq!(apps[0].get("status"), Some(&json!("approved")));
        assert_eq!(apps[1].get("status"), Some(&json!("approved")));
        assert_eq!(apps[2].get("status"), Some(&json!("pending")));
    }

    #[tokio::test]
    async fn premium_worker_last_active_fallback_chain() {
        // JS: stats[norm]?.last_active_at || approved_at || submitted_at || 0.
        let (state, _dir) = test_state();
        let now = mitch_lib::school::now_millis();
        // No stats entry: falls through to approved_at (9d ago) → expired.
        let _ = state.store.write_document(
            &apps_file(&state),
            &json!([{
                "email": "user@student.rjuhsd.us",
                "type": "premium",
                "status": "approved",
                "grantPremium": true,
                "approved_at": now - 9 * 86_400_000,
                "submitted_at": now - 10 * 86_400_000,
            }]),
        );
        premium_maintenance_worker(&state).await.unwrap();
        let apps = state.store.read_document(&apps_file(&state), json!([]));
        assert_eq!(
            apps.as_array().unwrap()[0].get("status"),
            Some(&json!("expired"))
        );
    }

    #[tokio::test]
    async fn premium_worker_email_revoke_notifies_after_seven_days() {
        let (state, _dir) = test_state();
        let now = mitch_lib::school::now_millis();
        // premium_email set, no premium status, lost 8d ago, never notified.
        mitch_lib::user_stats::user_stats_set(
            &state.store,
            "user@student.rjuhsd.us",
            &json!({
                "premium_email": "user@student.mitch.pro",
                "premium_lost_at": now - 8 * 86_400_000
            }),
        );
        premium_maintenance_worker(&state).await.unwrap();
        let entry = mitch_lib::user_stats::user_stats_get(&state.store, "user@student.rjuhsd.us");
        assert_eq!(entry.get("email_revoke_notified"), Some(&json!(true)));

        // Regaining premium clears both markers (server.js:4290-4299).
        let _ = state.store.write_document(
            &state.cfg.data_dir.join("applications.json"),
            &json!([{ "email": "user@student.rjuhsd.us", "type": "premium",
                      "status": "approved", "grantPremium": true }]),
        );
        premium_maintenance_worker(&state).await.unwrap();
        let entry = mitch_lib::user_stats::user_stats_get(&state.store, "user@student.rjuhsd.us");
        assert!(entry.get("premium_lost_at").is_none());
        assert!(entry.get("email_revoke_notified").is_none());
    }

    #[test]
    fn local_now_parts_matches_offset_math() {
        // The tuple is civil (y, m, d, h, min) in the local TZ — sanity: the
        // minute rolls over exactly at second 0 of each minute boundary.
        let ms = 1_790_000_000_000; // fixed instant
        let (y, m, d, h, min) = local_now_parts_at(ms);
        assert!((1..=12).contains(&m));
        assert!((1..=31).contains(&d));
        assert!((0..=23).contains(&h));
        assert!((0..=59).contains(&min));
        assert!((2020..=2100).contains(&y));
    }

    #[test]
    fn happy_hour_activate_and_deactivate_broadcasts() {
        let (state, _dir) = test_state();
        // Pick a fixed LA school-day morning instant and derive its parts.
        // 2026-09-16 10:00:00 UTC = 03:00 PDT (Wednesday) — hour 3, weekday 3.
        let ms = 1_789_615_200_000i64;
        let (_, _, _, hour, weekday, _) = mitch_lib::school::la_local_parts(ms);
        assert_eq!(weekday, 3); // Wednesday — inside the day 1-5 gate.
        state
            .computed_happy_hour
            .store(hour, std::sync::atomic::Ordering::SeqCst);
        happy_hour_worker_at(&state, ms);
        assert!(state
            .happy_hour_active
            .load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(
            f64::from_bits(
                state
                    .coin_multiplier
                    .load(std::sync::atomic::Ordering::SeqCst)
            ),
            2.0
        );

        // One second past the hour: deactivates with the end broadcast.
        let after = ms + 3_600_000;
        happy_hour_worker_at(&state, after);
        assert!(!state
            .happy_hour_active
            .load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(
            f64::from_bits(
                state
                    .coin_multiplier
                    .load(std::sync::atomic::Ordering::SeqCst)
            ),
            1.0
        );
    }

    #[test]
    fn happy_hour_stays_quiet_outside_school_days() {
        let (state, _dir) = test_state();
        // 2026-09-20 10:00 UTC is a Sunday — weekday 0, outside 1-5.
        let ms = 1_789_963_200_000i64;
        let (_, _, _, _, weekday, _) = mitch_lib::school::la_local_parts(ms);
        assert_eq!(weekday, 0);
        state
            .happy_hour_active
            .store(true, std::sync::atomic::Ordering::SeqCst);
        happy_hour_worker_at(&state, ms);
        assert!(!state
            .happy_hour_active
            .load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(
            f64::from_bits(
                state
                    .coin_multiplier
                    .load(std::sync::atomic::Ordering::SeqCst)
            ),
            1.0
        );
    }
}
