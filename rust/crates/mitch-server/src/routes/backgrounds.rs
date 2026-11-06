//! Custom user backgrounds & wallpaper library (server.js:9500-9544, 11370-11684).
//!
//! - `GET /api/backgrounds/list` — list official wallpaper presets
//! - `POST /api/backgrounds/upload` — upload custom background image/video
//! - `GET /api/backgrounds/mine` — list user's uploaded custom backgrounds
//! - `POST /api/backgrounds/delete` — delete custom background
//! - `GET /api/bg/(thumb/)?<fileId>.<ext>` — public capability server for background assets

use super::blog::authed_email_for_request;
use super::me::{cookies_of, data_file, json_response, parse_body_strict};
use crate::state::AppState;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use mitch_lib::auth::{is_premium_email, normalize_email, valid_id};
use mitch_lib::crypto::sha256_hex;
use mitch_lib::school::now_millis;
use regex::Regex;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

const PICKLE_ORIGIN: &str = "https://sexypickleclub.com";

/// Resolves user background storage directory: `data/backgrounds/<sha256(norm)[..32]>/`.
pub fn bg_user_dir(data_dir: &Path, norm: &str) -> PathBuf {
    let hash = sha256_hex(norm.as_bytes());
    let dir = data_dir.join("backgrounds").join(&hash[..32]);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// MIME type to file extension for backgrounds.
pub fn bg_mime_ext(mime: &str) -> &'static str {
    match mime.to_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "video/mp4" => "mp4",
        "image/gif" => "gif",
        "video/webm" => "webm",
        _ => "",
    }
}

/// Converts a base filename into a display name.
pub fn format_bg_name(base: &str) -> String {
    let stripped = base.strip_prefix("bg-").unwrap_or(base);
    let with_spaces = stripped.replace('-', " ");
    let mut result = String::new();
    let mut capitalize_next = true;
    for ch in with_spaces.chars() {
        if ch.is_alphanumeric() {
            if capitalize_next {
                result.extend(ch.to_uppercase());
                capitalize_next = false;
            } else {
                result.push(ch);
            }
        } else {
            result.push(ch);
            capitalize_next = true;
        }
    }
    result
}

fn check_custom_rate_limit(
    state: &AppState,
    headers: &HeaderMap,
    bucket: &str,
) -> Option<Response> {
    let cookies = cookies_of(state, headers);
    let val = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cookies.get("id").unwrap_or(""));
    let id_key = if valid_id(val, &state.id_secret) {
        format!("id:{val}")
    } else {
        "anon".to_string()
    };
    let ip = crate::handler::get_real_ip(headers, None);
    if let Some((code, msg)) = state.rate_limit_check(&ip, &id_key, bucket) {
        Some(json_response(code, json!({ "error": msg })))
    } else {
        None
    }
}

fn unreachable_regex() -> Regex {
    Regex::new("$^").unwrap_or_else(|_| unreachable!())
}

/// Dispatches `/api/backgrounds/*` and `/api/bg/*` requests.
pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Option<Response> {
    // 1. GET /api/bg/(?:(thumb)/)?([0-9a-f]{24})\.(png|jpg|webp|webm)
    if path.starts_with("/api/bg/") {
        static BG_RE: OnceLock<Regex> = OnceLock::new();
        let bg_re = BG_RE.get_or_init(|| {
            Regex::new(r"^/api/bg/(?:(thumb)/)?([0-9a-f]{24})\.(png|jpg|webp|webm)$")
                .unwrap_or_else(|_| unreachable_regex())
        });

        if let Some(caps) = bg_re.captures(path) {
            if *method != Method::GET {
                return Some(StatusCode::METHOD_NOT_ALLOWED.into_response());
            }
            let is_thumb = caps.get(1).is_some();
            let file_id = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let req_ext = caps.get(3).map(|m| m.as_str()).unwrap_or("");

            let lib = state
                .store
                .read_document(&data_file(state, "backgrounds.json"), json!({}));
            let mut rec_val: Option<Value> = None;
            let mut owner_norm = String::new();

            if let Some(map) = lib.as_object() {
                for (norm, items) in map {
                    if let Some(arr) = items.as_array() {
                        if let Some(hit) = arr
                            .iter()
                            .find(|it| it.get("id").and_then(|v| v.as_str()) == Some(file_id))
                        {
                            rec_val = Some(hit.clone());
                            owner_norm = norm.clone();
                            break;
                        }
                    }
                }
            }

            let Some(rec) = rec_val else {
                return Some(StatusCode::NOT_FOUND.into_response());
            };

            let dir = bg_user_dir(state.data_dir(), &owner_norm);
            if is_thumb {
                let thumb_path = dir.join(format!("{file_id}_thumb.webp"));
                if thumb_path.is_file() {
                    if let Ok(bytes) = std::fs::read(&thumb_path) {
                        return Some(
                            Response::builder()
                                .status(200)
                                .header("Content-Type", "image/webp")
                                .header("Cache-Control", "public, max-age=31536000, immutable")
                                .header("Access-Control-Allow-Origin", PICKLE_ORIGIN)
                                .header("Vary", "Origin")
                                .header("Cross-Origin-Resource-Policy", "cross-origin")
                                .body(axum::body::Body::from(bytes))
                                .unwrap_or_else(|_| {
                                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                                }),
                        );
                    }
                }
            }

            let mime = rec.get("mime").and_then(|v| v.as_str()).unwrap_or("");
            let ext = bg_mime_ext(mime);
            let filename = rec.get("file").and_then(|v| v.as_str()).unwrap_or("");
            static FILE_RE: OnceLock<Regex> = OnceLock::new();
            let file_re = FILE_RE.get_or_init(|| {
                Regex::new(r"^[0-9a-f]{24}\.[a-z0-9]+$").unwrap_or_else(|_| unreachable_regex())
            });

            if ext != req_ext || !file_re.is_match(filename) {
                return Some(StatusCode::NOT_FOUND.into_response());
            }

            let file_path = dir.join(filename);
            if let Ok(bytes) = std::fs::read(&file_path) {
                return Some(
                    Response::builder()
                        .status(200)
                        .header("Content-Type", mime)
                        .header("Cache-Control", "public, max-age=31536000, immutable")
                        .header("Access-Control-Allow-Origin", PICKLE_ORIGIN)
                        .header("Vary", "Origin")
                        .header("Cross-Origin-Resource-Policy", "cross-origin")
                        .body(axum::body::Body::from(bytes))
                        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
                );
            }

            return Some(StatusCode::NOT_FOUND.into_response());
        }
    }

    if !path.starts_with("/api/backgrounds/") {
        return None;
    }

    // 2. GET /api/backgrounds/list
    if path == "/api/backgrounds/list" && *method == Method::GET {
        let dir = state.cfg.webroot.join("backgrounds");
        let thumbs_dir = dir.join("thumbs");

        let mut items: Vec<Value> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let mut filenames: Vec<String> = entries
                .filter_map(|res| res.ok())
                .filter(|entry| entry.file_type().map(|ft| ft.is_file()).unwrap_or(false))
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|f| f.ends_with(".webp") || f.ends_with(".webm"))
                .collect();

            filenames.sort();

            for f in filenames {
                let is_video = f.ends_with(".webm");
                let base = if is_video {
                    f.strip_suffix(".webm").unwrap_or(&f)
                } else {
                    f.strip_suffix(".webp").unwrap_or(&f)
                };

                let thumb_name = format!("{base}.webp");
                let has_thumb = thumbs_dir.join(&thumb_name).is_file();
                let id = base.strip_prefix("bg-").unwrap_or(base);
                let name = format_bg_name(base);
                let url = format!("/backgrounds/{f}");
                let thumb_url = if has_thumb {
                    format!("/backgrounds/thumbs/{thumb_name}")
                } else {
                    format!("/backgrounds/{f}")
                };

                items.push(json!({
                    "id": id,
                    "name": name,
                    "url": url,
                    "thumbUrl": thumb_url,
                    "type": if is_video { "video" } else { "image" },
                }));
            }
        }

        return Some(json_response(200, json!({ "ok": true, "items": items })));
    }

    // 3. POST /api/backgrounds/upload
    if path == "/api/backgrounds/upload" && *method == Method::POST {
        if let Some(resp) = check_custom_rate_limit(state, headers, "/api/backgrounds/upload") {
            return Some(resp);
        }
        let Some(email) = authed_email_for_request(state, headers) else {
            return Some(json_response(401, json!({ "error": "not logged in" })));
        };

        let is_premium = is_premium_email(&state.store, &email);
        let max_storage_bytes: u64 = if is_premium {
            1_000_000_000
        } else {
            100_000_000
        };

        let Some(body) = parse_body_strict(body_bytes) else {
            return Some(json_response(
                400,
                json!({ "error": "bad json or payload too large" }),
            ));
        };

        let mime = body
            .get("mime")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        let data = body.get("data").and_then(|v| v.as_str()).unwrap_or("");
        let ext = bg_mime_ext(&mime);
        if ext.is_empty() {
            return Some(json_response(
                400,
                json!({ "error": "unsupported image/video type" }),
            ));
        }

        let prefix = format!("data:{mime};base64,");
        if !data.starts_with(&prefix) {
            return Some(json_response(400, json!({ "error": "invalid image data" })));
        }
        let payload = &data[prefix.len()..];
        if payload.is_empty()
            || !payload.chars().all(|c| {
                c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=' || c.is_whitespace()
            })
        {
            return Some(json_response(400, json!({ "error": "invalid image data" })));
        }
        let clean_payload: String = payload.chars().filter(|c| !c.is_whitespace()).collect();
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(clean_payload.as_bytes())
        else {
            return Some(json_response(400, json!({ "error": "invalid image data" })));
        };
        if bytes.is_empty() {
            return Some(json_response(400, json!({ "error": "empty file" })));
        }

        let norm = normalize_email(&email);
        let file = data_file(state, "backgrounds.json");
        let raw = state.store.read_document(&file, json!({}));
        let mut lib = raw.as_object().cloned().unwrap_or_default();
        let mut items = lib
            .get(&norm)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let current_usage: u64 = items
            .iter()
            .map(|it| it.get("bytes").and_then(|v| v.as_u64()).unwrap_or(0))
            .sum();

        if current_usage + (bytes.len() as u64) > max_storage_bytes {
            let error_msg = if is_premium {
                "Storage quota exceeded (1GB limit for Premium)."
            } else {
                "Storage quota exceeded (100MB limit for free tier). Upgrade to Premium for 1GB!"
            };
            return Some(json_response(413, json!({ "error": error_msg })));
        }

        let id: String = (0..12)
            .map(|_| format!("{:02x}", rand::random::<u8>()))
            .collect();
        let u_dir = bg_user_dir(state.data_dir(), &norm);
        let filename = format!("{id}.{ext}");
        let file_path = u_dir.join(&filename);

        if std::fs::write(&file_path, &bytes).is_err() {
            return Some(json_response(
                500,
                json!({ "error": "failed to write file" }),
            ));
        }

        // Spawn async ffmpeg for thumbnail generation (if ffmpeg is present)
        let thumb_path = u_dir.join(format!("{id}_thumb.webp"));
        let is_video = ext == "webm" || ext == "mp4";
        let mut cmd = tokio::process::Command::new("ffmpeg");
        if is_video {
            cmd.args(["-y", "-ss", "00:00:01", "-i"])
                .arg(&file_path)
                .args(["-vframes", "1", "-vf", "scale=240:-1", "-q:v", "75"])
                .arg(&thumb_path);
        } else {
            cmd.args(["-y", "-i"])
                .arg(&file_path)
                .args(["-vf", "scale=240:-1", "-q:v", "75"])
                .arg(&thumb_path);
        }
        tokio::spawn(async move {
            let _ = cmd.output().await;
        });

        let raw_name = body.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let name = mitch_lib::jsval::js_slice_utf16(raw_name, 80);

        items.push(json!({
            "id": id,
            "file": filename,
            "mime": mime,
            "bytes": bytes.len(),
            "name": name,
            "ts": now_millis(),
        }));

        lib.insert(norm, Value::Array(items));
        let _ = state.store.write_document(&file, &Value::Object(lib));

        return Some(json_response(
            200,
            json!({
                "ok": true,
                "url": format!("/api/bg/{id}.{ext}"),
                "id": id,
            }),
        ));
    }

    // 4. GET /api/backgrounds/mine
    if path == "/api/backgrounds/mine" && *method == Method::GET {
        let Some(email) = authed_email_for_request(state, headers) else {
            return Some(json_response(401, json!({ "error": "not logged in" })));
        };
        let norm = normalize_email(&email);
        let u_dir = bg_user_dir(state.data_dir(), &norm);

        let file = data_file(state, "backgrounds.json");
        let raw = state.store.read_document(&file, json!({}));
        let lib = raw.as_object().cloned().unwrap_or_default();
        let items = lib
            .get(&norm)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let out_items: Vec<Value> = items
            .iter()
            .map(|item| {
                let item_id = item.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let file_name = item.get("file").and_then(|v| v.as_str()).unwrap_or("");
                let thumb_path = u_dir.join(format!("{item_id}_thumb.webp"));
                let has_thumb = thumb_path.is_file();
                let thumb_url = if has_thumb {
                    format!("/api/bg/thumb/{item_id}.webp")
                } else {
                    format!("/api/bg/{file_name}")
                };

                json!({
                    "id": item_id,
                    "url": format!("/api/bg/{file_name}"),
                    "thumbUrl": thumb_url,
                    "mime": item.get("mime").and_then(|v| v.as_str()).unwrap_or(""),
                    "bytes": item.get("bytes").and_then(|v| v.as_u64()).unwrap_or(0),
                    "name": item.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                    "ts": item.get("ts").and_then(|v| v.as_f64()).unwrap_or(0.0) as u64,
                })
            })
            .collect();

        return Some(json_response(
            200,
            json!({ "ok": true, "items": out_items }),
        ));
    }

    // 5. POST /api/backgrounds/delete
    if path == "/api/backgrounds/delete" && *method == Method::POST {
        if let Some(resp) = check_custom_rate_limit(state, headers, "/api/backgrounds/delete") {
            return Some(resp);
        }
        let Some(email) = authed_email_for_request(state, headers) else {
            return Some(json_response(401, json!({ "error": "not logged in" })));
        };
        let Some(body) = parse_body_strict(body_bytes) else {
            return Some(json_response(400, json!({ "error": "bad json" })));
        };
        let want_id = body.get("id").and_then(|v| v.as_str()).unwrap_or("");
        static ID_RE: OnceLock<Regex> = OnceLock::new();
        let id_re = ID_RE
            .get_or_init(|| Regex::new(r"^[0-9a-f]{24}$").unwrap_or_else(|_| unreachable_regex()));
        if !id_re.is_match(want_id) {
            return Some(json_response(400, json!({ "error": "bad id" })));
        }

        let norm = normalize_email(&email);
        let file = data_file(state, "backgrounds.json");
        let raw = state.store.read_document(&file, json!({}));
        let mut lib = raw.as_object().cloned().unwrap_or_default();
        let mut items = lib
            .get(&norm)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let pos = items
            .iter()
            .position(|it| it.get("id").and_then(|v| v.as_str()) == Some(want_id));
        let Some(idx) = pos else {
            return Some(json_response(404, json!({ "error": "not found" })));
        };

        let removed = items.remove(idx);
        let u_dir = bg_user_dir(state.data_dir(), &norm);
        if let Some(f) = removed.get("file").and_then(|v| v.as_str()) {
            let _ = std::fs::remove_file(u_dir.join(f));
        }
        let _ = std::fs::remove_file(u_dir.join(format!("{want_id}_thumb.webp")));

        lib.insert(norm, Value::Array(items));
        let _ = state.store.write_document(&file, &Value::Object(lib));

        return Some(json_response(200, json!({ "ok": true })));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bg_name() {
        assert_eq!(format_bg_name("bg-galaxy-cherry"), "Galaxy Cherry");
        assert_eq!(format_bg_name("space-invaders"), "Space Invaders");
        assert_eq!(format_bg_name("plain"), "Plain");
    }

    #[test]
    fn test_bg_mime_ext() {
        assert_eq!(bg_mime_ext("IMAGE/PNG"), "png");
        assert_eq!(bg_mime_ext("image/jpeg"), "jpg");
        assert_eq!(bg_mime_ext("video/webm"), "webm");
        assert_eq!(bg_mime_ext("video/mp4"), "mp4");
        assert_eq!(bg_mime_ext("application/pdf"), "");
    }

    use axum::body::to_bytes;
    use axum::http::HeaderValue;

    fn test_state() -> (Arc<AppState>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "mitch_bg_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(dir.join("data"));
        let _ = std::fs::create_dir_all(dir.join("webserver/backgrounds/thumbs"));
        let cfg = crate::hosts::SiteConfig::load();
        let cfg = crate::hosts::SiteConfig {
            data_dir: dir.join("data"),
            webroot: dir.join("webserver"),
            base_dir: dir.clone(),
            ..cfg
        };
        let store = Arc::new(
            mitch_lib::data::DataStore::open(&dir, &dir.join("data"))
                .unwrap_or_else(|e| panic!("store: {e}")),
        );
        (Arc::new(AppState::new(cfg, Arc::clone(&store))), dir)
    }

    fn auth_headers(state: &AppState, email: &str) -> HeaderMap {
        let sess = mitch_lib::auth::create_auth_session(
            &state.store,
            &state.id_secret,
            &normalize_email(email),
            email,
            false,
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_str(&format!(
                "mitch_session={}; studentId={}",
                sess.token, sess.sid
            ))
            .unwrap(),
        );
        headers
    }

    #[tokio::test]
    async fn test_backgrounds_lifecycle() {
        let (state, dir) = test_state();
        let user = "artist@student.rjuhsd.us";
        let headers = auth_headers(&state, user);

        // Populate webserver/backgrounds with a preset
        let preset_file = dir.join("webserver/backgrounds/bg-galaxy-cherry.webp");
        let _ = std::fs::write(&preset_file, b"dummy webp");

        // 1. GET /api/backgrounds/list
        let list_resp = handle(&state, &Method::GET, "/api/backgrounds/list", &headers, &[])
            .await
            .unwrap();
        assert_eq!(list_resp.status(), 200);
        let bytes = to_bytes(list_resp.into_body(), usize::MAX).await.unwrap();
        let list_val: Value = serde_json::from_slice(&bytes).unwrap();
        let items = list_val.get("items").and_then(|v| v.as_array()).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].get("id").and_then(|v| v.as_str()),
            Some("galaxy-cherry")
        );
        assert_eq!(
            items[0].get("name").and_then(|v| v.as_str()),
            Some("Galaxy Cherry")
        );

        // 2. Upload custom background
        let dummy_png_base64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
        let upload_body = json!({
            "mime": "image/png",
            "data": format!("data:image/png;base64,{dummy_png_base64}"),
            "name": "My Custom Wall"
        });
        let upload_resp = handle(
            &state,
            &Method::POST,
            "/api/backgrounds/upload",
            &headers,
            &serde_json::to_vec(&upload_body).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(upload_resp.status(), 200);
        let bytes = to_bytes(upload_resp.into_body(), usize::MAX).await.unwrap();
        let up_val: Value = serde_json::from_slice(&bytes).unwrap();
        let file_id = up_val
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let url = up_val
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        assert_eq!(url, format!("/api/bg/{file_id}.png"));

        // 3. GET /api/backgrounds/mine
        let mine_resp = handle(&state, &Method::GET, "/api/backgrounds/mine", &headers, &[])
            .await
            .unwrap();
        assert_eq!(mine_resp.status(), 200);
        let bytes = to_bytes(mine_resp.into_body(), usize::MAX).await.unwrap();
        let mine_val: Value = serde_json::from_slice(&bytes).unwrap();
        let my_items = mine_val.get("items").and_then(|v| v.as_array()).unwrap();
        assert_eq!(my_items.len(), 1);
        assert_eq!(
            my_items[0].get("id").and_then(|v| v.as_str()),
            Some(file_id.as_str())
        );

        // 4. Capability server GET /api/bg/:id.png
        let cap_resp = handle(&state, &Method::GET, &url, &HeaderMap::new(), &[])
            .await
            .unwrap();
        assert_eq!(cap_resp.status(), 200);
        assert_eq!(cap_resp.headers().get("Content-Type").unwrap(), "image/png");
        assert_eq!(
            cap_resp.headers().get("Cache-Control").unwrap(),
            "public, max-age=31536000, immutable"
        );

        // 5. Delete background
        let del_body = json!({ "id": file_id });
        let del_resp = handle(
            &state,
            &Method::POST,
            "/api/backgrounds/delete",
            &headers,
            &serde_json::to_vec(&del_body).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(del_resp.status(), 200);

        // Verify mine is now empty
        let mine_resp2 = handle(&state, &Method::GET, "/api/backgrounds/mine", &headers, &[])
            .await
            .unwrap();
        let bytes = to_bytes(mine_resp2.into_body(), usize::MAX).await.unwrap();
        let mine_val2: Value = serde_json::from_slice(&bytes).unwrap();
        let my_items2 = mine_val2.get("items").and_then(|v| v.as_array()).unwrap();
        assert_eq!(my_items2.len(), 0);

        // Capability server returns 404 after deletion
        let cap_resp2 = handle(&state, &Method::GET, &url, &HeaderMap::new(), &[])
            .await
            .unwrap();
        assert_eq!(cap_resp2.status(), 404);
    }
}
