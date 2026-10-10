//! `/api/dm/*` — 13 endpoints + `/ws` presence/chat (plan Step 11).
//! sealAtRest/openAtRest, allSockets/userPresence, same-origin upgrade check,
//! identical JSON message shapes. Ported before games: per-conversation state
//! is bounded and proves the WS + at-rest + presence stack. Wired into
//! handler.rs BEFORE canvas (JS order: dm at 19452, canvas at 21634+).
//!
//! Method checks mirror the JS exactly: /api/dm/send and /api/dm/inbox have
//! NO method gate (any verb runs the ladder), /api/dm/groups splits GET
//! (list) from POST (create) with PUT/DELETE falling through to 404, and
//! mark-read/leave/clear/expiry/report run on any verb. The four attachment
//! endpoints are all method-checked (upload/delete POST, serve/list GET)
//! with other verbs falling through to the 404/static path.

mod attachment;
mod inbox;
mod manage;
mod notif;
pub(crate) use attachment::clean_expired;
pub(crate) use notif::notif_allowed;
mod send;

use crate::routes::me::{cookies_of, data_file};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::auth;
use mitch_lib::dm::DmAddrIndex;
use mitch_lib::jsval::{self, truthy};
use serde_json::{json, Value};
use std::sync::Arc;

/// Dispatches `/api/dm/*` requests; `None` falls through to the next group.
pub(crate) async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
    search: &str,
) -> Option<Response> {
    if path == "/api/dm/send" {
        return send::handle(state, headers, body_bytes).await;
    }
    // /api/dm/inbox has no method check in the JS — any verb runs the ladder.
    if path == "/api/dm/inbox" {
        return Some(inbox::inbox(state, headers, search));
    }
    if path == "/api/dm/groups" {
        if *method == Method::GET {
            return Some(inbox::groups_get(state, headers));
        }
        // The create handler is gated to POST; other verbs fall through.
        if *method == Method::POST {
            return Some(manage::groups_post(state, headers, body_bytes).await);
        }
        return None;
    }
    // The remaining five have no method check in the JS either — GET with an
    // empty body parses to {} and runs the same ladder.
    if path == "/api/dm/mark-read" {
        return Some(manage::mark_read(state, headers, body_bytes).await);
    }
    if path == "/api/dm/group/leave" {
        return Some(manage::group_leave(state, headers, body_bytes).await);
    }
    if path == "/api/dm/clear" {
        return Some(manage::clear(state, headers, body_bytes).await);
    }
    if path == "/api/dm/expiry" {
        return Some(manage::expiry(state, headers, body_bytes).await);
    }
    if path == "/api/dm/report" {
        return Some(manage::report(state, headers, body_bytes).await);
    }
    // The attachment endpoints are method-checked in the JS; other verbs
    // fall through to the 404/static path like every unclaimed route.
    if path == "/api/dm/attachment/upload" {
        return if *method == Method::POST {
            Some(attachment::upload(state, headers, body_bytes, search).await)
        } else {
            None
        };
    }
    if path == "/api/dm/attachment" {
        return if *method == Method::GET {
            Some(attachment::get_attachment(state, headers, search))
        } else {
            None
        };
    }
    if path == "/api/dm/attachments" {
        return if *method == Method::GET {
            Some(attachment::list(state, headers))
        } else {
            None
        };
    }
    if path == "/api/dm/attachments/delete" {
        return if *method == Method::POST {
            Some(attachment::delete_attachments(state, headers, body_bytes))
        } else {
            None
        };
    }
    None
}

/// The raw-stream upload body cap (see attachment.rs module docs): the 250MB
/// per-user quota plus margin for multipart framing.
pub(crate) fn upload_body_cap() -> usize {
    attachment::E2E_MAX_USER_BYTES + 8 * 1024 * 1024
}

/// What the auth ladder hands to each endpoint handler.
pub(super) struct DmAuth {
    pub sid: String,
    pub email: String,
}

/// The shared ladder (valid sid → not revoked → names[sid] email): 401
/// `auth required`, then 403 `email not found`.
pub(super) fn dm_auth(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<DmAuth, (u16, &'static str)> {
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
    let email = auth::names_get_email(&state.store, sid)
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    if email.is_empty() {
        return Err((403, "email not found"));
    }
    Ok(DmAuth {
        sid: sid.to_string(),
        email,
    })
}

/// `isRevoked(id)` (server.js:2769) — key presence in revoked.json.
/// (members.rs keeps a private copy; DM needs it before its email lookup.)
pub(super) fn is_revoked_id(state: &AppState, sid: &str) -> bool {
    state
        .store
        .read_document(&data_file(state, "revoked.json"), json!({}))
        .get(sid)
        .is_some()
}

/// `normalizeEmail(v)` on a raw JSON value — the JS falsy gate (`if (!email)
/// return ''`) comes first, then String(v).
pub(super) fn norm_of(v: &Value) -> String {
    if !truthy(v) {
        return String::new();
    }
    auth::normalize_email(&jsval::string(v))
}

/// `resolveMemberRef(m) || normalizeEmail(m)` — the member-norm fold used by
/// every membership check.
pub(super) fn member_norm(idx: &DmAddrIndex, v: &Value) -> String {
    let raw = jsval::string(&jsval::or(Some(v), json!("")));
    let resolved = mitch_lib::dm::resolve_member_ref(idx, &raw);
    if resolved.is_empty() {
        auth::normalize_email(&raw)
    } else {
        resolved
    }
}

/// JS `===` on JSON values: numeric equality across int/float reprs, identity
/// (never equal) for objects/arrays, strict type mismatch otherwise.
pub(super) fn js_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            x.as_f64().unwrap_or(f64::NAN) == y.as_f64().unwrap_or(f64::NAN)
        }
        (Value::Object(_) | Value::Array(_), _) | (_, Value::Object(_) | Value::Array(_)) => false,
        _ => a == b,
    }
}

/// `'group:' + g.id` — String concatenation, where a missing key renders as
/// `"undefined"` and null renders as `"null"`.
pub(super) fn concat_key(prefix: &str, v: Option<&Value>) -> String {
    match v {
        Some(x) => format!("{}{}", prefix, jsval::string(x)),
        None => format!("{}undefined", prefix),
    }
}

/// `myCleared[k] || 0` in the middle of a `||` chain: returns Some only when
/// the stored value is TRUTHY (an unparseable truthy value yields NaN, which
/// is falsy in JS and must keep falling through the chain).
pub(super) fn cleared_lookup(my_cleared: &Value, key: &str) -> Option<f64> {
    let v = my_cleared.get(key)?;
    if !truthy(v) {
        return None;
    }
    Some(jsval::number(v).unwrap_or(f64::NAN))
}

/// `(m.readBy || []).some(e => normalizeEmail(e) === norm)` — a non-array
/// readBy would throw in JS (500); here it degrades to "not found".
pub(super) fn read_by_some(m: &Value, norm: &str) -> bool {
    m.get("readBy")
        .and_then(|v| v.as_array())
        .map(|entries| entries.iter().any(|e| norm_of(e) == norm))
        .unwrap_or(false)
}

/// `Number(x) || 0` — a whole double serializes without a trailing `.0`,
/// exactly like `JSON.stringify` of a JS number.
pub(super) fn js_num_value(n: f64) -> Value {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 9.007_199_254_740_992e15 {
        json!(n as i64)
    } else {
        json!(n)
    }
}

/// `String(m.groupId)` — a missing property renders "undefined" (a present
/// null renders "null" via jsval::string).
pub(super) fn string_prop(v: Option<&Value>) -> String {
    match v {
        Some(x) => jsval::string(x),
        None => "undefined".to_string(),
    }
}

/// `new URL(req.url).searchParams.get(k)` — WHATWG urlencoded decoding:
/// '+' becomes a space, %XX decodes leniently (invalid sequences pass
/// through, matching the bun probe `searchParams.get('%zz') === '%zz'`).
/// Shared by inbox.rs and attachment.rs.
pub(super) fn qs_get(search: &str, key: &str) -> Option<String> {
    for pair in search.trim_start_matches('?').split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if form_urlencoded_decode(k) == key {
            return Some(form_urlencoded_decode(v));
        }
    }
    None
}

fn form_urlencoded_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 3 <= bytes.len() => {
                if let Some(byte) = s
                    .get(i + 1..i + 3)
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                {
                    out.push(byte);
                    i += 3;
                    continue;
                }
                out.push(bytes[i]);
            }
            _ => out.push(bytes[i]),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
