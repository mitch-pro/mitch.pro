//! `GET /api/matrix/cosmetics?ids=@a:mitch.pro,@b:mitch.pro` — the equipped
//! shop cosmetics of the people shown in the chat client, so the chat page can
//! style names, badges and message rows (`webserver/matrix/cosmetics.js`).
//!
//! Needs a signed-in session (it is not a public directory). Only what the
//! account owns and has equipped is returned (`shop::public_loadout`), the same
//! items the profile page and leaderboard already show. The caller's own site
//! theme comes back as `me.theme`. At most 100 ids per request; ids that are
//! not `@name:mitch.pro` or that map to no account are left out.

use crate::routes::me::{cookies_of, json_response, me_uid};
use crate::state::AppState;
use axum::http::HeaderMap;
use axum::response::Response;
use mitch_lib::auth;
use serde_json::{json, Map, Value};
use std::collections::HashMap;

const MAX_IDS: usize = 100;

pub(crate) fn cosmetics(state: &AppState, headers: &HeaderMap, search: &str) -> Response {
    let uid = me_uid(&cookies_of(state, headers));
    if !auth::valid_id(&uid, &state.id_secret) {
        return json_response(401, json!({ "error": "Not signed in." }));
    }
    let Some(my_email) = auth::email_from_sid(&state.store, &state.id_secret, &uid) else {
        return json_response(401, json!({ "error": "Not signed in." }));
    };

    let ids: Vec<String> = url::form_urlencoded::parse(search.trim_start_matches('?').as_bytes())
        .find(|(k, _)| k == "ids")
        .map(|(_, v)| {
            v.split(',')
                .map(|id| id.trim().to_string())
                .filter(|id| !id.is_empty())
                .take(MAX_IDS)
                .collect()
        })
        .unwrap_or_default();

    // One read of the mapping for the whole batch: localpart -> account email.
    let mapping = state
        .store
        .read_document(&state.data_dir().join("matrix_users.json"), json!({}));
    let mut owners: HashMap<String, String> = HashMap::new();
    if let Some(map) = mapping.as_object() {
        for (key, value) in map {
            let Some(localpart) = value.as_str() else {
                continue;
            };
            let email = if key.contains('@') {
                Some(auth::normalize_email(key))
            } else {
                auth::names_get_email(&state.store, key).map(|e| auth::normalize_email(&e))
            };
            if let Some(email) = email {
                owners.insert(localpart.to_lowercase(), email);
            }
        }
    }

    let mut users = Map::new();
    for id in ids {
        let Some((localpart, domain)) = id.strip_prefix('@').and_then(|r| r.split_once(':')) else {
            continue;
        };
        if !domain.eq_ignore_ascii_case("mitch.pro") {
            continue;
        }
        let Some(email) = owners.get(&localpart.to_lowercase()) else {
            continue;
        };
        let loadout = mitch_lib::shop::public_loadout(&state.store, email);
        let empty = loadout
            .as_object()
            .is_some_and(|o| o.values().all(|v| v.as_str() == Some("")));
        if !empty {
            users.insert(id, loadout);
        }
    }

    let my_norm = auth::normalize_email(&my_email);
    let theme = mitch_lib::shop::cosmetics_get(&state.store, &my_norm)
        .and_then(|c| {
            c.get("activeTheme")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    json_response(200, json!({ "users": users, "me": { "theme": theme } }))
}
