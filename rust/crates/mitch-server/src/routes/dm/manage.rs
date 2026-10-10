//! `/api/dm/mark-read`, `/api/dm/groups` (create), `/api/dm/group/leave`,
//! `/api/dm/clear`, `/api/dm/expiry`, `/api/dm/report`
//! (server.js:19840-20099). None of these has a method check in the JS —
//! any verb with a parseable body runs the same ladder.

use super::{concat_key, dm_auth, js_eq, js_num_value, member_norm, norm_of, string_prop};
use crate::handler::get_real_ip;
use crate::hosts::is_pickle_host;
use crate::routes::me::{cookies_of, data_file, json_response, parse_body_strict};
use crate::routes::push::verify_recaptcha;
use crate::state::AppState;
use axum::http::HeaderMap;
use axum::response::Response;
use mitch_lib::auth;
use mitch_lib::coins;
use mitch_lib::dm::{self, DMS_MAIN, DMS_PICKLE};
use mitch_lib::jsval::{self, truthy};
use mitch_lib::profile::{default_username_for_email, normalize_username};
use mitch_lib::school::now_millis;
use serde_json::{json, Value};
use std::sync::Arc;

/// `await tryParseJson()` — declared Content-Length over the cap throws, a
/// streamed body over the cap throws, an EMPTY body parses to `{}`.
fn parse_body(headers: &HeaderMap, body_bytes: &[u8]) -> Result<Value, ()> {
    let max = dm::max_json_body_bytes();
    // Number(header || 0) — a missing header is 0; garbage is NaN (never >).
    let declared = headers
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .unwrap_or(f64::NAN);
    if declared > max as f64 {
        return Err(());
    }
    if body_bytes.len() > max {
        return Err(());
    }
    parse_body_strict(body_bytes).ok_or(())
}

fn store_of(headers: &HeaderMap) -> &'static dm::DmStore {
    if is_pickle_host(headers) {
        &DMS_PICKLE
    } else {
        &DMS_MAIN
    }
}

/// `Math.random().toString(36).slice(2, 8)` — six base-36 digits.
fn random_base36_6() -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let x: f64 = rand::random();
    let mut frac = x;
    let mut out = String::new();
    for _ in 0..6 {
        frac *= 36.0;
        let d = frac.floor();
        out.push(DIGITS[d as usize % 36] as char);
        frac -= d;
    }
    out
}

/// `POST /api/dm/groups` — create a group (GET is handled by inbox.rs).
pub(super) async fn groups_post(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Response {
    let auth = match dm_auth(state, headers) {
        Ok(a) => a,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    let body = match parse_body(headers, body_bytes) {
        Ok(b) => b,
        Err(()) => return json_response(400, json!({ "error": "bad json" })),
    };
    let name = {
        let raw = jsval::js_slice_utf16(
            jsval::string(&jsval::or(body.get("name"), json!(""))).trim(),
            60,
        );
        if raw.is_empty() {
            "Group chat".to_string()
        } else {
            raw
        }
    };
    // Store canonical emails so membership checks match regardless of the ref
    // style the client used (username, masked email, or real email).
    let idx = dm::address_index_cached(&state.store, &state.cfg.data_dir);
    let mut members = vec![auth.email.clone()];
    if let Some(raw) = body.get("members").and_then(|v| v.as_array()) {
        for m in raw {
            let resolved =
                dm::resolve_member_ref(&idx, &jsval::string(&jsval::or(Some(m), json!(""))));
            let value = if resolved.is_empty() {
                jsval::string(&jsval::or(Some(m), json!("")))
                    .to_lowercase()
                    .trim()
                    .to_string()
            } else {
                resolved
            };
            if !value.is_empty() && !members.contains(&value) {
                members.push(value);
            }
        }
    }
    if members.len() < 2 {
        return json_response(400, json!({ "error": "need at least one other member" }));
    }
    let store = store_of(headers);
    let group = json!({
        "id": format!("{}-{}", now_millis(), random_base36_6()),
        "name": name,
        "members": members,
        "createdBy": auth.email,
        "createdAt": now_millis(),
    });
    let mut groups: Vec<Value> = state
        .store
        .read_document(&data_file(state, store.groups), json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    groups.push(group.clone());
    state
        .store
        .write_document(&data_file(state, store.groups), &Value::Array(groups))
        .ok();
    json_response(200, json!({ "ok": true, "group": group }))
}

/// `/api/dm/mark-read` — mark the caller's DMs (or a group conversation) as
/// read. No revoked check and no email gate in the JS: myEmail may be ''.
pub(super) async fn mark_read(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Response {
    let cookies = cookies_of(state, headers);
    let sid = cookies.get("studentId").unwrap_or("");
    let sid = if sid.is_empty() {
        cookies.get("id").unwrap_or("")
    } else {
        sid
    };
    if sid.is_empty() || !auth::valid_id(sid, &state.id_secret) {
        return json_response(401, json!({ "error": "auth required" }));
    }
    let my_email = auth::names_get_email(&state.store, sid)
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    let body = match parse_body(headers, body_bytes) {
        Ok(b) => b,
        Err(()) => return json_response(400, json!({})),
    };
    let from = jsval::string(&jsval::or(body.get("from"), json!(""))).to_lowercase();
    let group_id = match body.get("groupId").filter(|v| truthy(v)) {
        Some(v) => jsval::string(v),
        None => String::new(),
    };
    let store = store_of(headers);
    let idx = dm::address_index_cached(&state.store, &state.cfg.data_dir);
    let profiles = state
        .store
        .read_document(&data_file(state, "profiles.json"), json!({}));
    let my_norm = auth::normalize_email(&my_email);
    // Per-message closure state the JS recomputes inside the loop; constant
    // per request, so hoisted (same semantics, one read instead of N).
    let peer_ctx = if group_id.is_empty() {
        let from_resolved = {
            let r = dm::resolve_member_ref(&idx, &from);
            if r.is_empty() {
                auth::normalize_email(&from)
            } else {
                r
            }
        };
        let from_norm_email = auth::normalize_email(&if from_resolved.is_empty() {
            from.clone()
        } else {
            from_resolved.clone()
        });
        let from_username = {
            let u = profiles
                .get(from_norm_email.as_str())
                .and_then(|p| p.get("username"))
                .filter(|u| truthy(u))
                .cloned()
                .unwrap_or_else(|| {
                    let du = default_username_for_email(&from_norm_email);
                    if !du.is_empty() {
                        json!(du)
                    } else {
                        json!(from)
                    }
                });
            normalize_username(&jsval::string(&u))
        };
        let my_username = {
            let u = profiles
                .get(my_norm.as_str())
                .and_then(|p| p.get("username"))
                .filter(|u| truthy(u))
                .cloned()
                .unwrap_or_else(|| json!(default_username_for_email(&my_norm)));
            normalize_username(&jsval::string(&u))
        };
        Some((from_norm_email, from_username, my_username))
    } else {
        None
    };
    let mut dms: Vec<Value> = state
        .store
        .read_document(&data_file(state, store.dms), json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut changed = false;
    let now = now_millis() as f64;
    for m in dms.iter_mut() {
        let mut marked = false;
        if !group_id.is_empty() {
            if string_prop(m.get("kind")) == "group"
                && js_eq(m.get("groupId").unwrap_or(&Value::Null), &json!(group_id))
                && !super::read_by_some(m, &my_norm)
            {
                let mut read_by = m
                    .get("readBy")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                read_by.push(json!(my_email));
                let read_at_falsy = m.get("readAt").map(|v| !truthy(v)).unwrap_or(true);
                if let Some(obj) = m.as_object_mut() {
                    obj.insert("readBy".into(), Value::Array(read_by));
                    if read_at_falsy {
                        obj.insert("readAt".into(), js_num_value(now));
                    }
                }
                marked = true;
                changed = true;
            }
        } else if let Some((from_norm_email, from_username, my_username)) = &peer_ctx {
            // isFromPeer / isToMe — username, masked email, and verbatim
            // fallbacks all match.
            let is_from_peer = |v: &Value| -> bool {
                if !truthy(v) {
                    return false;
                }
                let nv = norm_of(v);
                if nv == *from_norm_email || nv == auth::normalize_email(&from) {
                    return true;
                }
                !jsval::string(v).contains('@')
                    && !from_username.is_empty()
                    && normalize_username(&jsval::string(&jsval::or(Some(v), json!(""))))
                        == *from_username
            };
            let is_to_me = |v: &Value| -> bool {
                if !truthy(v) {
                    return false;
                }
                let nv = norm_of(v);
                if nv == my_norm || dm::resolve_member_ref(&idx, &jsval::string(v)) == my_norm {
                    return true;
                }
                !jsval::string(v).contains('@')
                    && !my_username.is_empty()
                    && normalize_username(&jsval::string(&jsval::or(Some(v), json!(""))))
                        == *my_username
            };
            let kind_dm = match m.get("kind") {
                None => true,
                Some(k) => !truthy(k) || jsval::string(k) == "dm",
            };
            let not_read = m.get("read").map(|v| !truthy(v)).unwrap_or(true);
            if kind_dm
                && m.get("to").map(is_to_me).unwrap_or(false)
                && (from.is_empty() || m.get("from").map(is_from_peer).unwrap_or(false))
                && not_read
            {
                let read_at_falsy = m.get("readAt").map(|v| !truthy(v)).unwrap_or(true);
                if let Some(obj) = m.as_object_mut() {
                    obj.insert("read".into(), Value::Bool(true));
                    if read_at_falsy {
                        obj.insert("readAt".into(), js_num_value(now));
                    }
                }
                marked = true;
                changed = true;
            }
        }
        if marked && dm::is_dm_message_read(m) {
            let exp = dm::get_expiry_for_msg(&state.store, &state.cfg.data_dir, m, store.pickle);
            let has_expires = m.get("expiresAt").map(truthy).unwrap_or(false);
            if exp > 0.0 && !has_expires {
                // Math.min(now + exp, (m.ts || now) + Math.max(exp, 3600000));
                // NaN propagates like JS Math.min (serializes as null).
                let ts_or_now = match m.get("ts") {
                    Some(v) if truthy(v) => jsval::number(v).unwrap_or(f64::NAN),
                    _ => now,
                };
                let a = now + exp;
                let b = ts_or_now + exp.max(3_600_000.0);
                let val = if a.is_nan() || b.is_nan() {
                    f64::NAN
                } else {
                    a.min(b)
                };
                if let Some(obj) = m.as_object_mut() {
                    obj.insert("expiresAt".into(), js_num_value(val));
                }
            }
        }
    }
    if changed {
        let pruned = dm::prune_dms(&state.store, &state.cfg.data_dir, &dms, store.pickle, now);
        state
            .store
            .write_document(&data_file(state, store.dms), &Value::Array(pruned))
            .ok();
    }
    // Reading a DM pays the sender — the coin reward for the message that
    // was just read.
    if !from.is_empty() {
        let r = dm::resolve_member_ref(&idx, &from);
        let target = if r.is_empty() { from.clone() } else { r };
        coins::add_coins(
            &state.store,
            &state.cfg.data_dir,
            &target,
            2.0,
            state.coin_multiplier(),
            "",
        );
    }
    json_response(200, json!({ "success": true }))
}

/// `/api/dm/group/leave` — remove the caller from a group (the group dies
/// with its last member).
pub(super) async fn group_leave(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Response {
    let auth = match dm_auth(state, headers) {
        Ok(a) => a,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    let body = match parse_body(headers, body_bytes) {
        Ok(b) => b,
        Err(()) => return json_response(400, json!({ "error": "bad json" })),
    };
    let group_id = jsval::string(&jsval::or(body.get("groupId"), json!("")));
    if group_id.is_empty() {
        return json_response(400, json!({ "error": "missing groupId" }));
    }
    let store = store_of(headers);
    let mut groups: Vec<Value> = state
        .store
        .read_document(&data_file(state, store.groups), json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let pos = groups
        .iter()
        .position(|g| js_eq(g.get("id").unwrap_or(&Value::Null), &json!(group_id)));
    let Some(pos) = pos else {
        return json_response(404, json!({ "error": "group not found" }));
    };
    let my_norm = auth::normalize_email(&auth.email);
    // A non-array members field would throw in JS (500); degrade to leaving
    // the group untouched (the save is a no-op round-trip).
    if let Some(members) = groups[pos]
        .get_mut("members")
        .and_then(|v| v.as_array_mut())
    {
        members.retain(|m| norm_of(m) != my_norm);
        if members.is_empty() {
            groups.remove(pos);
        }
    }
    state
        .store
        .write_document(&data_file(state, store.groups), &Value::Array(groups))
        .ok();
    json_response(200, json!({ "ok": true }))
}

/// `/api/dm/clear` — hide a conversation from this user's view only.
pub(super) async fn clear(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Response {
    let auth = match dm_auth(state, headers) {
        Ok(a) => a,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    let body = match parse_body(headers, body_bytes) {
        Ok(b) => b,
        Err(()) => return json_response(400, json!({ "error": "bad json" })),
    };
    let store = store_of(headers);
    let idx = dm::address_index_cached(&state.store, &state.cfg.data_dir);
    let norm = auth::normalize_email(&auth.email);
    // if (!cleared[norm]) cleared[norm] = {} — a truthy non-object entry
    // makes the JS property writes silent no-ops, mirrored by the
    // as_object_mut() guards below.
    let mut my_cleared = mitch_lib::dm::dm_cleared_get(&state.store, store, &norm);
    if !truthy(&my_cleared) {
        my_cleared = json!({});
    }
    let now = now_millis() as f64;
    let mut target_hit = true;
    if truthy(&jsval::or(body.get("all"), json!(false))) {
        let groups: Vec<Value> = state
            .store
            .read_document(&data_file(state, store.groups), json!([]))
            .as_array()
            .cloned()
            .unwrap_or_default();
        for g in &groups {
            let mine = g
                .get("members")
                .and_then(|v| v.as_array())
                .map(|members| members.iter().any(|m| norm_of(m) == norm))
                .unwrap_or(false);
            if mine {
                if let Some(entry) = my_cleared.as_object_mut() {
                    entry.insert(concat_key("group:", g.get("id")), js_num_value(now));
                }
            }
        }
        let dms: Vec<Value> = state
            .store
            .read_document(&data_file(state, store.dms), json!([]))
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut peers: Vec<String> = Vec::new();
        for m in &dms {
            let kind_dm = match m.get("kind") {
                None => true,
                Some(k) => !truthy(k) || jsval::string(k) == "dm",
            };
            if !kind_dm {
                continue;
            }
            if norm_of(&jsval::or(m.get("from"), json!(""))) == norm {
                let p = norm_of(&jsval::or(m.get("to"), json!("")));
                if !p.is_empty() && !peers.contains(&p) {
                    peers.push(p);
                }
            }
            if norm_of(&jsval::or(m.get("to"), json!(""))) == norm {
                let p = norm_of(&jsval::or(m.get("from"), json!("")));
                if !p.is_empty() && !peers.contains(&p) {
                    peers.push(p);
                }
            }
        }
        for peer in peers {
            if let Some(entry) = my_cleared.as_object_mut() {
                entry.insert(format!("dm:{peer}"), js_num_value(now));
            }
        }
    } else if let Some(gid) = body.get("groupId").filter(|v| truthy(v)) {
        if let Some(entry) = my_cleared.as_object_mut() {
            entry.insert(concat_key("group:", Some(gid)), js_num_value(now));
        }
    } else if let Some(with_v) = body.get("with").filter(|v| truthy(v)) {
        let with_raw = jsval::string(with_v);
        let with_res = {
            let r = dm::resolve_member_ref(&idx, &with_raw);
            if r.is_empty() {
                auth::normalize_email(&with_raw)
            } else {
                r
            }
        };
        if let Some(entry) = my_cleared.as_object_mut() {
            entry.insert(
                format!("dm:{}", auth::normalize_email(&with_raw)),
                js_num_value(now),
            );
            entry.insert(
                format!("dm:{}", auth::normalize_email(&with_res)),
                js_num_value(now),
            );
        }
    } else {
        target_hit = false;
    }
    if !target_hit {
        return json_response(400, json!({ "error": "missing target" }));
    }
    mitch_lib::dm::dm_cleared_set(&state.store, store, &norm, &my_cleared);
    json_response(200, json!({ "success": true }))
}

/// `/api/dm/expiry` — set the conversation-wide auto-delete window. Either
/// DM participant (or any group member) can change it; it applies to
/// messages from everyone in the conversation. The live `chat_expiry` WS
/// broadcast is wired below (server.js:20056-20062).
pub(super) async fn expiry(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Response {
    let auth = match dm_auth(state, headers) {
        Ok(a) => a,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    let body = match parse_body(headers, body_bytes) {
        Ok(b) => b,
        Err(()) => return json_response(400, json!({ "error": "bad json" })),
    };
    // Number(body.expiry) || 0 — NaN/undefined/'' all land on 0.
    let requested = match body.get("expiry") {
        Some(v) => jsval::number(v).unwrap_or(f64::NAN),
        None => f64::NAN,
    };
    let requested = if requested.is_nan() || requested == 0.0 {
        0.0
    } else {
        requested
    };
    if requested != 0.0 && !dm::CHAT_EXPIRY_OPTIONS.contains(&requested) {
        return json_response(400, json!({ "error": "invalid expiry" }));
    }
    let store = store_of(headers);
    let exp_pfx = if store.pickle { "spc:" } else { "" };
    let idx = dm::address_index_cached(&state.store, &state.cfg.data_dir);
    let norm = auth::normalize_email(&auth.email);
    // (group_id, with) mirror the JS wsGroupId/wsWith staging vars.
    let group_id;
    let with;
    let key;
    // JS wsTargets — the normalized-email set the `chat_expiry` broadcast
    // fans out to (server.js:20000-20020).
    let mut ws_targets: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Some(gid_v) = body.get("groupId").filter(|v| truthy(v)) {
        let gid = jsval::string(gid_v);
        let groups: Vec<Value> = state
            .store
            .read_document(&data_file(state, store.groups), json!([]))
            .as_array()
            .cloned()
            .unwrap_or_default();
        let Some(g) = groups
            .iter()
            .find(|g| js_eq(g.get("id").unwrap_or(&Value::Null), &json!(gid)))
        else {
            return json_response(404, json!({ "error": "group not found" }));
        };
        let member_norms: Vec<String> = g
            .get("members")
            .and_then(|v| v.as_array())
            .map(|members| members.iter().map(|m| member_norm(&idx, m)).collect())
            // A non-array members field would throw in JS (500); degrade to
            // an empty roster.
            .unwrap_or_default();
        if !member_norms.iter().any(|n| n == &norm) {
            return json_response(403, json!({ "error": "not a member" }));
        }
        key = format!("{}{}", exp_pfx, dm::group_expiry_key(&gid));
        group_id = Some(gid);
        with = None;
        ws_targets.extend(member_norms);
    } else if let Some(with_v) = body.get("with").filter(|v| truthy(v)) {
        // resolveMemberRef returns '' for anything that isn't a real member,
        // which doubles as the exists check.
        let peer_resolved = dm::resolve_member_ref(&idx, &jsval::string(with_v));
        if peer_resolved.is_empty() || peer_resolved == norm {
            return json_response(400, json!({ "error": "invalid peer" }));
        }
        group_id = None;
        with = Some(peer_resolved.clone());
        key = format!("{}{}", exp_pfx, dm::dm_expiry_key(&norm, &peer_resolved));
        ws_targets.insert(norm.clone());
        ws_targets.insert(peer_resolved.clone());
    } else {
        return json_response(400, json!({ "error": "missing target" }));
    }
    dm::set_chat_expiry(&state.store, &state.cfg.data_dir, &key, requested);
    let mut pruned_any = false;
    if requested > 0.0 {
        let dms: Vec<Value> = state
            .store
            .read_document(&data_file(state, store.dms), json!([]))
            .as_array()
            .cloned()
            .unwrap_or_default();
        let initial_len = dms.len();
        let now = now_millis() as f64;
        let ws_norm = with.as_ref().map(|w| auth::normalize_email(w));
        let resolve_of = |v: Option<&Value>| -> String {
            dm::resolve_member_ref(&idx, &jsval::string(&jsval::or(v, json!(""))))
        };
        let mut filtered: Vec<Value> = Vec::with_capacity(dms.len());
        for m in dms {
            let matches = if let Some(gid) = &group_id {
                // String(m.groupId) === String(wsGroupId) — STRING coercion
                // on both sides here (unlike the strict find above).
                string_prop(m.get("groupId")) == *gid
            } else if let Some(ws_norm) = ws_norm.as_ref() {
                let is_me_m = |v: Option<&Value>| -> bool {
                    norm_of(&jsval::or(v, json!(""))) == norm || resolve_of(v) == norm
                };
                let is_peer_m = |v: Option<&Value>| -> bool {
                    norm_of(&jsval::or(v, json!(""))) == *ws_norm || resolve_of(v) == *ws_norm
                };
                string_prop(m.get("kind")) != "group"
                    && ((is_me_m(m.get("from")) && is_peer_m(m.get("to")))
                        || (is_peer_m(m.get("from")) && is_me_m(m.get("to"))))
            } else {
                false
            };
            if !matches {
                filtered.push(m);
                continue;
            }
            if dm::is_dm_message_read(&m) {
                // (m.ts || 0) — a truthy unparseable ts poisons the arithmetic
                // to NaN exactly like JS, so the drop conditions go false.
                let ts0 = match m.get("ts") {
                    Some(v) if truthy(v) => jsval::number(v).unwrap_or(f64::NAN),
                    _ => 0.0,
                };
                let read_at_old = m
                    .get("readAt")
                    .filter(|v| truthy(v))
                    .and_then(jsval::number)
                    .map(|r| now - r >= requested);
                if (now - ts0 >= 3_600_000.0)
                    || (now - ts0 >= requested)
                    || read_at_old.unwrap_or(false)
                {
                    continue; // delete old read message!
                }
                let mut m = m;
                // Math.min(now + requested, (m.ts || now) + Math.max(requested, 3600000))
                let ts_or_now = match m.get("ts") {
                    Some(v) if truthy(v) => jsval::number(v).unwrap_or(f64::NAN),
                    _ => now,
                };
                let a = now + requested;
                let b = ts_or_now + requested.max(3_600_000.0);
                let val = if a.is_nan() || b.is_nan() {
                    f64::NAN
                } else {
                    a.min(b)
                };
                if let Some(obj) = m.as_object_mut() {
                    obj.insert("expiresAt".into(), js_num_value(val));
                }
                filtered.push(m);
                continue;
            }
            filtered.push(m);
        }
        if filtered.len() != initial_len {
            let pruned = dm::prune_dms(
                &state.store,
                &state.cfg.data_dir,
                &filtered,
                store.pickle,
                now,
            );
            state
                .store
                .write_document(&data_file(state, store.dms), &Value::Array(pruned))
                .ok();
            pruned_any = true;
        }
    }
    // The live `chat_expiry` WS broadcast (server.js:20056-20062). JS
    // JSON.stringify drops undefined groupId/with keys, so build the map
    // conditionally.
    {
        let mut payload = serde_json::Map::new();
        payload.insert("type".into(), json!("chat_expiry"));
        payload.insert("key".into(), json!(key));
        payload.insert("expiry".into(), js_num_value(requested));
        if let Some(gid) = &group_id {
            payload.insert("groupId".into(), json!(gid));
        }
        if let Some(w) = &with {
            payload.insert("with".into(), json!(w));
        }
        payload.insert("reload".into(), json!(pruned_any));
        crate::ws::broadcast(
            state,
            crate::ws::WsRecipients::Emails(ws_targets),
            Value::Object(payload).to_string(),
        );
    }
    json_response(
        200,
        json!({
            "success": true,
            "expiry": js_num_value(requested),
            "pruned": pruned_any,
        }),
    )
}

/// `/api/dm/report` — report a message for admin review (reCAPTCHA-gated).
pub(super) async fn report(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Response {
    let auth = match dm_auth(state, headers) {
        Ok(a) => a,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    let body = match parse_body(headers, body_bytes) {
        Ok(b) => b,
        Err(()) => return json_response(400, json!({ "error": "bad json" })),
    };
    let ip = get_real_ip(headers, None);
    let token = jsval::string(&jsval::or(body.get("recaptcha_token"), json!("")));
    if !verify_recaptcha(state, &token, &ip, &auth.sid).await {
        return json_response(
            400,
            json!({ "error": "reCAPTCHA failed. Please try again." }),
        );
    }
    let mut reports: Vec<Value> = state
        .store
        .read_document(&data_file(state, "chat_reports.json"), json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let context = match body.get("context").and_then(|v| v.as_array()) {
        Some(arr) => json!(arr
            .iter()
            .map(|m| {
                let ts = jsval::number(&jsval::or(m.get("ts"), json!(0))).unwrap_or(f64::NAN);
                json!({
                    "from": jsval::string(&jsval::or(m.get("from"), json!(""))),
                    "to": jsval::string(&jsval::or(m.get("to"), json!(""))),
                    "text": jsval::js_slice_utf16(&jsval::string(&jsval::or(m.get("text"), json!(""))), 2000),
                    "ts": if ts.is_nan() || ts == 0.0 { json!(0) } else { js_num_value(ts) },
                    "reported": m.get("reported").map(truthy).unwrap_or(false),
                })
            })
            .collect::<Vec<_>>()),
        None => json!([]),
    };
    let new_report = json!({
        "id": jsval::string(&jsval::or(body.get("id"), json!(""))),
        "reason": jsval::js_slice_utf16(&jsval::string(&jsval::or(body.get("reason"), json!(""))), 500),
        "reportedBy": auth.email,
        "ts": now_millis(),
        "context": context,
    });
    reports.push(new_report);
    if reports.len() > 5000 {
        let excess = reports.len() - 5000;
        reports.drain(0..excess);
    }
    state
        .store
        .write_document(
            &data_file(state, "chat_reports.json"),
            &Value::Array(reports),
        )
        .ok();
    json_response(200, json!({ "success": true }))
}
