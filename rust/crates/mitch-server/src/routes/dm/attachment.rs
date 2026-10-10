//! `/api/dm/attachment*` (server.js:20099-20440) — the attachment store:
//! upload (multipart / JSON base64 / raw stream), serve, list, delete, plus
//! the `cleanExpiredE2eAttachments` sweeper and the 250MB per-user quota.
//!
//! Storage layout matches the JS exactly:
//! - the index is `data/e2e_attachments.json` — a DB-routed document
//!   (NOT in PRESERVED_DATA_FILES), written through the data layer;
//! - the blobs are raw files under `data/e2e_attachments/<id><ext>`, named by
//!   a 16-byte hex id plus a sanitized extension.
//!
//! Body-cap note (main.rs): the JS reads `req.arrayBuffer()` / `req.formData()`
//! with no cap and enforces the 250MB quota after reading. Rust buffers the
//! body with an 8MB margin (`upload_body_cap`), so everything up to that cap
//! takes the identical quota path; a LARGER body is replaced by cap+1 zeros,
//! which still reaches the raw-branch 413 (file size > quota) but degrades to
//! a 500 on the multipart branch — documented Step-11 deviation for >264MB
//! uploads (Rust refuses to buffer them).

use super::{is_revoked_id, qs_get};
use crate::handler::get_real_ip;
use crate::routes::me::{cookies_of, json_response, parse_body_strict};
use crate::state::AppState;
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::Response;
use mitch_lib::auth::{self, encode_uri_component};
use mitch_lib::crypto::random_bytes_hex;
use mitch_lib::data::DataStore;
use mitch_lib::dm;
use mitch_lib::jsval::{self, number, truthy};
use mitch_lib::school::now_millis;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// `E2E_MAX_USER_BYTES` (server.js:215) — 250 MB max per user.
pub(super) const E2E_MAX_USER_BYTES: usize = 250 * 1024 * 1024;

/// `E2E_ATTACHMENT_TTL_MS` (server.js:216) — 2 days retention.
const E2E_ATTACHMENT_TTL_MS: usize = 2 * 24 * 60 * 60 * 1000;

/// The index document path (a DB-routed doc, not a preserved file).
fn index_path(data_dir: &Path) -> PathBuf {
    data_dir.join("e2e_attachments.json")
}

/// The raw blob directory (`E2E_ATTACHMENTS_DIR`).
fn attachments_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("e2e_attachments")
}

/// What the three upload branches hand back: either a direct jsonResp return
/// from inside the JS `try` block, or a thrown error caught by the JS `catch`
/// (→ 500 `Failed to upload attachment`).
enum UploadErr {
    Early(u16, Value),
    Fail(String),
}

/// `POST /api/dm/attachment/upload` — the full upload ladder: rate limit,
/// auth, `cleanExpiredE2eAttachments`, then the content-type branch.
pub(super) async fn upload(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body_bytes: &[u8],
    search: &str,
) -> Response {
    if let Some(resp) = rate_gate(state, headers, "/api/dm/attachment/upload") {
        return resp;
    }
    let email = match att_auth(state, headers) {
        Ok(e) => e,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    clean_expired(state);
    // `const ct = (req.headers.get('content-type') || '').toLowerCase();`
    let ct_raw = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let ct = ct_raw.to_lowercase();
    let out = if ct.contains("multipart/form-data") {
        upload_multipart(state, &email, ct_raw, body_bytes).await
    } else if ct.contains("application/json") {
        upload_json(state, &email, headers, body_bytes)
    } else {
        upload_raw(state, &email, headers, body_bytes, search)
    };
    match out {
        Ok(v) => json_response(200, v),
        Err(UploadErr::Early(code, obj)) => json_response(code, obj),
        Err(UploadErr::Fail(msg)) => json_response(
            500,
            json!({ "error": "Failed to upload attachment", "message": msg }),
        ),
    }
}

/// `GET /api/dm/attachment` — serve one attachment blob.
pub(super) fn get_attachment(state: &Arc<AppState>, headers: &HeaderMap, search: &str) -> Response {
    // The gate is the point — userEmail itself is unused below (server.js:20292).
    if let Err((code, msg)) = att_auth(state, headers) {
        return json_response(code, json!({ "error": msg }));
    }
    // `String(urlObj.searchParams.get('id') || '').trim()`
    let id = qs_get(search, "id").unwrap_or_default().trim().to_string();
    let id_ok = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !id_ok {
        return json_response(400, json!({ "error": "invalid attachment id" }));
    }

    clean_expired(state);
    let idx = load_index(&state.store, state.data_dir());
    let att = idx.get(&id).filter(|a| truthy(a));
    let Some(att) = att else {
        return json_response(404, json!({ "error": "attachment not found or expired" }));
    };
    // `att.expiresAt && Date.now() > att.expiresAt` — numeric coercion with
    // NaN never expiring.
    let expired = match att.get("expiresAt") {
        Some(e) if truthy(e) => now_millis() as f64 > number(e).unwrap_or(f64::NAN),
        _ => false,
    };
    if expired {
        clean_expired(state);
        return json_response(404, json!({ "error": "attachment expired" }));
    }
    // `join(E2E_ATTACHMENTS_DIR, att.file)` — the JS would throw on a
    // missing/non-string file name; realistic entries always carry one.
    let Some(file) = att.get("file").and_then(|v| v.as_str()) else {
        return json_response(404, json!({ "error": "attachment file missing" }));
    };
    let path = attachments_dir(state.data_dir()).join(file);
    if !path.exists() {
        return json_response(404, json!({ "error": "attachment file missing" }));
    }
    let Ok(bytes) = std::fs::read(&path) else {
        return json_response(404, json!({ "error": "attachment file missing" }));
    };

    let is_download = qs_get(search, "download")
        .map(|v| v == "1")
        .unwrap_or(false);
    // `(att.name || 'attachment')` — truthy gate, then String().
    let disp_name = jsval::str_or(att.get("name"), "attachment");
    // `.replace(/["\r\n]/g, '_')`
    let safe_name: String = disp_name
        .chars()
        .map(|c| {
            if c == '"' || c == '\r' || c == '\n' {
                '_'
            } else {
                c
            }
        })
        .collect();
    let encoded_name = encode_uri_component(&disp_name);
    let disposition = format!(
        "{}; filename=\"{}\"; filename*=UTF-8''{}",
        if is_download { "attachment" } else { "inline" },
        safe_name,
        encoded_name
    );
    let mime = jsval::str_or(att.get("mime"), "application/octet-stream");
    // A junk `mime` (arbitrary ≤80-char upload input) can't form a header
    // value; JS would throw when building the Response. Degrade to octet-stream.
    let mime_value = HeaderValue::from_str(&mime)
        .unwrap_or(HeaderValue::from_static("application/octet-stream"));
    let disposition_value = HeaderValue::from_str(&disposition)
        .unwrap_or(HeaderValue::from_static("inline; filename=\"attachment\""));
    axum::response::Response::builder()
        .status(200)
        .header("content-type", mime_value)
        .header("content-disposition", disposition_value)
        .header(
            "cache-control",
            "private, no-cache, no-store, must-revalidate",
        )
        .header("x-content-type-options", "nosniff")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| json_response(500, json!({ "error": "attachment serve failed" })))
}

/// `GET /api/dm/attachments` — list this user's attachments and usage.
pub(super) fn list(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let email = match att_auth(state, headers) {
        Ok(e) => e,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    clean_expired(state);
    let idx = load_index(&state.store, state.data_dir());
    let (total, items) = usage(&email, &idx);
    json_response(
        200,
        json!({
            "total": super::js_num_value(total),
            "max": super::js_num_value(E2E_MAX_USER_BYTES as f64),
            "ttlMs": super::js_num_value(E2E_ATTACHMENT_TTL_MS as f64),
            "items": items,
        }),
    )
}

/// `POST /api/dm/attachments/delete` — delete ids to free quota. Admins and
/// moderators may delete anyone's attachments.
pub(super) fn delete_attachments(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Response {
    if let Some(resp) = rate_gate(state, headers, "/api/dm/attachments/delete") {
        return resp;
    }
    let email = match att_auth(state, headers) {
        Ok(e) => e,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    // `tryParseJson(65536)` → 400 'bad json' (declared CL, streamed length,
    // or parse failure).
    let body = match parse_limited(headers, body_bytes, 65536, "bad json") {
        Ok(b) => b,
        Err((code, obj)) => return json_response(code, obj),
    };
    // `Array.isArray(body?.ids) ? body.ids.map(x => String(x || '').trim())
    // .filter(Boolean) : (body?.id ? [String(body.id).trim()] : [])`
    let ids: Vec<String> = if body.get("ids").map(|v| v.is_array()).unwrap_or(false) {
        body["ids"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter(|x| truthy(x))
                    .map(|x| jsval::string(x).trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    } else if body.get("id").map(truthy).unwrap_or(false) {
        vec![jsval::string(body.get("id").unwrap_or(&Value::Null))
            .trim()
            .to_string()]
    } else {
        Vec::new()
    };
    if ids.is_empty() {
        return json_response(400, json!({ "error": "no attachments specified" }));
    }

    let data_dir = state.data_dir();
    let dir = attachments_dir(data_dir);
    let norm = auth::normalize_email(&email);
    let privileged = auth::is_admin_email(&state.store, &email)
        || auth::is_moderator_email(&state.store, &email);
    let mut idx = load_index(&state.store, data_dir);
    let mut modified = false;
    if let Some(map) = idx.as_object_mut() {
        for id in &ids {
            // `att && (normalizeEmail(att.user) === norm || isPrivileged)`
            let should_delete = map
                .get(id.as_str())
                .map(|a| truthy(a) && (norm_of_user(a) == norm || privileged))
                .unwrap_or(false);
            if !should_delete {
                continue;
            }
            // `if (att.file) { … unlink … }` — truthy string only.
            if let Some(f) = map
                .get(id.as_str())
                .and_then(|a| a.get("file"))
                .and_then(|v| v.as_str())
            {
                if !f.is_empty() {
                    let p = dir.join(f);
                    if p.exists() {
                        let _ = std::fs::remove_file(p);
                    }
                }
            }
            map.remove(id.as_str());
            modified = true;
        }
    }
    // `if (modified) await saveE2eAttachmentsIndex(idx);` — a save failure
    // throws in the JS (unhandled 500); here it degrades to a still-correct
    // usage report. Only observable on disk failure.
    if modified {
        let _ = save_index(&state.store, data_dir, &idx);
    }
    let (total, items) = usage(&email, &idx);
    json_response(
        200,
        json!({
            "success": true,
            "deleted": ids.len(),
            "total": super::js_num_value(total),
            "max": super::js_num_value(E2E_MAX_USER_BYTES as f64),
            "items": items,
        }),
    )
}

// ── Upload branches ──────────────────────────────────────────────────────────

/// The multipart/form-data branch (`await req.formData()`).
async fn upload_multipart(
    state: &Arc<AppState>,
    email: &str,
    ct_raw: &str,
    body_bytes: &[u8],
) -> Result<Value, UploadErr> {
    // bun's formData() throws on a multipart content-type with no boundary;
    // multer surfaces the same class of error.
    let Some(boundary) = multipart_boundary(ct_raw) else {
        return Err(UploadErr::Fail(
            "malformed multipart/form-data: no boundary".to_string(),
        ));
    };
    let owned = body_bytes.to_vec();
    let stream = futures_util::stream::once(async move {
        Ok::<_, std::io::Error>(axum::body::Bytes::from(owned))
    });
    let mut mp = multer::Multipart::new(stream, boundary);
    // `form.get('file')` — the FIRST entry named 'file', string value or File.
    // A part with no filename parameter is a string value → 'no file provided'.
    let mut file_part: Option<(String, String, Vec<u8>)> = None;
    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|e| UploadErr::Fail(e.to_string()))?
    {
        if field.name() != Some("file") {
            continue;
        }
        // field.bytes() consumes the field — capture its metadata first. A
        // part with no filename parameter is a string value → 'no file
        // provided'.
        let fname = field.file_name().map(str::to_string);
        let ftype = field
            .content_type()
            .map(|m| m.to_string())
            .unwrap_or_default();
        let data = field
            .bytes()
            .await
            .map_err(|e| UploadErr::Fail(e.to_string()))?;
        if let Some(fname) = fname {
            file_part = Some((fname, ftype, data.to_vec()));
        }
        break;
    }
    let Some((raw_name, raw_type, data)) = file_part else {
        return Err(UploadErr::Early(
            400,
            json!({ "error": "no file provided" }),
        ));
    };
    // `String(file.name || 'attachment').slice(0, 180)`
    let file_name = jsval::js_slice_utf16(
        &if raw_name.is_empty() {
            "attachment".to_string()
        } else {
            raw_name
        },
        180,
    );
    // `String(file.type || 'application/octet-stream').slice(0, 80)`
    let file_mime = jsval::js_slice_utf16(
        &if raw_type.is_empty() {
            "application/octet-stream".to_string()
        } else {
            raw_type
        },
        80,
    );
    // `Number(file.size) || 0` — always a non-negative integer here.
    let file_size = data.len() as f64;
    if file_size <= 0.0 {
        return Err(UploadErr::Early(400, json!({ "error": "empty file" })));
    }
    if file_size > E2E_MAX_USER_BYTES as f64 {
        return Err(UploadErr::Early(
            413,
            json!({
                "error": "quotaExceeded",
                "message": "Attachment exceeds 250MB maximum file size limit.",
                "max": super::js_num_value(E2E_MAX_USER_BYTES as f64),
            }),
        ));
    }
    let idx = load_index(&state.store, state.data_dir());
    let (total, items) = usage(email, &idx);
    if total + file_size > E2E_MAX_USER_BYTES as f64 {
        return Err(UploadErr::Early(413, quota_with_mb(&total, &items)));
    }
    persist(state, email, &file_name, &file_mime, file_size, &data)
}

/// The application/json branch (`{ name, mime, data: <base64> }`).
fn upload_json(
    state: &Arc<AppState>,
    email: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Result<Value, UploadErr> {
    let body = parse_limited(
        headers,
        body_bytes,
        dm::max_chat_json_body_bytes(),
        "bad json or payload too large",
    )
    .map_err(|(c, o)| UploadErr::Early(c, o))?;
    // `if (!body || !body.data)` — body is always an object here.
    if !body.get("data").map(truthy).unwrap_or(false) {
        return Err(UploadErr::Early(
            400,
            json!({ "error": "no data provided" }),
        ));
    }
    let file_name = jsval::js_slice_utf16(
        &jsval::string(&jsval::or(body.get("name"), json!("attachment"))),
        180,
    );
    let file_mime = jsval::js_slice_utf16(
        &jsval::string(&jsval::or(
            body.get("mime"),
            json!("application/octet-stream"),
        )),
        80,
    );
    // `let dataStr = String(body.data); if (dataStr.includes(','))
    // dataStr = dataStr.split(',')[1];` — the segment AFTER the first comma.
    let mut data_str = jsval::string(body.get("data").unwrap_or(&Value::Null));
    if data_str.contains(',') {
        if let Some(second) = data_str.split(',').nth(1) {
            data_str = second.to_string();
        }
    }
    let data = js_base64_decode(&data_str);
    let file_size = data.len() as f64;
    if file_size <= 0.0 {
        return Err(UploadErr::Early(400, json!({ "error": "empty file" })));
    }
    let idx = load_index(&state.store, state.data_dir());
    let (total, items) = usage(email, &idx);
    if total + file_size > E2E_MAX_USER_BYTES as f64 {
        return Err(UploadErr::Early(413, quota_with_mb(&total, &items)));
    }
    persist(state, email, &file_name, &file_mime, file_size, &data)
}

/// The raw-stream branch (any other content type).
fn upload_raw(
    state: &Arc<AppState>,
    email: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
    search: &str,
) -> Result<Value, UploadErr> {
    // `decodeURIComponent(x-filename || searchParams('name') || 'attachment')`
    // — an empty header is falsy and falls through, and decodeURIComponent
    // throws on malformed input (→ the JS catch → 500).
    let raw_name = headers
        .get("x-filename")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| qs_get(search, "name"))
        .unwrap_or_else(|| "attachment".to_string());
    let decoded = match decode_uri_component_strict(&raw_name) {
        Some(s) => s,
        None => return Err(UploadErr::Fail("URI error".to_string())),
    };
    let file_name = jsval::js_slice_utf16(&decoded, 180);
    // The raw branch reads the RAW content-type (not the lowercased `ct`).
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream");
    let file_mime = jsval::js_slice_utf16(ct, 80);
    let idx = load_index(&state.store, state.data_dir());
    let (total, items) = usage(email, &idx);
    // `Number(content-length || 0)` — garbage is NaN (falsy), so parse
    // failure defaults to 0.
    let declared = headers
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .unwrap_or(0.0);
    // `if (declaredLen && (usage.total + declaredLen > E2E_MAX_USER_BYTES))`
    if declared != 0.0 && total + declared > E2E_MAX_USER_BYTES as f64 {
        return Err(UploadErr::Early(413, quota_plain(&total, &items)));
    }
    let id = random_bytes_hex(16);
    let disk_filename = format!("{id}{}", sanitize_ext(&file_name));
    // JS creates the directory before reading the body into the buffer.
    std::fs::create_dir_all(attachments_dir(state.data_dir()))
        .map_err(|e| UploadErr::Fail(format!("{e}")))?;
    let file_size = body_bytes.len() as f64;
    // NOTE: the raw branch has no empty-file check — a zero-byte upload
    // stores a 0-size entry.
    if total + file_size > E2E_MAX_USER_BYTES as f64 {
        return Err(UploadErr::Early(413, quota_plain(&total, &items)));
    }
    persist_as(
        state,
        email,
        &file_name,
        &file_mime,
        file_size,
        body_bytes,
        &id,
        &disk_filename,
    )
}

// ── Shared post-quota persistence ────────────────────────────────────────────

/// `id = randomBytes(16).toString('hex')`, extension sanitized, then write.
fn persist(
    state: &Arc<AppState>,
    email: &str,
    file_name: &str,
    file_mime: &str,
    file_size: f64,
    bytes: &[u8],
) -> Result<Value, UploadErr> {
    let id = random_bytes_hex(16);
    let disk_filename = format!("{id}{}", sanitize_ext(file_name));
    persist_as(
        state,
        email,
        file_name,
        file_mime,
        file_size,
        bytes,
        &id,
        &disk_filename,
    )
}

/// `writeFileSync(diskPath, buf)` + index entry + save + the upload response.
#[allow(clippy::too_many_arguments)]
fn persist_as(
    state: &Arc<AppState>,
    email: &str,
    file_name: &str,
    file_mime: &str,
    file_size: f64,
    bytes: &[u8],
    id: &str,
    disk_filename: &str,
) -> Result<Value, UploadErr> {
    let data_dir = state.data_dir();
    std::fs::create_dir_all(attachments_dir(data_dir))
        .map_err(|e| UploadErr::Fail(format!("{e}")))?;
    std::fs::write(attachments_dir(data_dir).join(disk_filename), bytes)
        .map_err(|e| UploadErr::Fail(format!("{e}")))?;
    let now = now_millis() as f64;
    let entry = json!({
        "id": id,
        "file": disk_filename,
        "name": file_name,
        "size": super::js_num_value(file_size),
        "mime": file_mime,
        "user": email,
        "ts": super::js_num_value(now),
        "expiresAt": super::js_num_value(now + E2E_ATTACHMENT_TTL_MS as f64),
    });
    let mut idx = load_index(&state.store, data_dir);
    // `idx[id] = {...}` — the index is always an object in practice (it is
    // only ever written by this code); a non-object would be a data error.
    let Some(obj) = idx.as_object_mut() else {
        return Err(UploadErr::Fail(
            "attachment index is not an object".to_string(),
        ));
    };
    obj.insert(id.to_string(), entry);
    save_index(&state.store, data_dir, &idx).map_err(|e| UploadErr::Fail(format!("{e}")))?;
    Ok(json!({
        "success": true,
        "attachment": {
            "id": id,
            "url": format!("/api/dm/attachment?id={id}"),
            "name": file_name,
            "size": super::js_num_value(file_size),
            "mime": file_mime,
            "expiresAt": super::js_num_value(now + E2E_ATTACHMENT_TTL_MS as f64),
        },
    }))
}

// ── Quota response shapes ────────────────────────────────────────────────────

/// The multipart/JSON usage-quota 413 (includes the used-MB figure).
fn quota_with_mb(total: &f64, items: &[Value]) -> Value {
    json!({
        "error": "quotaExceeded",
        "message": format!(
            "Storage quota exceeded (250MB max total). You have used {}MB. Delete old attachments to free up space.",
            js_to_fixed_1(total / (1024.0 * 1024.0))
        ),
        "total": super::js_num_value(*total),
        "max": super::js_num_value(E2E_MAX_USER_BYTES as f64),
        "items": items,
    })
}

/// The raw-stream usage-quota 413 (no used-MB figure).
fn quota_plain(total: &f64, items: &[Value]) -> Value {
    json!({
        "error": "quotaExceeded",
        "message": "Storage quota exceeded (250MB max total). Delete old attachments to free up space.",
        "total": super::js_num_value(*total),
        "max": super::js_num_value(E2E_MAX_USER_BYTES as f64),
        "items": items,
    })
}

// ── Index / cleanup / usage helpers ──────────────────────────────────────────

/// `loadE2eAttachmentsIndex()` — `loadJson(E2E_ATTACHMENTS_INDEX, {})`.
fn load_index(store: &DataStore, data_dir: &Path) -> Value {
    store.read_document(&index_path(data_dir), json!({}))
}

/// `saveE2eAttachmentsIndex(idx)` — `saveJson(E2E_ATTACHMENTS_INDEX, idx)`.
fn save_index(
    store: &DataStore,
    data_dir: &Path,
    idx: &Value,
) -> Result<(), mitch_lib::data::DataError> {
    store.write_document(&index_path(data_dir), idx)
}

/// `cleanExpiredE2eAttachments()` (server.js:6286) — drop and unlink every
/// entry whose `expiresAt` has passed (or whose entry is falsy). All failures
/// are swallowed (the JS wraps this in try/catch and only logs).
pub(crate) fn clean_expired(state: &AppState) {
    clean_expired_in(&state.store, state.data_dir());
}

fn clean_expired_in(store: &DataStore, data_dir: &Path) {
    let idx = load_index(store, data_dir);
    let now = now_millis() as f64;
    let dir = attachments_dir(data_dir);
    let mut expired_keys: Vec<String> = Vec::new();
    if let Some(obj) = idx.as_object() {
        for (id, att) in obj {
            // `if (!att || (att.expiresAt && now > att.expiresAt))`
            let is_expired = match att {
                v if !truthy(v) => true,
                v => match v.get("expiresAt") {
                    Some(e) if truthy(e) => now > number(e).unwrap_or(f64::NAN),
                    _ => false,
                },
            };
            if !is_expired {
                continue;
            }
            // `if (att && att.file)` — unlink best-effort.
            if truthy(att) {
                if let Some(f) = att.get("file").and_then(|v| v.as_str()) {
                    let p = dir.join(f);
                    if p.exists() {
                        let _ = std::fs::remove_file(p);
                    }
                }
            }
            expired_keys.push(id.clone());
        }
    }
    if expired_keys.is_empty() {
        return;
    }
    let mut idx = idx;
    if let Some(obj) = idx.as_object_mut() {
        for k in &expired_keys {
            obj.remove(k);
        }
    }
    let _ = save_index(store, data_dir, &idx);
}

/// `userE2eAttachmentUsage(userEmail, idx)` (server.js:6307) — the user's
/// byte total and item list (desc by truthy-Number ts, NaN comparator equal).
fn usage(email: &str, idx: &Value) -> (f64, Vec<Value>) {
    let norm = auth::normalize_email(email);
    let mut total = 0f64;
    let mut items: Vec<Value> = Vec::new();
    if let Some(obj) = idx.as_object() {
        for (id, att) in obj {
            // `if (att && normalizeEmail(att.user) === norm)`
            if !truthy(att) || norm_of_user(att) != norm {
                continue;
            }
            // `total += Number(att.size || 0)` — a truthy unparseable size
            // poisons the total with NaN (which never trips the quota gate).
            let size = match att.get("size") {
                Some(s) if truthy(s) => number(s).unwrap_or(f64::NAN),
                _ => 0.0,
            };
            total += size;
            // `items.push({ id, name: att.name, …, url })` — absent properties
            // are dropped by JSON.stringify (undefined), present-null kept.
            let mut item = Map::new();
            item.insert("id".to_string(), json!(id));
            for key in ["name", "size", "mime", "ts", "expiresAt"] {
                if let Some(v) = att.get(key) {
                    item.insert(key.to_string(), v.clone());
                }
            }
            item.insert(
                "url".to_string(),
                json!(format!("/api/dm/attachment?id={id}")),
            );
            items.push(Value::Object(item));
        }
    }
    // `items.sort((a, b) => (b.ts || 0) - (a.ts || 0))` — descending, stable;
    // a NaN difference compares equal (JS sort treats NaN as 0).
    items.sort_by(|a, b| {
        let d = ts_sort_key(b) - ts_sort_key(a);
        if d.is_nan() {
            std::cmp::Ordering::Equal
        } else {
            d.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
        }
    });
    (total, items)
}

/// `(b.ts || 0)` inside the sort comparator: falsy → 0, truthy-unparseable →
/// NaN (which the comparator then treats as equal).
fn ts_sort_key(item: &Value) -> f64 {
    match item.get("ts") {
        Some(t) if truthy(t) => number(t).unwrap_or(f64::NAN),
        _ => 0.0,
    }
}

/// `normalizeEmail(att.user)` on a raw entry.
fn norm_of_user(att: &Value) -> String {
    super::norm_of(&att.get("user").cloned().unwrap_or(Value::Null))
}

// ── The auth ladder ──────────────────────────────────────────────────────────

/// The attachment auth ladder (server.js:20104 et al) — valid sid → not
/// revoked → `cookies._authSession?.email || emailFromSid(sid) ||
/// names[sid] || ''` lowercased. Differs from dm_auth: it consults the
/// session cookie first and validates generations via emailFromSid.
fn att_auth(state: &AppState, headers: &HeaderMap) -> Result<String, (u16, &'static str)> {
    let cookies = cookies_of(state, headers);
    let sid = cookies.get("studentId").unwrap_or("");
    let sid = if sid.is_empty() {
        cookies.get("id").unwrap_or("")
    } else {
        sid
    };
    if sid.is_empty() || !auth::valid_id(sid, &state.id_secret) || is_revoked_id(state, sid) {
        return Err((401, "auth required"));
    }
    // `cookies._authSession?.email || …` — the parsed session record's email.
    let session_email = cookies
        .get("_authSession")
        .and_then(|v| serde_json::from_str::<Value>(v).ok())
        .and_then(|s| s.get("email").cloned())
        .filter(truthy)
        .map(|v| jsval::string(&v));
    let email = session_email
        .or_else(|| auth::email_from_sid(&state.store, &state.id_secret, sid))
        .or_else(|| auth::names_get_email(&state.store, sid))
        .unwrap_or_default()
        .to_lowercase();
    if email.is_empty() {
        return Err((403, "email not found"));
    }
    Ok(email)
}

/// `checkRateLimit(req, path)` — the same id_key derivation as dm/send.
fn rate_gate(state: &Arc<AppState>, headers: &HeaderMap, path: &'static str) -> Option<Response> {
    let ip = get_real_ip(headers, None);
    let cookies = cookies_of(state, headers);
    let sid = cookies.get("studentId").unwrap_or("");
    let sid = if sid.is_empty() {
        cookies.get("id").unwrap_or("")
    } else {
        sid
    };
    let id_key = if auth::valid_id(sid, &state.id_secret) {
        format!("id:{sid}")
    } else {
        "anon".to_string()
    };
    state
        .rate_limit_check(&ip, &id_key, path)
        .map(|(code, msg)| json_response(code, json!({ "error": msg })))
}

// ── Byte-exact JS helpers ────────────────────────────────────────────────────

/// `tryParseJson(max)` — declared CL > max, streamed body > max, or a parse
/// failure all return false in the JS; an empty body parses to `{}` success.
fn parse_limited(
    headers: &HeaderMap,
    body_bytes: &[u8],
    max: usize,
    msg: &'static str,
) -> Result<Value, (u16, Value)> {
    // `Number(header || 0)` — a missing header is 0; garbage is NaN (never >).
    let declared = headers
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .unwrap_or(f64::NAN);
    if declared > max as f64 {
        return Err((400, json!({ "error": msg })));
    }
    if body_bytes.len() > max {
        return Err((400, json!({ "error": msg })));
    }
    parse_body_strict(body_bytes).ok_or((400, json!({ "error": msg })))
}

/// Node `path.extname` on POSIX: the substring from the last '.' of the
/// basename; empty when the basename is empty or its only dot is the leading
/// character (bun probe: '.hidden' → '', 'file.' → '.', '..foo' → '.foo').
fn js_extname(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or("");
    match base.rfind('.') {
        Some(i) if i > 0 => base[i..].to_string(),
        _ => String::new(),
    }
}

/// `extname(fileName).slice(0, 10).replace(/[^a-zA-Z0-9._-]/g, '') || '.bin'`
/// — the slice happens BEFORE the replace.
fn sanitize_ext(file_name: &str) -> String {
    let sliced = jsval::js_slice_utf16(&js_extname(file_name), 10);
    let cleaned: String = sliced
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
        .collect();
    if cleaned.is_empty() {
        ".bin".to_string()
    } else {
        cleaned
    }
}

/// `Buffer.from(s, 'base64')` — Node ignores every byte outside the standard
/// and URL-safe alphabets and stops at the first '=' (bun probe:
/// '===aGVsbG8=' → empty, 'ab=cd==' → one byte, 'aGVs bG8!' → 'hello').
fn js_base64_decode(s: &str) -> Vec<u8> {
    let mut acc = 0u32;
    let mut bits = 0u32;
    let mut out = Vec::new();
    for b in s.bytes() {
        if b == b'=' {
            break;
        }
        let v = match b {
            b'A'..=b'Z' => u32::from(b - b'A'),
            b'a'..=b'z' => u32::from(b - b'a') + 26,
            b'0'..=b'9' => u32::from(b - b'0') + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => continue,
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    out
}

/// JS `decodeURIComponent` — strict: a lone '%', a truncated escape, or an
/// invalid UTF-8 sequence throws (None here → the caller's 500 'URI error').
fn decode_uri_component_strict(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let byte = u8::from_str_radix(s.get(i + 1..i + 3)?, 16).ok()?;
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// JS `(x).toFixed(1)` — correctly rounded with ties picking the larger n
/// (1.25 → "1.3", -1.25 → "-1.2"). Callers pass exact dyadic values, so
/// `n * 10` is exact and the tie test is reliable.
fn js_to_fixed_1(n: f64) -> String {
    if n.is_nan() {
        return "NaN".to_string();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    let scaled = n * 10.0;
    if (scaled - scaled.trunc()).abs() == 0.5 {
        return format!("{:.1}", scaled.ceil() / 10.0);
    }
    format!("{n:.1}")
}

/// The `boundary=` parameter of a multipart Content-Type header (WHATWG MIME
/// parameter parsing: case-insensitive name, optional quoted-string value).
fn multipart_boundary(ct: &str) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_store(tag: &str) -> (PathBuf, PathBuf, DataStore) {
        let base = std::env::temp_dir().join(format!(
            "mitch-att-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("data")).unwrap();
        let store = DataStore::open(&base, &base.join("data")).unwrap();
        (base.clone(), base.join("data"), store)
    }

    #[test]
    fn extname_matches_node() {
        assert_eq!(js_extname("a.b.c"), ".c");
        assert_eq!(js_extname(".hidden"), "");
        assert_eq!(js_extname("file."), ".");
        assert_eq!(js_extname("..foo"), ".foo");
        assert_eq!(js_extname("dir/file"), "");
        assert_eq!(js_extname(""), "");
        assert_eq!(js_extname("dir.name/file"), "");
    }

    #[test]
    fn sanitize_ext_slices_before_replacing() {
        assert_eq!(sanitize_ext("report.pdf"), ".pdf");
        assert_eq!(sanitize_ext("a.tar.gz"), ".gz");
        assert_eq!(sanitize_ext(".hidden"), ".bin");
        assert_eq!(sanitize_ext("file."), ".");
        // 'f.abcdefgh$ij' → ext '.abcdefgh$ij' (12 chars) → slice(0,10) is
        // '.abcdefgh$' → '$' stripped by the replace.
        assert_eq!(sanitize_ext("f.abcdefgh$ij"), ".abcdefgh");
        assert_eq!(sanitize_ext("noext"), ".bin");
        assert_eq!(sanitize_ext(""), ".bin");
        assert_eq!(sanitize_ext("x.abcdefghijklmno"), ".abcdefghi");
    }

    #[test]
    fn base64_decode_matches_buffer_from() {
        // All five vectors verified against bun's Buffer.from.
        assert_eq!(js_base64_decode("aGVsbG8!"), b"hello".to_vec());
        assert_eq!(js_base64_decode("ab=cd=="), vec![105]);
        assert_eq!(js_base64_decode("a-b_c"), vec![107, 230, 255]);
        assert_eq!(js_base64_decode("aGVs bG8="), b"hello".to_vec());
        assert_eq!(js_base64_decode("===aGVsbG8="), Vec::<u8>::new());
        assert_eq!(js_base64_decode(""), Vec::<u8>::new());
    }

    #[test]
    fn strict_uri_decode() {
        assert_eq!(
            decode_uri_component_strict("50%25+off"),
            Some("50%+off".into())
        );
        assert_eq!(decode_uri_component_strict("%2B"), Some("+".into()));
        assert_eq!(decode_uri_component_strict("%C3%A9"), Some("é".into()));
        assert_eq!(
            decode_uri_component_strict("plain.txt"),
            Some("plain.txt".into())
        );
        assert_eq!(decode_uri_component_strict("%zz"), None);
        assert_eq!(decode_uri_component_strict("%C3"), None);
        assert_eq!(decode_uri_component_strict("50%"), None);
    }

    #[test]
    fn to_fixed_1_matches_js() {
        // All verified against bun.
        assert_eq!(js_to_fixed_1(1.25), "1.3");
        assert_eq!(js_to_fixed_1(12.25), "12.3");
        assert_eq!(js_to_fixed_1(1.35), "1.4");
        assert_eq!(js_to_fixed_1(0.05), "0.1");
        assert_eq!(js_to_fixed_1(1310720.0 / 1048576.0), "1.3");
        assert_eq!(js_to_fixed_1(0.0), "0.0");
        assert_eq!(js_to_fixed_1(f64::NAN), "NaN");
        assert_eq!(js_to_fixed_1(f64::INFINITY), "Infinity");
        assert_eq!(js_to_fixed_1(f64::NEG_INFINITY), "-Infinity");
        assert_eq!(js_to_fixed_1(-1.25), "-1.2");
    }

    #[test]
    fn multipart_boundary_extracts_param() {
        assert_eq!(
            multipart_boundary("multipart/form-data; boundary=----WebKitFormBoundaryX"),
            Some("----WebKitFormBoundaryX".to_string())
        );
        assert_eq!(
            multipart_boundary("multipart/form-data; boundary=\"quoted value\""),
            Some("quoted value".to_string())
        );
        assert_eq!(
            multipart_boundary("multipart/form-data;Boundary=AbC"),
            Some("AbC".to_string())
        );
        assert_eq!(multipart_boundary("multipart/form-data"), None);
        assert_eq!(multipart_boundary("multipart/form-data; boundary="), None);
        assert_eq!(
            multipart_boundary("multipart/form-data; charset=utf-8; boundary=b1"),
            Some("b1".to_string())
        );
    }

    #[test]
    fn usage_totals_sizes_and_drops_undefined() {
        let idx = json!({
            "a": { "id": "a", "name": "one.png", "size": 100, "mime": "image/png", "ts": 50, "user": "Me@Student.rjuhsd.us", "expiresAt": 999 },
            "b": { "id": "b", "size": 23, "user": "me@student.rjuhsd.us" },
            "c": { "id": "c", "size": 9999, "user": "other@student.rjuhsd.us" },
            "d": { "size": "garbage", "user": "me@student.rjuhsd.us" },
            "e": { "id": "e", "size": 5, "user": "me+tag@student.rjuhsd.us" }
        });
        let (total, items) = usage("me@student.rjuhsd.us", &idx);
        // 'e' normalizes to the same address (plus-tag strip) and its size 5
        // counts; 'd' has a truthy unparseable size → NaN poisons the total.
        assert!(total.is_nan());
        // Order: b (no ts → 0) and d (no ts → 0) keep insertion order around
        // nothing else; a has ts 50 so it sorts first (descending).
        assert_eq!(items[0].get("id").and_then(|v| v.as_str()), Some("a"));
        assert_eq!(items.len(), 4);
        // 'b' has no name/ts/mime/expiresAt → those keys are dropped.
        let b = items
            .iter()
            .find(|i| i.get("id") == Some(&json!("b")))
            .unwrap();
        assert!(b.get("name").is_none());
        assert!(b.get("ts").is_none());
        assert!(b.get("mime").is_none());
        assert_eq!(b.get("size"), Some(&json!(23)));
        assert_eq!(b.get("url"), Some(&json!("/api/dm/attachment?id=b")));
        // A present-null ts is kept (not dropped).
        let with_null = json!({ "id": "n", "user": "me@student.rjuhsd.us", "ts": null, "size": 1 });
        let idx2 = json!({ "n": with_null });
        let (_, items2) = usage("me@student.rjuhsd.us", &idx2);
        assert_eq!(items2[0].get("ts"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn usage_sorts_desc_by_ts_with_nan_equal() {
        let idx = json!({
            "x": { "id": "x", "user": "a@x", "ts": "garbage" },
            "y": { "id": "y", "user": "a@x", "ts": 10 },
            "z": { "id": "z", "user": "a@x", "ts": 5 }
        });
        let (_, items) = usage("a@x", &idx);
        let ids: Vec<&str> = items
            .iter()
            .map(|i| i.get("id").and_then(|v| v.as_str()).unwrap_or(""))
            .collect();
        // y(10) then z(5) — but x's ts is a NaN sort key, which compares
        // EQUAL to everything, and a stable sort keeps equal elements in
        // their original order: x stays first.
        assert_eq!(ids, vec!["x", "y", "z"]);
    }

    #[test]
    fn clean_expired_unlinks_and_prunes() {
        let (_base, data, store) = temp_store("clean");
        let dir = attachments_dir(&data);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old.bin"), b"old").unwrap();
        std::fs::write(dir.join("keep.bin"), b"keep").unwrap();
        let now = now_millis() as i64;
        let idx = json!({
            "old": { "id": "old", "file": "old.bin", "user": "a@x", "size": 3, "expiresAt": now - 1 },
            "keep": { "id": "keep", "file": "keep.bin", "user": "a@x", "size": 4, "expiresAt": now + 1000 },
            "noexp": { "id": "noexp", "file": "keep.bin", "user": "a@x", "size": 4 },
            "falsy": null,
            "badexp": { "id": "badexp", "file": "keep.bin", "user": "a@x", "expiresAt": "garbage" }
        });
        store.write_document(&index_path(&data), &idx).unwrap();
        clean_expired_in(&store, &data);
        assert!(!dir.join("old.bin").exists());
        assert!(dir.join("keep.bin").exists());
        let after = store.read_document(&index_path(&data), json!({}));
        assert!(after.get("old").is_none());
        assert!(after.get("keep").is_some());
        assert!(after.get("noexp").is_some());
        // A falsy entry is dropped without touching any file.
        assert!(after.get("falsy").is_none());
        // A truthy unparseable expiresAt never expires (NaN comparison).
        assert!(after.get("badexp").is_some());
    }

    #[test]
    fn parse_limited_gates() {
        let mut h = HeaderMap::new();
        h.insert("content-length", HeaderValue::from_static("10"));
        // Empty body → {} success.
        assert_eq!(parse_limited(&h, b"", 100, "bad").unwrap(), json!({}));
        // Declared CL > max → Err.
        assert!(parse_limited(&h, b"", 5, "bad").is_err());
        // Streamed body > max → Err.
        assert!(parse_limited(&h, b"0123456789", 5, "bad").is_err());
        // Bad json → Err; garbage CL is NaN (never > max) and falls through.
        let mut h2 = HeaderMap::new();
        h2.insert("content-length", HeaderValue::from_static("garbage"));
        assert!(parse_limited(&h2, b"{nope", 100, "bad").is_err());
        assert_eq!(
            parse_limited(&h2, b"{\"a\":1}", 100, "bad").unwrap(),
            json!({"a":1})
        );
    }
}
