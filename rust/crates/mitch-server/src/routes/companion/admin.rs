//! Admin half of the companion feature: the preset-authoring tool backend
//! (`/api/admin/companion/characters*`) and the image serve endpoint. The
//! global `/api/admin/*` gate in `handler.rs` already enforces
//! `isAnyAdminId` (+ passphrase for full admins) before any of this runs;
//! the `is_any_admin` re-checks below match the defense-in-depth pattern
//! other admin submodules use (e.g. `moderation::lookup_profile`).

use crate::routes::admin::admin_ctx;
use crate::routes::me::{cookies_of, json_response, me_uid, parse_body_strict};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::{IntoResponse, Response};
use mitch_lib::auth;
use mitch_lib::jsval::str_or;
use serde_json::json;
use std::sync::Arc;

pub(super) const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// A character price above this is almost certainly a typo (e.g. missing
/// a decimal point) rather than an intentional listing — also keeps
/// `f64` arithmetic in this module away from any extreme-value weirdness.
pub(super) const MAX_PRICE_COINS: f64 = 1_000_000.0;

pub(super) fn image_ext(mime: &str) -> &'static str {
    match mime.to_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "",
    }
}

/// `<mood>` as a safe filename component: lowercase alnum/dash/underscore,
/// capped at 40 chars, non-empty.
pub(super) fn sanitize_mood(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(40)
        .collect::<String>()
        .to_lowercase();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

pub(super) fn images_dir(state: &AppState, character_id: &str) -> std::path::PathBuf {
    state.data_dir().join("companion_images").join(character_id)
}

pub(super) fn multipart_boundary(ct: &str) -> Option<String> {
    for part in ct.split(';').skip(1) {
        let p = part.trim();
        let Some((name, value)) = p.split_once('=') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case("boundary") {
            continue;
        }
        let v = value.trim();
        let v = v
            .strip_prefix('"')
            .and_then(|r| r.strip_suffix('"'))
            .unwrap_or(v);
        if v.is_empty() {
            return None;
        }
        return Some(v.to_string());
    }
    None
}

pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Option<Response> {
    let ctx = admin_ctx(state, headers);
    if !ctx.is_any_admin(state) {
        return Some(json_response(403, json!({ "error": "forbidden" })));
    }
    if path == "/api/admin/companion/characters" {
        return match *method {
            Method::GET => Some(list(state)),
            Method::POST => Some(upsert(state, body_bytes)),
            _ => None,
        };
    }
    if path == "/api/admin/companion/characters/delete" && *method == Method::POST {
        return Some(delete(state, body_bytes));
    }
    if path == "/api/admin/companion/characters/set-default" && *method == Method::POST {
        return Some(set_default(state, body_bytes));
    }
    if path == "/api/admin/companion/characters/mark-reviewed" && *method == Method::POST {
        return Some(mark_reviewed(state, body_bytes));
    }
    if path == "/api/admin/companion/characters/image" && *method == Method::POST {
        return Some(upload_image(state, headers, body_bytes).await);
    }
    if path == "/api/admin/companion/characters/image/delete" && *method == Method::POST {
        return Some(delete_image(state, body_bytes));
    }
    None
}

fn list(state: &Arc<AppState>) -> Response {
    let chars = mitch_lib::companion::list_characters(&state.store);
    let out: Vec<_> = chars
        .into_iter()
        .map(|c| {
            json!({
                "id": c.id,
                "name": c.name,
                "personalityPrompt": c.personality_prompt,
                "isPreset": c.is_preset,
                "isDefault": c.is_default,
                "needsReview": c.needs_review,
                "creatorEmail": c.creator_email,
                "priceCoins": c.price_coins,
                "moods": c.moods,
            })
        })
        .collect();
    json_response(200, json!({ "characters": out }))
}

/// Parses and validates a price field shared with the user-facing
/// "my characters" create/edit endpoints — finite, non-negative, capped.
#[allow(clippy::result_large_err)] // the Err path is an early-return Response, not propagated far
pub(super) fn parse_price_coins(body: &serde_json::Value) -> Result<f64, Response> {
    let price = body
        .get("priceCoins")
        .and_then(mitch_lib::jsval::number)
        .unwrap_or(0.0);
    if !price.is_finite() || price < 0.0 || price > MAX_PRICE_COINS {
        return Err(json_response(
            400,
            json!({ "error": format!("price must be between 0 and {MAX_PRICE_COINS}") }),
        ));
    }
    Ok(price)
}

fn upsert(state: &Arc<AppState>, body_bytes: &[u8]) -> Response {
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
    let id = str_or(body.get("id"), "");
    if id.is_empty() {
        let new_id =
            mitch_lib::companion::create_preset_character(&state.store, &name, &prompt, price);
        return json_response(200, json!({ "ok": true, "id": new_id }));
    }
    if mitch_lib::companion::get_character(&state.store, &id).is_none() {
        return json_response(404, json!({ "error": "character not found" }));
    }
    mitch_lib::companion::update_character(&state.store, &id, &name, &prompt, price);
    json_response(200, json!({ "ok": true, "id": id }))
}

fn mark_reviewed(state: &Arc<AppState>, body_bytes: &[u8]) -> Response {
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let id = str_or(body.get("id"), "");
    if mitch_lib::companion::mark_reviewed(&state.store, &id) {
        json_response(200, json!({ "ok": true }))
    } else {
        json_response(404, json!({ "error": "character not found" }))
    }
}

fn delete(state: &Arc<AppState>, body_bytes: &[u8]) -> Response {
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let id = str_or(body.get("id"), "");
    if id.is_empty() {
        return json_response(400, json!({ "error": "id required" }));
    }
    let dir = images_dir(state, &id);
    if dir.exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    if mitch_lib::companion::delete_character(&state.store, &id) {
        json_response(200, json!({ "ok": true }))
    } else {
        json_response(404, json!({ "error": "character not found" }))
    }
}

fn set_default(state: &Arc<AppState>, body_bytes: &[u8]) -> Response {
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let id = str_or(body.get("id"), "");
    if mitch_lib::companion::get_character(&state.store, &id).is_none() {
        return json_response(404, json!({ "error": "character not found" }));
    }
    mitch_lib::companion::set_default_character(&state.store, &id);
    json_response(200, json!({ "ok": true }))
}

async fn upload_image(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
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
            Some("id") => {
                character_id = field.text().await.unwrap_or_default().trim().to_string();
            }
            Some("mood") => {
                mood = field.text().await.unwrap_or_default().trim().to_string();
            }
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

    if mitch_lib::companion::get_character(&state.store, &character_id).is_none() {
        return json_response(404, json!({ "error": "character not found" }));
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

    let dir = images_dir(state, &character_id);
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

fn delete_image(state: &Arc<AppState>, body_bytes: &[u8]) -> Response {
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let id = str_or(body.get("id"), "");
    let Some(mood) = sanitize_mood(&str_or(body.get("mood"), "")) else {
        return json_response(400, json!({ "error": "invalid mood name" }));
    };
    if let Some(file) = mitch_lib::companion::character_image_file(&state.store, &id, &mood) {
        let _ = std::fs::remove_file(images_dir(state, &id).join(file));
    }
    mitch_lib::companion::remove_character_image(&state.store, &id, &mood);
    json_response(200, json!({ "ok": true }))
}

/// `GET /api/companion/character-image?id=&mood=` — any logged-in user
/// (the widget needs this for whichever character it's rendering), not
/// admin-only. Lives here because it shares `images_dir` with the writer.
pub(super) fn serve_image(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    id: &str,
    mood: &str,
) -> Response {
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    if auth::email_from_sid(&state.store, &state.id_secret, &sid).is_none() {
        return json_response(401, json!({ "error": "not logged in" }));
    }
    let Some(mood) = sanitize_mood(mood) else {
        return json_response(400, json!({ "error": "invalid mood" }));
    };

    let maybe_file = mitch_lib::companion::character_image_file(&state.store, id, &mood)
        .or_else(|| mitch_lib::companion::character_fallback_image_file(&state.store, id));

    if let Some(file) = maybe_file {
        let path = images_dir(state, id).join(&file);
        if let Ok(bytes) = std::fs::read(&path) {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let mime = match ext {
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                "webp" => "image/webp",
                "gif" => "image/gif",
                "svg" => "image/svg+xml",
                _ => "application/octet-stream",
            };
            return (
                axum::http::StatusCode::OK,
                [
                    (axum::http::header::CONTENT_TYPE, mime),
                    (axum::http::header::CACHE_CONTROL, "private, max-age=3600"),
                ],
                bytes,
            )
                .into_response();
        }
    }

    // Fallback: serve a clean SVG companion avatar so the widget never breaks with 404
    const FALLBACK_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 120" width="120" height="120"><defs><radialGradient id="g" cx="50%" cy="40%" r="55%"><stop offset="0%" stop-color="#60a5fa"/><stop offset="100%" stop-color="#3b82f6"/></radialGradient></defs><circle cx="60" cy="60" r="54" fill="url(#g)"/><ellipse cx="60" cy="72" rx="34" ry="24" fill="#ffffff" opacity="0.9"/><circle cx="44" cy="52" r="7" fill="#1e293b"/><circle cx="76" cy="52" r="7" fill="#1e293b"/><circle cx="46" cy="50" r="2.5" fill="#ffffff"/><circle cx="78" cy="50" r="2.5" fill="#ffffff"/><circle cx="34" cy="62" r="5.5" fill="#f472b6" opacity="0.6"/><circle cx="86" cy="62" r="5.5" fill="#f472b6" opacity="0.6"/><path d="M 49 66 Q 60 76 71 66" fill="none" stroke="#1e293b" stroke-width="4" stroke-linecap="round"/></svg>"##;

    (
        axum::http::StatusCode::OK,
        [
            (axum::http::header::CONTENT_TYPE, "image/svg+xml"),
            (axum::http::header::CACHE_CONTROL, "private, max-age=300"),
        ],
        FALLBACK_SVG.as_bytes().to_vec(),
    )
        .into_response()
}
