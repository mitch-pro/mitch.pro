//! `/api/companion/*` + `/api/admin/companion/*` — the desktop companion
//! character feature. New subsystem, no `server.js` equivalent: a small
//! anime-style character widget the user can enable site-wide, chat with
//! (DeepSeek-backed), and drag around the screen.
//!
//! Full scope now built, including the economy (see
//! `mitch_lib::companion`'s module doc for the exploit walk-through):
//! admin-authored presets, user-created custom characters with creator-set
//! pricing (`mine.rs`), a MitchCoins subscription gate plus per-character
//! unlock pricing (`economy.rs`), DeepSeek-backed chat with short-term memory,
//! and the admin review queue for user-generated prompts.
//!
//! See `docs/companion.md`.

mod admin;
mod deepseek;
mod economy;
mod mine;

use crate::routes::me::{cookies_of, json_response, me_uid, parse_body_strict};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::auth;
use mitch_lib::jsval::str_or;
use serde_json::json;
use std::sync::Arc;

/// Dispatches both `/api/companion/*` and `/api/admin/companion/*` (the
/// admin half needs the raw multipart body for image uploads, which the
/// JSON-only `routes::admin::handle` dispatcher doesn't carry — the global
/// `/api/admin/*` gate in handler.rs still protects it either way).
pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
    search: &str,
) -> Option<Response> {
    if path.starts_with("/api/admin/companion/") {
        return admin::handle(state, method, path, headers, body_bytes).await;
    }
    if path.starts_with("/api/companion/mine") {
        return mine::handle(state, method, path, headers, body_bytes).await;
    }
    if path == "/api/companion/subscribe"
        || path == "/api/companion/characters/unlock"
        || path == "/api/companion/coin-request/accept"
    {
        return economy::handle(state, method, path, headers, body_bytes).await;
    }
    if path == "/api/companion/characters" && *method == Method::GET {
        return Some(list_characters(state, headers));
    }
    if path == "/api/companion/settings" {
        return match *method {
            Method::GET => Some(get_settings(state, headers)),
            Method::POST => Some(set_settings(state, headers, body_bytes)),
            _ => None,
        };
    }
    if path == "/api/companion/chat" && *method == Method::POST {
        return Some(chat(state, headers, body_bytes).await);
    }
    if path == "/api/companion/character-image" && *method == Method::GET {
        let id = qs_get(search, "id").unwrap_or_default();
        let mood = qs_get(search, "mood").unwrap_or_default();
        return Some(admin::serve_image(state, headers, &id, &mood));
    }
    None
}

fn qs_get(search: &str, key: &str) -> Option<String> {
    url::form_urlencoded::parse(search.as_bytes())
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

fn logged_in_email(state: &AppState, headers: &HeaderMap) -> Option<String> {
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    auth::email_from_sid(&state.store, &state.id_secret, &sid)
}

/// `displayName`, falling back to `username`, falling back to the default
/// derived from the email — whichever is the first non-empty one.
fn display_name_for(state: &AppState, email: &str) -> String {
    let norm = auth::normalize_email(email);
    let profile = mitch_lib::profiles::profile_get_opt(&state.store, &norm);
    let from_profile = profile.as_ref().and_then(|p| {
        let display = p.get("displayName").and_then(|v| v.as_str()).unwrap_or("");
        if !display.is_empty() {
            return Some(display.to_string());
        }
        p.get("username")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    });
    from_profile.unwrap_or_else(|| mitch_lib::profile::default_username_for_email(email))
}

/// Public listing — personality prompts and creator emails stay
/// server-side; the widget only needs enough to render a picker.
/// `unlocked` is per-caller: whether *they* can select this character
/// right now without an extra purchase.
fn list_characters(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let Some(email) = logged_in_email(state, headers) else {
        return json_response(401, json!({ "error": "not logged in" }));
    };
    let chars = mitch_lib::companion::list_visible_characters(&state.store, &email);
    let out: Vec<_> = chars
        .into_iter()
        .map(|c| {
            let unlocked = mitch_lib::companion::has_unlocked(&state.store, &email, &c);
            json!({
                "id": c.id,
                "name": c.name,
                "isPreset": c.is_preset,
                "isDefault": c.is_default,
                "priceCoins": c.price_coins,
                "unlocked": unlocked,
                "moods": c.moods,
            })
        })
        .collect();
    json_response(200, json!({ "characters": out }))
}

fn get_settings(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let Some(email) = logged_in_email(state, headers) else {
        return json_response(401, json!({ "error": "not logged in" }));
    };
    let s = mitch_lib::companion::get_user_settings(&state.store, &email);
    let subscription_expires_at =
        mitch_lib::companion::subscription_expires_at(&state.store, &email);
    json_response(
        200,
        json!({
            "enabled": s.enabled,
            "activeCharacterId": s.active_character_id,
            "allowCoinRequests": true,
            "subscriptionActive": mitch_lib::companion::is_subscription_active(&state.store, &email),
            "subscriptionExpiresAt": subscription_expires_at,
        }),
    )
}

fn set_settings(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(email) = logged_in_email(state, headers) else {
        return json_response(401, json!({ "error": "not logged in" }));
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let enabled = body
        .get("enabled")
        .map(mitch_lib::jsval::truthy)
        .unwrap_or(false);
    let character_id = str_or(body.get("activeCharacterId"), "");
    if enabled {
        if !mitch_lib::companion::is_subscription_active(&state.store, &email) {
            return json_response(402, json!({ "error": "subscription required" }));
        }
        if character_id.is_empty() {
            return json_response(400, json!({ "error": "choose a character" }));
        }
        let Some(character) = mitch_lib::companion::get_character(&state.store, &character_id)
        else {
            return json_response(400, json!({ "error": "unknown character" }));
        };
        if !mitch_lib::companion::visible_to(&character, &email) {
            return json_response(404, json!({ "error": "unknown character" }));
        }
        if !mitch_lib::companion::has_unlocked(&state.store, &email, &character) {
            return json_response(402, json!({ "error": "character not unlocked" }));
        }
    }
    let character_id = if enabled { character_id } else { String::new() };
    mitch_lib::companion::set_user_settings(&state.store, &email, enabled, &character_id);
    mitch_lib::companion::set_allow_coin_requests(&state.store, &email, true);
    json_response(200, json!({ "ok": true }))
}

async fn chat(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(email) = logged_in_email(state, headers) else {
        return json_response(401, json!({ "error": "not logged in" }));
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let message = str_or(body.get("message"), "").trim().to_string();
    if message.is_empty() {
        return json_response(400, json!({ "error": "empty message" }));
    }
    // 2000 chars is generous for a chat bubble and keeps the DeepSeek call cheap.
    let message: String = message.chars().take(2000).collect();
    let page = str_or(body.get("page"), "");
    let page: String = page.chars().take(200).collect();

    let settings = mitch_lib::companion::get_user_settings(&state.store, &email);
    if !settings.enabled || settings.active_character_id.is_empty() {
        return json_response(403, json!({ "error": "companion not enabled" }));
    }
    // Defense in depth: `enabled` is a stored preference that isn't
    // re-validated continuously, so a lapsed subscription (or a price
    // change that revokes a never-actually-unlocked character) must still
    // be caught here, not just at the point `enabled` was first flipped on.
    if !mitch_lib::companion::is_subscription_active(&state.store, &email) {
        return json_response(402, json!({ "error": "subscription expired" }));
    }
    let Some(character) =
        mitch_lib::companion::get_character(&state.store, &settings.active_character_id)
    else {
        return json_response(409, json!({ "error": "active character no longer exists" }));
    };
    if !mitch_lib::companion::has_unlocked(&state.store, &email, &character) {
        return json_response(402, json!({ "error": "character not unlocked" }));
    }
    let moods = if character.moods.is_empty() {
        vec!["idle".to_string()]
    } else {
        character.moods.clone()
    };
    let api_key = std::env::var("DEEPSEEK_API_KEY").unwrap_or_default();
    if api_key.is_empty() {
        return json_response(503, json!({ "error": "companion AI is not configured" }));
    }

    let balance = mitch_lib::coins::get_coins(&state.store, state.data_dir(), &email);
    let user = deepseek::UserContext {
        email: email.clone(),
        display_name: display_name_for(state, &email),
        mitchcoin_balance: balance,
        premium: auth::is_premium_email(&state.store, &email),
    };
    // Last 5 exchanges (10 messages) is enough for the character to "keep
    // some history" without the prompt growing without bound.
    let history =
        mitch_lib::companion::recent_chat_messages(&state.store, &email, &character.id, 10);
    let memories = mitch_lib::companion::recent_memories(&state.store, &email, &character.id);

    // Both halves of "models aren't released until admins review": an
    // unreviewed character (including its own creator testing it) is never
    // even offered customJs/coin-request capability, not just hidden from
    // other users after the fact. No opt-in needed; the character can take
    // up to 100% of coins directly when cooldown has elapsed.
    let caps = deepseek::Capabilities {
        allow_custom_js: !character.needs_review,
        allow_coin_requests: !character.needs_review
            && mitch_lib::companion::coin_request_cooldown_elapsed(
                &state.store,
                &email,
                &character.id,
            ),
    };

    match deepseek::chat(
        &api_key,
        &character.name,
        &character.personality_prompt,
        &page,
        &message,
        &moods,
        &user,
        &history,
        &memories,
        &caps,
    )
    .await
    {
        Ok(r) => {
            mitch_lib::companion::add_chat_message(
                &state.store,
                &email,
                &character.id,
                "user",
                &message,
                "",
            );
            mitch_lib::companion::add_chat_message(
                &state.store,
                &email,
                &character.id,
                "model",
                &r.reply,
                &r.mood,
            );
            if !r.remember.is_empty() {
                mitch_lib::companion::add_memory(&state.store, &email, &character.id, &r.remember);
            }
            let mut coins_taken = 0.0;
            if r.coin_request_percent > 0.0 {
                let percent = r.coin_request_percent.clamp(0.0, 100.0);
                let balance = mitch_lib::coins::get_coins(&state.store, state.data_dir(), &email);
                let amount = (balance * percent / 100.0).min(balance.max(0.0));
                if amount > 0.0
                    && mitch_lib::coins::spend_coins(
                        &state.store,
                        state.data_dir(),
                        &email,
                        amount,
                        "companion_coin_take",
                    )
                    .is_ok()
                {
                    coins_taken = amount;
                }
                mitch_lib::companion::record_coin_request(
                    &state.store,
                    &email,
                    &character.id,
                    r.coin_request_percent,
                );
            }
            json_response(
                200,
                json!({
                    "reply": r.reply,
                    "mood": r.mood,
                    "action": r.action,
                    "customJs": r.custom_js,
                    "coinRequestPercent": r.coin_request_percent,
                    "coinsTaken": coins_taken,
                }),
            )
        }
        Err(_) => json_response(502, json!({ "error": "companion AI failed, try again" })),
    }
}
