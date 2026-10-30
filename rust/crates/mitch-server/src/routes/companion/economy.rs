//! `/api/companion/subscribe` + `/api/companion/characters/unlock` — the
//! paid half of the companion feature. New subsystem, no `server.js`
//! equivalent. See the economy-integrity walk-through in
//! `mitch_lib::companion`'s module doc and `docs/companion.md`.

use crate::routes::me::{cookies_of, json_response, me_uid, parse_body_strict};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::auth;
use mitch_lib::jsval::str_or;
use serde_json::json;
use std::sync::Arc;

const WEEK_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const MONTH_MS: i64 = 30 * 24 * 60 * 60 * 1000;
const WEEK_COST: f64 = 750.0;
const MONTH_COST: f64 = 1500.0;

/// The creator's cut of every unlock — the explicit answer was "like
/// 25%"; the remaining 75% is a sink (not credited to anyone), which is
/// what keeps two colluding accounts from using this to print coins: the
/// buyer always loses more than the creator gains, every single time.
const CREATOR_PAYOUT_FRACTION: f64 = 0.25;

fn logged_in_email(state: &AppState, headers: &HeaderMap) -> Option<String> {
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    auth::email_from_sid(&state.store, &state.id_secret, &sid)
}

pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Option<Response> {
    if path == "/api/companion/subscribe" && *method == Method::POST {
        return Some(subscribe(state, headers, body_bytes));
    }
    if path == "/api/companion/characters/unlock" && *method == Method::POST {
        return Some(unlock(state, headers, body_bytes));
    }
    if path == "/api/companion/coin-request/accept" && *method == Method::POST {
        return Some(accept_coin_request(state, headers, body_bytes));
    }
    None
}

fn subscribe(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(email) = logged_in_email(state, headers) else {
        return json_response(401, json!({ "error": "not logged in" }));
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let (cost, duration_ms) = match str_or(body.get("plan"), "").as_str() {
        "week" => (WEEK_COST, WEEK_MS),
        "month" => (MONTH_COST, MONTH_MS),
        _ => {
            return json_response(
                400,
                json!({ "error": "plan must be \"week\" or \"month\"" }),
            )
        }
    };

    if let Err(e) = mitch_lib::coins::spend_coins(
        &state.store,
        state.data_dir(),
        &email,
        cost,
        "companion_subscribe",
    ) {
        return json_response(400, json!({ "error": format!("insufficient coins: {e}") }));
    }
    let expires_at = mitch_lib::companion::extend_subscription(&state.store, &email, duration_ms);
    json_response(200, json!({ "ok": true, "expiresAt": expires_at }))
}

fn unlock(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(email) = logged_in_email(state, headers) else {
        return json_response(401, json!({ "error": "not logged in" }));
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let character_id = str_or(body.get("characterId"), "");
    let Some(character) = mitch_lib::companion::get_character(&state.store, &character_id) else {
        return json_response(404, json!({ "error": "character not found" }));
    };
    if !mitch_lib::companion::visible_to(&character, &email) {
        return json_response(404, json!({ "error": "character not found" }));
    }
    if mitch_lib::companion::has_unlocked(&state.store, &email, &character) {
        return json_response(200, json!({ "ok": true, "alreadyUnlocked": true }));
    }

    let price = character.price_coins;
    if price > 0.0 {
        if let Err(e) = mitch_lib::coins::spend_coins(
            &state.store,
            state.data_dir(),
            &email,
            price,
            "companion_unlock_character",
        ) {
            return json_response(400, json!({ "error": format!("insufficient coins: {e}") }));
        }
    }
    mitch_lib::companion::unlock_character(&state.store, &email, &character_id);
    // User-made characters pay their creator a cut; admin presets have no
    // creator_email, so there's nobody to pay — the full price is a sink.
    if !character.creator_email.is_empty() {
        let payout = price * CREATOR_PAYOUT_FRACTION;
        if payout > 0.0 {
            mitch_lib::coins::add_coins(
                &state.store,
                state.data_dir(),
                &character.creator_email,
                payout,
                1.0,
                "companion_character_royalty",
            );
        }
    }
    json_response(200, json!({ "ok": true, "alreadyUnlocked": false }))
}

/// The only place a companion coin request ever actually moves coins.
/// Nothing in `deepseek.rs`/`routes/companion/mod.rs` deducts anything —
/// they only ever record that a percent was *offered*. This re-validates
/// against the live balance right now (never a stale snapshot from when
/// the offer was made) and against what `consume_coin_request` says was
/// actually offered (never trusting a percent the client sends up itself),
/// then applies a pure sink deduction — no creator payout, same reasoning
/// as the no-payout-on-the-other-75%-of-an-unlock sink above.
fn accept_coin_request(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(email) = logged_in_email(state, headers) else {
        return json_response(401, json!({ "error": "not logged in" }));
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let character_id = str_or(body.get("characterId"), "");
    let Some(character) = mitch_lib::companion::get_character(&state.store, &character_id) else {
        return json_response(404, json!({ "error": "character not found" }));
    };
    let offered_percent =
        mitch_lib::companion::consume_coin_request(&state.store, &email, &character_id);
    if offered_percent <= 0.0 {
        return json_response(
            409,
            json!({ "error": "no pending coin request from this character" }),
        );
    }
    let percent = offered_percent.clamp(0.0, 100.0);
    let balance = mitch_lib::coins::get_coins(&state.store, state.data_dir(), &email);
    let amount = balance * percent / 100.0;
    if amount > 0.0 {
        let _ = mitch_lib::coins::spend_coins(
            &state.store,
            state.data_dir(),
            &email,
            amount,
            "companion_coin_request",
        );
    }
    json_response(
        200,
        json!({ "ok": true, "deducted": amount, "characterName": character.name }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    #[test]
    fn subscribe_insufficient_coins_returns_error() {
        let dir = std::env::temp_dir().join(format!(
            "mitch-companion-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(mitch_lib::data::DataStore::open(&dir, &dir.join("data")).unwrap());
        let mut cfg = crate::hosts::SiteConfig::load();
        cfg.data_dir = dir.clone();
        let state = Arc::new(AppState::new(cfg, store));

        let sess = auth::create_auth_session(
            &state.store,
            &state.id_secret,
            "test@mitch.pro",
            "test@mitch.pro",
            "",
            "",
            false,
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            axum::http::HeaderValue::from_str(&format!(
                "mitch_session={}; studentId={}",
                sess.token, sess.sid
            ))
            .unwrap(),
        );

        let body = serde_json::to_vec(&json!({ "plan": "week" })).unwrap();
        let res = subscribe(&state, &headers, &body);
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            mitch_lib::coins::get_coins(&state.store, state.data_dir(), "test@mitch.pro"),
            0.0
        );
    }
}
