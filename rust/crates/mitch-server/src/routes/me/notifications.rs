//! Notification + tutorial endpoints (server.js:14166-14305, 15935-15948,
//! 18899-19045): `/api/me/coin-gifts` (+`/read`), `/api/me/notif-prefs`
//! GET/POST, `/api/me/notifications` (+`/read`), `/api/me/complete-tutorial`.
//!
//! JS notes preserved: `coin-gifts`, `notifications`, and their `/read`
//! twins have NO method check (any verb matches); every auth check reads
//! `cookies['studentId'] || cookies['id'] || ''`.

use super::me_uid;
use super::{cookies_of, data_file, json_response, parse_body_strict};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::auth;
use mitch_lib::jsval;
use serde_json::{json, Map, Value};
use std::sync::Arc;

pub(crate) fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body: &Value,
    body_bytes: &[u8],
) -> Option<Response> {
    // No-method-check endpoints first (JS matches on path alone).
    if path == "/api/me/coin-gifts/read" {
        return Some(me_coin_gifts_read(state, headers, body, body_bytes));
    }
    if path == "/api/me/notifications/read" {
        return Some(me_notifications_read(state, headers, body, body_bytes));
    }
    if path == "/api/me/coin-gifts" {
        return Some(me_coin_gifts(state, headers));
    }
    if path == "/api/me/notifications" {
        return Some(me_notifications(state, headers));
    }
    if path == "/api/me/notif-prefs" && *method == Method::GET {
        return Some(me_notif_prefs_get(state, headers));
    }
    if path == "/api/me/notif-prefs" && *method == Method::POST {
        return Some(me_notif_prefs_post(state, headers, body, body_bytes));
    }
    if path == "/api/me/complete-tutorial" && *method == Method::POST {
        return Some(me_complete_tutorial(state, headers));
    }
    None
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ── Notif prefs ─────────────────────────────────────────────────────────────

/// `NOTIF_PREF_DEFAULTS` (server.js:2197-2206).
pub(crate) fn notif_pref_defaults() -> Value {
    json!({
        "dm": true,
        "group": true,
        "friends_online": true,
        "digest": true,
        "quietEnabled": false,
        "quietStart": "22:00",
        "quietEnd": "08:00",
        "tzOffset": 0,
    })
}

/// `getNotifPrefs` (server.js:2208-2211).
pub(crate) fn get_notif_prefs(state: &AppState, norm: &str) -> Value {
    let all = state
        .store
        .read_document(&data_file(state, "notif_prefs.json"), json!({}));
    let defaults = notif_pref_defaults();
    let stored = all.get(norm).cloned().unwrap_or(json!({}));
    let mut out = defaults;
    if let (Some(base), Some(extra)) = (out.as_object_mut(), stored.as_object()) {
        for (k, v) in extra {
            base.insert(k.clone(), v.clone());
        }
    }
    out
}

fn me_notif_prefs_get(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let cookies = cookies_of(state, headers);
    let uid = me_uid(&cookies);
    if !auth::valid_id(&uid, &state.id_secret) {
        return json_response(401, json!({ "error": "Not authenticated" }));
    }
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &uid) else {
        return json_response(401, json!({ "error": "email not found" }));
    };
    json_response(
        200,
        json!({ "prefs": get_notif_prefs(state, &auth::normalize_email(&email)) }),
    )
}

/// `POST /api/me/notif-prefs` — server.js:14186-14220. Bool keys via
/// `=== true`, HH:MM quiet hours, tzOffset clamp ±840; unknown keys dropped.
fn me_notif_prefs_post(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body: &Value,
    body_bytes: &[u8],
) -> Response {
    if parse_body_strict(body_bytes).is_none() {
        return json_response(400, json!({ "error": "bad json" }));
    }
    let cookies = cookies_of(state, headers);
    let uid = me_uid(&cookies);
    if !auth::valid_id(&uid, &state.id_secret) {
        return json_response(401, json!({ "error": "Not authenticated" }));
    }
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &uid) else {
        return json_response(401, json!({ "error": "email not found" }));
    };
    let norm = auth::normalize_email(&email);

    const NOTIF_BOOL_KEYS: &[&str] = &["dm", "group", "friends_online", "digest", "quietEnabled"];
    let incoming_obj = body.as_object().filter(|_| !body.is_array());
    let mut next = get_notif_prefs(state, &norm);
    if let Some(incoming) = incoming_obj {
        for (key, value) in incoming {
            if NOTIF_BOOL_KEYS.contains(&key.as_str()) {
                if let Some(obj) = next.as_object_mut() {
                    obj.insert(key.clone(), json!(value.as_bool().unwrap_or(false)));
                }
            } else if key == "quietStart" || key == "quietEnd" {
                let s = jsval::str_or(Some(value), "");
                if !is_hhmm(&s) {
                    return json_response(400, json!({ "error": "quiet hours must be HH:MM" }));
                }
                if let Some(obj) = next.as_object_mut() {
                    obj.insert(key.clone(), json!(s));
                }
            } else if key == "tzOffset" {
                // `Number(incoming[key])` — NaN → 400 bad tzOffset.
                let Some(off) = jsval::number(value) else {
                    return json_response(400, json!({ "error": "bad tzOffset" }));
                };
                if let Some(obj) = next.as_object_mut() {
                    obj.insert(key.clone(), json!(off.round().clamp(-840.0, 840.0) as i64));
                }
            }
        }
    }
    let quiet_enabled = next
        .get("quietEnabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let qs = next
        .get("quietStart")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let qe = next.get("quietEnd").and_then(|v| v.as_str()).unwrap_or("");
    if quiet_enabled && qs == qe {
        return json_response(
            400,
            json!({ "error": "quiet hours start and end must differ" }),
        );
    }
    let prefs_file = data_file(state, "notif_prefs.json");
    let mut all = state.store.read_document(&prefs_file, json!({}));
    if let Some(obj) = all.as_object_mut() {
        obj.insert(norm.clone(), next.clone());
    }
    if state.store.write_document(&prefs_file, &all).is_err() {
        return json_response(400, json!({ "error": "save failed" }));
    }
    json_response(200, json!({ "ok": true, "prefs": next }))
}

/// `/^([01]?\d|2[0-3]):[0-5]\d$/`.
fn is_hhmm(s: &str) -> bool {
    let bytes = s.as_bytes();
    let (hh, rest) = match bytes.len() {
        5 => (&s[0..2], &s[3..5]),
        4 => (&s[0..1], &s[2..4]),
        _ => return false,
    };
    if !rest.as_bytes().iter().all(|c| c.is_ascii_digit()) {
        return false;
    }
    if bytes[bytes.len() - 3] != b':' {
        return false;
    }
    let Ok(h) = hh.parse::<u32>() else {
        return false;
    };
    let Ok(m) = rest.parse::<u32>() else {
        return false;
    };
    h <= 23 && m <= 59
}

// ── Coin gifts ──────────────────────────────────────────────────────────────

/// `/api/me/coin-gifts` — server.js:18899-18911. Unread only, first 10.
fn me_coin_gifts(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let cookies = cookies_of(state, headers);
    let uid = me_uid(&cookies);
    if !auth::valid_id(&uid, &state.id_secret) {
        return json_response(401, json!({ "error": "Not authenticated" }));
    }
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &uid) else {
        return json_response(401, json!({ "error": "email not found" }));
    };
    let norm = auth::normalize_email(&email);
    let notices: Vec<Value> = mitch_lib::coins::coin_gift_notices_for(&state.store, &norm)
        .into_iter()
        .filter(|g| !g.get("read").and_then(|v| v.as_bool()).unwrap_or(false))
        .take(10)
        .collect();
    json_response(200, json!({ "notices": notices }))
}

/// `/api/me/coin-gifts/read` — server.js:14166-14184. Empty `ids` marks all;
/// the store always gains `gifts[norm]` (JS writes even an empty array).
fn me_coin_gifts_read(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body: &Value,
    body_bytes: &[u8],
) -> Response {
    if parse_body_strict(body_bytes).is_none() {
        return json_response(400, json!({ "error": "bad json" }));
    }
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    if !auth::valid_id(&sid, &state.id_secret) {
        return json_response(401, json!({ "error": "unauthorized" }));
    }
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &sid) else {
        return json_response(401, json!({ "error": "email not found" }));
    };
    let norm = auth::normalize_email(&email);
    let ids: std::collections::HashSet<String> = body
        .get("ids")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().map(jsval::string).collect())
        .unwrap_or_default();
    mitch_lib::coins::mark_coin_gift_notices_read(&state.store, &norm, &ids);
    json_response(200, json!({ "ok": true }))
}

// ── Notifications aggregate ─────────────────────────────────────────────────

/// `/api/me/notifications` — server.js:18931-19046. Four notice families,
/// sorted ts desc, capped 25; `unread` counts the pre-trim length.
fn me_notifications(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let cookies = cookies_of(state, headers);
    let uid = me_uid(&cookies);
    if !auth::valid_id(&uid, &state.id_secret) {
        return json_response(401, json!({ "error": "Not authenticated" }));
    }
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &uid) else {
        return json_response(401, json!({ "error": "email not found" }));
    };
    let norm = auth::normalize_email(&email);

    let mut notices: Vec<Value> = Vec::new();
    for g in mitch_lib::coins::coin_gift_notices_for(&state.store, &norm) {
        if g.get("read").and_then(|v| v.as_bool()).unwrap_or(false) {
            continue;
        }
        if g.get("kind").and_then(|v| v.as_str()) == Some("admin_notice") {
            notices.push(json!({
                "type": "admin_notice",
                "id": jsval::string_of(g.get("id")),
                "title": jsval::str_or(g.get("title"), "Admin notification"),
                "body": format!("From site admin via {}", jsval::str_or(g.get("source"), "mitchdog.com")),
                "detail": jsval::str_or(g.get("message"), ""),
                "ts": jsval::or(g.get("ts"), json!(0)),
                "url": jsval::or(g.get("url"), json!(notification_url("/"))),
            }));
            continue;
        }
        let amount = g.get("amount").and_then(jsval::number).unwrap_or(0.0);
        notices.push(json!({
            "type": "coin_gift",
            "id": jsval::string_of(g.get("id")),
            "title": "Coin gift received",
            "body": format!(
                "{} coins from {}",
                jsval::number_to_locale_string(amount),
                jsval::str_or(g.get("from"), "admin")
            ),
            "detail": format!("Reason: {}", jsval::str_or(g.get("reason"), "admin gift")),
            "ts": jsval::or(g.get("ts"), json!(0)),
        }));
    }

    let dms = state
        .store
        .read_document(&data_file(state, "dms.json"), json!([]));
    let now = now_millis();
    // DM notices grouped per sender.
    let mut dm_by_sender: std::collections::HashMap<String, (i64, i64, String)> =
        std::collections::HashMap::new(); // count, latestTs, latestText
    for m in dms.as_array().into_iter().flatten() {
        if mitch_lib::chat::is_message_expired(&state.store, state.data_dir(), m, false, now) {
            continue;
        }
        if auth::normalize_email(&jsval::str_or(m.get("to"), "")) != norm
            || jsval::truthy(m.get("read").unwrap_or(&Value::Null))
        {
            continue;
        }
        let from = auth::normalize_email(&jsval::str_or(m.get("from"), ""));
        if from.is_empty() {
            continue;
        }
        let ts = m.get("ts").and_then(jsval::number).unwrap_or(0.0) as i64;
        let entry = dm_by_sender
            .entry(from.clone())
            .or_insert((0, 0, String::new()));
        entry.0 += 1;
        if ts >= entry.1 {
            entry.1 = ts;
            let mut text = dm_content_of(m, &state.id_secret);
            if serde_json::from_str::<Value>(&text)
                .map(|p| p.get("e2e").is_some())
                .unwrap_or(false)
            {
                text = "[Secure Message]".to_string();
            }
            // String(textToShow).slice(0, 120) — UTF-16 code units in JS;
            // char-based cap keeps the common (BMP) case identical.
            entry.2 = text.chars().take(120).collect();
        }
    }
    for (from, (count, latest_ts, latest_text)) in &dm_by_sender {
        let display = mitch_lib::profile::display_email(
            &state.store,
            state.data_dir(),
            &state.id_secret,
            from,
        );
        notices.push(json!({
            "type": "dm",
            "id": format!("dm:{}", from),
            "from": display,
            "title": format!("{} encrypted chat message{}", count, if *count == 1 { "" } else { "s" }),
            "body": format!("From {}", display),
            "detail": if latest_text.is_empty() { String::new() } else { format!("Latest: {}", latest_text) },
            "ts": latest_ts,
            "url": notification_url("/encrypt/"),
        }));
    }

    // Group notices — only groups the viewer belongs to.
    let groups = state
        .store
        .read_document(&data_file(state, "groups.json"), json!([]));
    let mut my_group_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for group in groups.as_array().into_iter().flatten() {
        let is_member = group
            .get("members")
            .and_then(|v| v.as_array())
            .map(|members| {
                members
                    .iter()
                    .any(|m| auth::normalize_email(&jsval::str_or(Some(m), "")) == norm)
            })
            .unwrap_or(false);
        if is_member {
            my_group_ids.insert(jsval::string(&jsval::or(group.get("id"), json!(""))));
        }
    }
    let mut group_by_id: std::collections::HashMap<String, (i64, i64, String)> =
        std::collections::HashMap::new(); // count, latestTs, name
    for m in dms.as_array().into_iter().flatten() {
        if m.get("kind").and_then(|v| v.as_str()) != Some("group") {
            continue;
        }
        let group_id = jsval::string(&jsval::or(m.get("groupId"), json!("")));
        if !my_group_ids.contains(&group_id) {
            continue;
        }
        if mitch_lib::chat::is_message_expired(&state.store, state.data_dir(), m, false, now) {
            continue;
        }
        if auth::normalize_email(&jsval::str_or(m.get("from"), "")) == norm {
            continue;
        }
        let read = m
            .get("readBy")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .any(|r| auth::normalize_email(&jsval::str_or(Some(r), "")) == norm)
            })
            .unwrap_or(false);
        if read {
            continue;
        }
        let ts = m.get("ts").and_then(jsval::number).unwrap_or(0.0) as i64;
        let name = jsval::str_or(m.get("groupName"), "Group chat");
        let entry = group_by_id.entry(group_id).or_insert((0, 0, name));
        entry.0 += 1;
        if ts >= entry.1 {
            entry.1 = ts;
        }
    }
    for (group_id, (count, latest_ts, name)) in &group_by_id {
        notices.push(json!({
            "type": "group_dm",
            "id": format!("group:{}", group_id),
            "groupId": group_id,
            "title": format!("{} message{} in {}", count, if *count == 1 { "" } else { "s" }, name),
            "body": "Encrypted group chat",
            "detail": "Open Secure Chat to read the conversation.",
            "ts": latest_ts,
            "url": notification_url("/encrypt/"),
        }));
    }

    // Matrix notifications (messages, calls, invites).
    let matrix = state
        .store
        .read_document(&data_file(state, "matrix_notifications.json"), json!({}));
    for mn in matrix
        .get(norm.as_str())
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        if mn.get("read").and_then(|v| v.as_bool()).unwrap_or(false) {
            continue;
        }
        let room_id = jsval::str_or(mn.get("roomId"), "");
        let room_fallback = format!("/matrix/#/room/{}", urlencoding_encode(&room_id));
        // JS object literal: absent keys (undefined) are dropped by
        // JSON.stringify — mirror by only inserting present source keys.
        let mut obj = Map::new();
        obj.insert("type".into(), jsval::or(mn.get("type"), json!("matrix")));
        if let Some(id) = mn.get("id") {
            obj.insert("id".into(), id.clone());
        }
        if let Some(rid) = mn.get("roomId") {
            obj.insert("matrixRoomId".into(), rid.clone());
        }
        if let Some(title) = mn.get("title") {
            obj.insert("title".into(), title.clone());
        }
        obj.insert(
            "body".into(),
            jsval::or(mn.get("body"), json!("Matrix Chat")),
        );
        obj.insert("detail".into(), jsval::or(mn.get("detail"), json!("")));
        obj.insert("ts".into(), jsval::or(mn.get("ts"), json!(now)));
        let url = mn
            .get("url")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(notification_url)
            .unwrap_or_else(|| notification_url(&room_fallback));
        obj.insert("url".into(), json!(url));
        notices.push(Value::Object(obj));
    }

    notices.sort_by(|a, b| {
        let ta = a.get("ts").and_then(jsval::number).unwrap_or(0.0);
        let tb = b.get("ts").and_then(jsval::number).unwrap_or(0.0);
        tb.partial_cmp(&ta).unwrap_or(std::cmp::Ordering::Equal)
    });
    let unread = notices.len();
    let top = notices.into_iter().take(25).collect::<Vec<_>>();
    json_response(200, json!({ "notifications": top, "unread": unread }))
}

/// `notificationUrl` (server.js:2053-2055).
fn notification_url(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    }
}

/// `encodeURIComponent` for the matrix room fragment.
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b'!'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// `dmContentOf(msg).text` (server.js:479-488) — open `enc1:` payloads.
/// On a failed unseal JS falls through to the raw stored text (server.js:490),
/// which is what the shared `dm_content_parts` mirrors.
fn dm_content_of(msg: &Value, id_secret: &[u8]) -> String {
    super::dm_content_parts(msg, id_secret)
        .0
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// `/api/me/notifications/read` — server.js:14222-14304. Marks coin gifts,
/// DM reads, group readBy, and matrix notifications read.
fn me_notifications_read(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body: &Value,
    body_bytes: &[u8],
) -> Response {
    if parse_body_strict(body_bytes).is_none() {
        return json_response(400, json!({ "error": "bad json" }));
    }
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    if !auth::valid_id(&sid, &state.id_secret) {
        return json_response(401, json!({ "error": "unauthorized" }));
    }
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &sid) else {
        return json_response(401, json!({ "error": "email not found" }));
    };
    let norm = auth::normalize_email(&email);

    let mark_all = jsval::truthy(body.get("all").unwrap_or(&Value::Null));
    let coin_gift_ids: std::collections::HashSet<String> = body
        .get("coinGiftIds")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().map(jsval::string).collect())
        .unwrap_or_default();
    let dm_froms: std::collections::HashSet<String> = body
        .get("dmFroms")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|v| auth::normalize_email(&jsval::string(v)))
                .collect()
        })
        .unwrap_or_default();
    let group_ids: std::collections::HashSet<String> = body
        .get("groupIds")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().map(jsval::string).collect())
        .unwrap_or_default();

    // 1. Coin gifts.
    let gift_ids_to_mark = if mark_all {
        std::collections::HashSet::new()
    } else {
        coin_gift_ids.clone()
    };
    if mark_all || !coin_gift_ids.is_empty() {
        mitch_lib::coins::mark_coin_gift_notices_read(&state.store, &norm, &gift_ids_to_mark);
    }

    // 2. DMs + group reads.
    let dms_file = data_file(state, "dms.json");
    let mut dms = state.store.read_document(&dms_file, json!([]));
    let groups = state
        .store
        .read_document(&data_file(state, "groups.json"), json!([]));
    let mut readable_group_ids: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for group in groups.as_array().into_iter().flatten() {
        let is_member = group
            .get("members")
            .and_then(|v| v.as_array())
            .map(|members| {
                members
                    .iter()
                    .any(|m| auth::normalize_email(&jsval::str_or(Some(m), "")) == norm)
            })
            .unwrap_or(false);
        if is_member {
            readable_group_ids.insert(jsval::string(&jsval::or(group.get("id"), json!(""))));
        }
    }
    let mut dms_changed = false;
    if let Some(arr) = dms.as_array_mut() {
        for m in arr.iter_mut() {
            if m.get("kind").and_then(|v| v.as_str()) == Some("group") {
                let group_id = jsval::string(&jsval::or(m.get("groupId"), json!("")));
                let already = m
                    .get("readBy")
                    .and_then(|v| v.as_array())
                    .map(|readers| {
                        readers
                            .iter()
                            .any(|r| auth::normalize_email(&jsval::str_or(Some(r), "")) == norm)
                    })
                    .unwrap_or(false);
                if readable_group_ids.contains(&group_id)
                    && (mark_all || group_ids.contains(&group_id))
                    && !already
                {
                    if let Some(obj) = m.as_object_mut() {
                        let mut readers = obj
                            .get("readBy")
                            .and_then(|v| v.as_array())
                            .cloned()
                            .unwrap_or_default();
                        readers.push(json!(email));
                        obj.insert("readBy".into(), Value::Array(readers));
                        dms_changed = true;
                    }
                }
            } else {
                let to = auth::normalize_email(&jsval::str_or(m.get("to"), ""));
                if to == norm
                    && !m.get("read").and_then(|v| v.as_bool()).unwrap_or(false)
                    && (mark_all
                        || dm_froms
                            .contains(&auth::normalize_email(&jsval::str_or(m.get("from"), ""))))
                {
                    if let Some(obj) = m.as_object_mut() {
                        obj.insert("read".into(), json!(true));
                        dms_changed = true;
                    }
                }
            }
        }
    }
    if dms_changed {
        let _ = state.store.write_document(&dms_file, &dms);
    }

    // 3. Matrix notifications (ids from matrixIds/matrixRoomIds, plus
    // coinGiftIds entries prefixed matrix:/matrix-).
    let mut matrix_ids: std::collections::HashSet<String> = body
        .get("matrixIds")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().map(jsval::string).collect())
        .unwrap_or_default();
    let matrix_room_ids: std::collections::HashSet<String> = body
        .get("matrixRoomIds")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().map(jsval::string).collect())
        .unwrap_or_default();
    for id in &coin_gift_ids {
        if id.starts_with("matrix:") || id.starts_with("matrix-") {
            matrix_ids.insert(id.clone());
        }
    }
    let matrix_file = data_file(state, "matrix_notifications.json");
    let mut all_matrix = state.store.read_document(&matrix_file, json!({}));
    let mut matrix_changed = false;
    let mut mine_matrix: Vec<Value> = all_matrix
        .get(norm.as_str())
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for n in mine_matrix.iter_mut() {
        let id = jsval::string_of(n.get("id"));
        let room_id = jsval::string_of(n.get("roomId"));
        if mark_all || matrix_ids.contains(&id) || matrix_room_ids.contains(&room_id) {
            if !n.get("read").and_then(|v| v.as_bool()).unwrap_or(false) {
                matrix_changed = true;
            }
            if let Some(obj) = n.as_object_mut() {
                obj.insert("read".into(), json!(true));
            }
            cancel_pending_matrix_email_alert(
                state,
                &norm,
                n.get("roomId").and_then(|v| v.as_str()).unwrap_or(""),
            );
        }
    }
    if matrix_changed {
        if let Some(obj) = all_matrix.as_object_mut() {
            obj.insert(norm.clone(), Value::Array(mine_matrix));
        }
        let _ = state.store.write_document(&matrix_file, &all_matrix);
        // JS also calls triggerNotificationRefresh() (server.js:3120-3127).
        crate::ws::broadcast(
            state,
            crate::ws::WsRecipients::All,
            json!({ "type": "refresh_notifications" }).to_string(),
        );
    }

    json_response(200, json!({ "ok": true }))
}

/// `cancelPendingMatrixEmailAlert` (server.js:8497-8516) — clears the
/// in-process pending record; the delayed-send scheduler lands in Step 11.
fn cancel_pending_matrix_email_alert(state: &AppState, norm: &str, room_id: &str) {
    let Ok(mut pending) = state.matrix_pending_email_alerts.lock() else {
        return;
    };
    if room_id.is_empty() {
        let prefix = format!("{}:", norm);
        pending.retain(|k, _| !k.starts_with(&prefix));
    } else {
        pending.remove(&format!("{}:{}", norm, room_id));
    }
}

// ── Complete tutorial ───────────────────────────────────────────────────────

/// `POST /api/me/complete-tutorial` — server.js:15935-15948. Resolves the
/// email from the raw sid (no validId); replaces the stored profile with
/// the ensured defaults plus both tutorial flags.
fn me_complete_tutorial(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let cookies = cookies_of(state, headers);
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &me_uid(&cookies))
    else {
        return json_response(401, json!({ "error": "not logged in" }));
    };
    let norm = auth::normalize_email(&email);
    let mut profile = mitch_lib::profile::ensure_profile_defaults(
        &state.store,
        state.data_dir(),
        &state.id_secret,
        &norm,
        &email,
        &json!({}),
    );
    if let Some(obj) = profile.as_object_mut() {
        obj.insert("hasCompletedTutorial".into(), json!(true));
        obj.insert("has_completed_tutorial".into(), json!(true));
    }
    let profiles_file = data_file(state, "profiles.json");
    let mut profiles = state.store.read_document(&profiles_file, json!({}));
    if let Some(obj) = profiles.as_object_mut() {
        obj.insert(norm.clone(), profile);
    }
    if state
        .store
        .write_document(&profiles_file, &profiles)
        .is_err()
    {
        return json_response(400, json!({ "error": "save failed" }));
    }
    json_response(200, json!({ "ok": true }))
}
