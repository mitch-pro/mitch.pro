//! `/api/companion/mine*` — self-service custom character creation for
//! regular (non-admin) users. New subsystem, no `server.js` equivalent.
//!
//! Economy: creating a character costs a flat, non-refundable
//! `CREATION_FEE_COINS` (pure sink — nobody receives it). The creator can
//! set their own `price_coins` for other users to unlock it later (see
//! `economy.rs`), but always has free access to characters they made
//! themselves (`mitch_lib::companion::has_unlocked`).
//!
//! Moderation: by explicit choice there's no pre-publish review gate — a
//! character is visible and purchasable the moment it's created. It's
//! flagged `needs_review` and every admin/owner gets an `admin_notice`
//! pointing at the existing `/admin/companion/` tool, where they can
//! inspect the prompt and delete it if it's a problem. Re-editing a
//! character's prompt re-flags it for another look.

use super::admin::{
    image_ext, images_dir, multipart_boundary, parse_price_coins, sanitize_mood, MAX_IMAGE_BYTES,
};
use crate::routes::me::{cookies_of, json_response, me_uid, parse_body_strict};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::auth;
use mitch_lib::jsval::str_or;
use serde_json::json;
use std::sync::Arc;

/// Flat, non-refundable fee to create a custom character — a pure sink,
/// same spirit as the companion subscription cost: real coins leave the
/// economy, nobody is credited for it.
const CREATION_FEE_COINS: f64 = 500.0;

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
    if path == "/api/companion/mine" {
        return match *method {
            Method::GET => Some(list(state, headers)),
            Method::POST => Some(create(state, headers, body_bytes)),
            _ => None,
        };
    }
    if path == "/api/companion/mine/update" && *method == Method::POST {
        return Some(update(state, headers, body_bytes));
    }
    if path == "/api/companion/mine/delete" && *method == Method::POST {
        return Some(delete(state, headers, body_bytes));
    }
    if path == "/api/companion/mine/image" && *method == Method::POST {
        return Some(upload_image(state, headers, body_bytes).await);
    }
    if path == "/api/companion/mine/image/delete" && *method == Method::POST {
        return Some(delete_image(state, headers, body_bytes));
    }
    None
}

fn list(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let Some(email) = logged_in_email(state, headers) else {
        return json_response(401, json!({ "error": "not logged in" }));
    };
    let chars = mitch_lib::companion::list_characters_by_creator(&state.store, &email);
    let out: Vec<_> = chars
        .into_iter()
        .map(|c| {
            json!({
                "id": c.id,
                "name": c.name,
                "personalityPrompt": c.personality_prompt,
                "needsReview": c.needs_review,
                "priceCoins": c.price_coins,
                "moods": c.moods,
            })
        })
        .collect();
    json_response(200, json!({ "characters": out }))
}

/// Notifies every admin/owner that a new custom character needs a look —
/// matches the explicit "no pre-publish gate, but admins get a noti"
/// answer. A few duplicate sends if someone's listed twice is harmless.
fn notify_admins_of_new_character(
    state: &Arc<AppState>,
    creator_email: &str,
    name: &str,
    id: &str,
) {
    let cfg = mitch_lib::auth::load_admin_config(&state.store);
    let targets = cfg
        .owners
        .iter()
        .chain(cfg.admins.iter())
        .chain(cfg.co_owners.iter());
    for admin_email in targets {
        mitch_lib::coins::add_admin_notification(
            &state.store,
            state.data_dir(),
            admin_email,
            "New custom companion character",
            &format!("{creator_email} created \"{name}\" ({id}). Review its prompt in the companion admin tool."),
            "system",
            "",
            "/admin/companion/",
        );
    }
}

fn create(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(email) = logged_in_email(state, headers) else {
        return json_response(401, json!({ "error": "not logged in" }));
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let name = str_or(body.get("name"), "").trim().to_string();
    let prompt = str_or(body.get("personalityPrompt"), "").trim().to_string();
    if name.is_empty() || name.chars().count() > 60 {
        return json_response(400, json!({ "error": "name must be 1-60 characters" }));
    }
    if prompt.is_empty() || prompt.chars().count() > 4000 {
        return json_response(
            400,
            json!({ "error": "personality prompt must be 1-4000 characters" }),
        );
    }
    let price = match parse_price_coins(&body) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    if let Err(e) = mitch_lib::coins::spend_coins(
        &state.store,
        state.data_dir(),
        &email,
        CREATION_FEE_COINS,
        "companion_create_character",
    ) {
        return json_response(400, json!({ "error": format!("insufficient coins: {e}") }));
    }
    let id =
        mitch_lib::companion::create_custom_character(&state.store, &email, &name, &prompt, price);
    notify_admins_of_new_character(state, &email, &name, &id);
    json_response(200, json!({ "ok": true, "id": id }))
}

/// Confirms the caller actually owns this character, 404ing otherwise so
/// a user can't probe for other people's character ids via error
/// messages ("forbidden" vs "not found" would leak existence).
#[allow(clippy::result_large_err)] // the Err path is an early-return Response, not propagated far
fn owned_character(
    state: &Arc<AppState>,
    email: &str,
    id: &str,
) -> Result<mitch_lib::companion::Character, Response> {
    let norm = mitch_lib::auth::normalize_email(email);
    match mitch_lib::companion::get_character(&state.store, id) {
        Some(c) if c.creator_email == norm => Ok(c),
        _ => Err(json_response(
            404,
            json!({ "error": "character not found" }),
        )),
    }
}

fn update(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(email) = logged_in_email(state, headers) else {
        return json_response(401, json!({ "error": "not logged in" }));
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let id = str_or(body.get("id"), "");
    if let Err(resp) = owned_character(state, &email, &id) {
        return resp;
    }
    let name = str_or(body.get("name"), "").trim().to_string();
    let prompt = str_or(body.get("personalityPrompt"), "").trim().to_string();
    if name.is_empty() || name.chars().count() > 60 {
        return json_response(400, json!({ "error": "name must be 1-60 characters" }));
    }
    if prompt.is_empty() || prompt.chars().count() > 4000 {
        return json_response(
            400,
            json!({ "error": "personality prompt must be 1-4000 characters" }),
        );
    }
    let price = match parse_price_coins(&body) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    mitch_lib::companion::update_character(&state.store, &id, &name, &prompt, price);
    // Content changed — worth another look, same as at creation.
    mitch_lib::companion::flag_needs_review(&state.store, &id);
    json_response(200, json!({ "ok": true }))
}

fn delete(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(email) = logged_in_email(state, headers) else {
        return json_response(401, json!({ "error": "not logged in" }));
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let id = str_or(body.get("id"), "");
    if let Err(resp) = owned_character(state, &email, &id) {
        return resp;
    }
    if let Some(dir) = images_dir(state, &id) {
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
    mitch_lib::companion::delete_character(&state.store, &id);
    json_response(200, json!({ "ok": true }))
}

async fn upload_image(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(email) = logged_in_email(state, headers) else {
        return json_response(401, json!({ "error": "not logged in" }));
    };
    let ct_raw = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let Some(boundary) = multipart_boundary(ct_raw) else {
        return json_response(
            400,
            json!({ "error": "malformed multipart/form-data: no boundary" }),
        );
    };
    let owned = body_bytes.to_vec();
    let stream = futures_util::stream::once(async move {
        Ok::<_, std::io::Error>(axum::body::Bytes::from(owned))
    });
    let mut mp = multer::Multipart::new(stream, boundary);

    let mut character_id = String::new();
    let mut mood = String::new();
    let mut file: Option<(String, Vec<u8>)> = None;
    loop {
        let Ok(Some(field)) = mp.next_field().await else {
            break;
        };
        match field.name() {
            Some("id") => character_id = field.text().await.unwrap_or_default().trim().to_string(),
            Some("mood") => mood = field.text().await.unwrap_or_default().trim().to_string(),
            Some("file") => {
                let ftype = field
                    .content_type()
                    .map(|m| m.to_string())
                    .unwrap_or_default();
                if let Ok(data) = field.bytes().await {
                    file = Some((ftype, data.to_vec()));
                }
            }
            _ => {}
        }
    }

    if let Err(resp) = owned_character(state, &email, &character_id) {
        return resp;
    }
    let Some(mood) = sanitize_mood(&mood) else {
        return json_response(400, json!({ "error": "invalid mood name" }));
    };
    let Some((mime, data)) = file else {
        return json_response(400, json!({ "error": "no file provided" }));
    };
    if data.len() > MAX_IMAGE_BYTES {
        return json_response(413, json!({ "error": "image too large (5MB max)" }));
    }
    let ext = image_ext(&mime);
    if ext.is_empty() {
        return json_response(
            400,
            json!({ "error": "unsupported image type (png/jpg/webp/gif only)" }),
        );
    }
    let Some(dir) = images_dir(state, &character_id) else {
        return json_response(400, json!({ "error": "invalid character id" }));
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return json_response(500, json!({ "error": "failed to store image" }));
    }
    let filename = format!("{mood}.{ext}");
    if std::fs::write(dir.join(&filename), &data).is_err() {
        return json_response(500, json!({ "error": "failed to store image" }));
    }
    mitch_lib::companion::set_character_image(&state.store, &character_id, &mood, &filename);
    json_response(200, json!({ "ok": true, "mood": mood }))
}

fn delete_image(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(email) = logged_in_email(state, headers) else {
        return json_response(401, json!({ "error": "not logged in" }));
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let id = str_or(body.get("id"), "");
    if let Err(resp) = owned_character(state, &email, &id) {
        return resp;
    }
    let Some(mood) = sanitize_mood(&str_or(body.get("mood"), "")) else {
        return json_response(400, json!({ "error": "invalid mood name" }));
    };
    if let Some(file) = mitch_lib::companion::character_image_file(&state.store, &id, &mood) {
        if let Some(dir) = images_dir(state, &id) {
            let _ = std::fs::remove_file(dir.join(file));
        }
    }
    mitch_lib::companion::remove_character_image(&state.store, &id, &mood);
    json_response(200, json!({ "ok": true }))
}
