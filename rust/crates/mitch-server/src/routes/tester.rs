//! `/api/tester/*` — backend for the Beta Tester Hub (`/tester/`).
//!
//! Every endpoint requires a session whose account is on the explicit tester
//! list (`data/testers.json`, managed from the admin panel). Admins are testers
//! for staff-hub access but have no sandbox unless they are listed too, so they
//! get a 403 here rather than silently replacing their real balance.
//!
//! The settings these endpoints write live in `tester_settings` and are honored
//! by `mitch_lib::tester` (coins), `auth::is_premium_email`,
//! `shop::sanitize_cosmetics_for_email` and the daily-login claim. See
//! `docs/tester-hub.md` for the economy rules.
//!
//! | Endpoint | Body | Effect |
//! | --- | --- | --- |
//! | `GET  /api/tester/status` | — | current sandbox state |
//! | `POST /api/tester/toggle-premium` | `{premium}` | force premium on/off |
//! | `POST /api/tester/coins` | `{mode:"unlimited"}` or `{mode:"custom", amount}` | sandbox balance |
//! | `POST /api/tester/settings` | `{unlockAllCosmetics?, bypassCooldowns?}` | feature toggles |
//! | `POST /api/tester/reset-cooldowns` | — | makes today's daily claim available again |
//! | `POST /api/tester/test-notification` | `{message}` | sends the caller an in-app notification |

use crate::routes::me::{cookies_of, json_response, me_uid, parse_body_strict};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::tester::{self, TesterSettings};
use mitch_lib::{auth, coins, daily_logins};
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Option<Response> {
    let route = path.strip_prefix("/api/tester/")?;
    let expected = match route {
        "status" => Method::GET,
        "toggle-premium" | "coins" | "settings" | "reset-cooldowns" | "test-notification" => {
            Method::POST
        }
        _ => return None,
    };
    if *method != expected {
        return Some(json_response(
            405,
            json!({ "error": "Method not allowed." }),
        ));
    }
    let email = match tester_email(state, headers) {
        Ok(email) => email,
        Err(resp) => return Some(*resp),
    };
    let mut cfg = tester::ensure(&state.store, &email);

    if route == "status" {
        return Some(json_response(200, status_json(&email, &cfg)));
    }
    if route == "reset-cooldowns" {
        return Some(reset_cooldowns(state, &email));
    }
    let Some(body) = parse_body_strict(body_bytes) else {
        return Some(json_response(400, json!({ "error": "Bad JSON" })));
    };

    match route {
        "toggle-premium" => {
            let Some(next) = body.get("premium").and_then(Value::as_bool) else {
                return Some(json_response(
                    400,
                    json!({ "error": "premium must be true or false" }),
                ));
            };
            cfg.premium = next;
            tester::save(&state.store, &email, &cfg);
            Some(json_response(
                200,
                json!({
                    "ok": true,
                    "isPremium": next,
                    "message": if next { "Premium enabled" } else { "Premium disabled" },
                }),
            ))
        }
        "coins" => Some(set_coins(state, &email, cfg, &body)),
        "settings" => {
            if let Some(v) = body.get("unlockAllCosmetics").and_then(Value::as_bool) {
                cfg.unlock_all_cosmetics = v;
            }
            if let Some(v) = body.get("bypassCooldowns").and_then(Value::as_bool) {
                cfg.bypass_cooldowns = v;
            }
            tester::save(&state.store, &email, &cfg);
            Some(json_response(
                200,
                json!({ "ok": true, "settings": settings_json(&cfg) }),
            ))
        }
        "test-notification" => Some(test_notification(state, &email, &body)),
        _ => None,
    }
}

/// The caller's email if they hold a session and are on the tester list.
fn tester_email(state: &Arc<AppState>, headers: &HeaderMap) -> Result<String, Box<Response>> {
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    if !auth::valid_id(&sid, &state.id_secret) {
        return Err(Box::new(json_response(
            401,
            json!({ "error": "Sign in required." }),
        )));
    }
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &sid) else {
        return Err(Box::new(json_response(
            401,
            json!({ "error": "Sign in required." }),
        )));
    };
    if !tester::is_listed(&state.store, &email) {
        return Err(Box::new(json_response(
            403,
            json!({ "error": "Your account is not on the beta tester list." }),
        )));
    }
    Ok(auth::normalize_email(&email))
}

fn settings_json(cfg: &TesterSettings) -> Value {
    json!({
        "premium": cfg.premium,
        "unlimitedCoins": cfg.unlimited_coins,
        "customCoins": if cfg.unlimited_coins { Value::Null } else { json!(cfg.custom_coins) },
        "unlockAllCosmetics": cfg.unlock_all_cosmetics,
        "bypassCooldowns": cfg.bypass_cooldowns,
    })
}

fn status_json(email: &str, cfg: &TesterSettings) -> Value {
    json!({
        "email": email,
        "isTester": true,
        "isPremium": cfg.premium,
        "unlimitedCoins": cfg.unlimited_coins,
        "coins": if cfg.unlimited_coins { tester::UNLIMITED_BALANCE } else { cfg.custom_coins },
        "settings": settings_json(cfg),
    })
}

fn set_coins(
    state: &Arc<AppState>,
    email: &str,
    mut cfg: TesterSettings,
    body: &Value,
) -> Response {
    match body.get("mode").and_then(Value::as_str) {
        Some("unlimited") => cfg.unlimited_coins = true,
        Some("custom") => {
            let amount = body.get("amount").and_then(Value::as_f64);
            let Some(amount) = amount.filter(|a| a.is_finite() && *a >= 0.0) else {
                return json_response(
                    400,
                    json!({ "error": "amount must be a number, 0 or more" }),
                );
            };
            cfg.unlimited_coins = false;
            cfg.custom_coins = amount.min(tester::MAX_CUSTOM_COINS);
        }
        _ => {
            return json_response(
                400,
                json!({ "error": "mode must be \"unlimited\" or \"custom\"" }),
            )
        }
    }
    tester::save(&state.store, email, &cfg);
    json_response(
        200,
        json!({
            "ok": true,
            "unlimitedCoins": cfg.unlimited_coins,
            "coins": if cfg.unlimited_coins { tester::UNLIMITED_BALANCE } else { cfg.custom_coins },
        }),
    )
}

/// Moves the last claim date back to yesterday (keeping the streak) so the
/// daily reward can be claimed again today. No-op if they never claimed.
fn reset_cooldowns(state: &Arc<AppState>, email: &str) -> Response {
    let yesterday = (chrono::Utc::now() - chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    match daily_logins::daily_login_get(&state.store, email) {
        Some(mut row) => {
            row["lastClaimDate"] = json!(yesterday);
            daily_logins::daily_login_set(&state.store, email, &row);
            json_response(
                200,
                json!({ "ok": true, "message": "Daily claim reset. You can claim again now." }),
            )
        }
        None => json_response(
            200,
            json!({ "ok": true, "message": "Nothing to reset yet; you haven't claimed a daily reward." }),
        ),
    }
}

/// Sends the tester an in-app notification and asks open pages to refresh.
fn test_notification(state: &Arc<AppState>, email: &str, body: &Value) -> Response {
    let message: String = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .chars()
        .take(200)
        .collect();
    if message.is_empty() {
        return json_response(400, json!({ "error": "message is required" }));
    }
    coins::add_admin_notification(
        &state.store,
        &state.cfg.data_dir,
        email,
        "Test notification",
        &message,
        "tester hub",
        "",
        "/tester/",
    );
    crate::ws::broadcast(
        state,
        crate::ws::WsRecipients::All,
        json!({ "type": "refresh_notifications" }).to_string(),
    );
    json_response(200, json!({ "ok": true }))
}
