//! `/api/companion/ability*` + `/api/companion/upgrades/buy`: per-user
//! special-ability state (Sandevistan strain, blood, cyberpsychosis) and
//! chrome upgrades. Data layer and the economy notes live in
//! `mitch_lib::companion_chrome`; see `docs/companion.md`.

use crate::routes::me::{cookies_of, json_response, me_uid, parse_body_strict};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::auth;
use mitch_lib::companion::Character;
use mitch_lib::companion_chrome::{self, AbilityState, BuyError};
use mitch_lib::jsval::str_or;
use serde_json::{json, Value};
use std::sync::Arc;

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
    search: &str,
) -> Option<Response> {
    let Some(email) = logged_in_email(state, headers) else {
        return Some(json_response(401, json!({ "error": "not logged in" })));
    };
    match (path, method) {
        ("/api/companion/ability", &Method::GET) => {
            let id = url::form_urlencoded::parse(search.as_bytes())
                .find(|(k, _)| k == "characterId")
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default();
            Some(get_state(state, &email, &id))
        }
        ("/api/companion/ability/use", &Method::POST) => Some(use_ability(state, &email)),
        ("/api/companion/upgrades/buy", &Method::POST) => Some(buy(state, &email, body_bytes)),
        _ => None,
    }
}

/// The JSON shape every endpoint here returns for a character's state.
pub fn state_json(character: &Character, s: &AbilityState) -> Value {
    let catalog: Vec<_> = companion_chrome::UPGRADES
        .iter()
        .filter(|u| u.ability == character.special_ability)
        .map(|u| {
            json!({
                "id": u.id,
                "name": u.name,
                "description": u.description,
                "priceCoins": u.price_coins,
                "requires": u.requires,
                "owned": s.upgrades.iter().any(|o| o == u.id),
            })
        })
        .collect();
    json!({
        "characterId": character.id,
        "specialAbility": character.special_ability,
        "strain": s.strain,
        "maxStrain": companion_chrome::MAX_STRAIN,
        "bleeding": s.bleeding,
        "psychosisUntil": s.psychosis_until,
        "upgrades": s.upgrades,
        "catalog": catalog,
        "now": mitch_lib::school::now_millis(),
    })
}

/// A character the caller can see, has unlocked, and that has an ability.
#[allow(clippy::result_large_err)] // the Err path is an early-return Response, as in admin.rs
fn usable_character(state: &AppState, email: &str, id: &str) -> Result<Character, Response> {
    let Some(character) = mitch_lib::companion::get_character(&state.store, id) else {
        return Err(json_response(
            404,
            json!({ "error": "character not found" }),
        ));
    };
    if !mitch_lib::companion::visible_to(&character, email) {
        return Err(json_response(
            404,
            json!({ "error": "character not found" }),
        ));
    }
    if character.special_ability.is_empty() {
        return Err(json_response(
            400,
            json!({ "error": "this character has no special ability" }),
        ));
    }
    if !mitch_lib::companion::has_unlocked(&state.store, email, &character) {
        return Err(json_response(
            402,
            json!({ "error": "character not unlocked" }),
        ));
    }
    Ok(character)
}

fn get_state(state: &Arc<AppState>, email: &str, id: &str) -> Response {
    let id = if id.is_empty() {
        mitch_lib::companion::get_user_settings(&state.store, email).active_character_id
    } else {
        id.to_string()
    };
    let character = match usable_character(state, email, &id) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let s = companion_chrome::ability_state(&state.store, email, &character.id);
    json_response(200, state_json(&character, &s))
}

/// Double-click (or the Catch the Turkey button) on the active companion.
/// Same gates as chat: enabled, subscribed, unlocked.
fn use_ability(state: &Arc<AppState>, email: &str) -> Response {
    let settings = mitch_lib::companion::get_user_settings(&state.store, email);
    if !settings.enabled || settings.active_character_id.is_empty() {
        return json_response(403, json!({ "error": "companion not enabled" }));
    }
    if !mitch_lib::companion::is_subscription_active(&state.store, email) {
        return json_response(402, json!({ "error": "subscription expired" }));
    }
    let character = match usable_character(state, email, &settings.active_character_id) {
        Ok(c) => c,
        Err(r) => return r,
    };
    match companion_chrome::record_use(&state.store, email, &character.id) {
        Ok(s) => json_response(200, state_json(&character, &s)),
        Err(ms) => json_response(
            429,
            json!({ "error": "Sandevistan is recharging", "retryAfterMs": ms }),
        ),
    }
}

fn buy(state: &Arc<AppState>, email: &str, body_bytes: &[u8]) -> Response {
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let id = str_or(body.get("characterId"), "");
    let upgrade = str_or(body.get("upgrade"), "");
    let character = match usable_character(state, email, &id) {
        Ok(c) => c,
        Err(r) => return r,
    };
    match companion_chrome::buy_upgrade(
        &state.store,
        state.data_dir(),
        email,
        &character.id,
        &character.special_ability,
        &upgrade,
    ) {
        Ok(s) => json_response(200, state_json(&character, &s)),
        Err(BuyError::UnknownUpgrade) | Err(BuyError::WrongCharacter) => {
            json_response(400, json!({ "error": "unknown upgrade" }))
        }
        Err(BuyError::AlreadyOwned) => json_response(409, json!({ "error": "already installed" })),
        Err(BuyError::MissingRequirement(req)) => {
            let name = companion_chrome::upgrade(req)
                .map(|u| u.name)
                .unwrap_or(req);
            json_response(400, json!({ "error": format!("needs {name} first") }))
        }
        Err(BuyError::InsufficientFunds) => {
            json_response(400, json!({ "error": "not enough coins" }))
        }
    }
}
