//! `GET /api/dm/groups` + `/api/dm/inbox` (server.js:19668-19838).
//! The ref-matching helpers (`isMeRef`/`isPeerRef`) accept usernames, masked
//! emails, and display emails so older messages addressed verbatim still
//! match — every normalization ladder here is load-bearing.

use super::{
    cleared_lookup, concat_key, dm_auth, js_eq, js_num_value, member_norm, norm_of, qs_get,
    read_by_some, string_prop,
};
use crate::hosts::is_pickle_host;
use crate::routes::me::{data_file, dm_content_parts, json_response};
use crate::state::AppState;
use axum::http::HeaderMap;
use axum::response::Response;
use mitch_lib::admin::mask_email;
use mitch_lib::auth;
use mitch_lib::dm::{DMS_MAIN, DMS_PICKLE};
use mitch_lib::jsval::{self, truthy};
use mitch_lib::profile::{default_username_for_email, display_email, normalize_username};
use serde_json::{json, Map, Value};
use std::sync::Arc;

/// `(m.ts || 0)` — a truthy unparseable ts is NaN in JS arithmetic (falsy
/// `||` hands it straight through), so NaN — not 0 — is the right default.
fn js_ts(m: &Value) -> f64 {
    match m.get("ts") {
        Some(v) if truthy(v) => jsval::number(v).unwrap_or(f64::NAN),
        _ => 0.0,
    }
}

/// `normalizeUsername(v)`.
fn norm_username(v: &Value) -> String {
    normalize_username(&jsval::string(&jsval::or(Some(v), json!(""))))
}

/// `displayEmail(x)` where x is a raw JSON value.
fn display_of(state: &AppState, v: &Value) -> String {
    display_email(
        &state.store,
        &state.cfg.data_dir,
        &state.id_secret,
        &jsval::string(&jsval::or(Some(v), json!(""))),
    )
}

/// `GET /api/dm/groups` — groups the caller belongs to, newest activity
/// first, members as public username refs.
pub(super) fn groups_get(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let auth = match dm_auth(state, headers) {
        Ok(a) => a,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    groups_inner(state, &auth, headers)
}

fn groups_inner(state: &Arc<AppState>, auth: &super::DmAuth, headers: &HeaderMap) -> Response {
    let store = if is_pickle_host(headers) {
        &DMS_PICKLE
    } else {
        &DMS_MAIN
    };
    let idx = mitch_lib::dm::address_index_cached(&state.store, &state.cfg.data_dir);
    let my_norm = auth::normalize_email(&auth.email);
    let all_groups: Vec<Value> = state
        .store
        .read_document(&data_file(state, store.groups), json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let groups: Vec<Value> = all_groups
        .into_iter()
        .filter(|g| {
            g.get("members")
                .and_then(|v| v.as_array())
                .map(|members| members.iter().any(|m| member_norm(&idx, m) == my_norm))
                // A truthy non-array members field would throw in JS (500);
                // here it degrades to "not a member".
                .unwrap_or(false)
        })
        .collect();
    let profiles = state
        .store
        .read_document(&data_file(state, "profiles.json"), json!({}));
    let dms: Vec<Value> = state
        .store
        .read_document(&data_file(state, store.dms), json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let my_cleared = mitch_lib::dm::dm_cleared_get(&state.store, store, &my_norm);
    let mut result: Vec<Value> = groups
        .iter()
        .map(|g| {
            let cleared_at =
                cleared_lookup(&my_cleared, &concat_key("group:", g.get("id"))).unwrap_or(0.0);
            // Sort ts desc — JS sort with the same comparator is stable.
            let mut msgs: Vec<&Value> = dms
                .iter()
                .filter(|m| {
                    string_prop(m.get("kind")) == "group"
                        && js_eq(
                            m.get("groupId").unwrap_or(&Value::Null),
                            g.get("id").unwrap_or(&Value::Null),
                        )
                        && js_ts(m) > cleared_at
                })
                .collect();
            msgs.sort_by(|a, b| {
                js_ts(b)
                    .partial_cmp(&js_ts(a))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let last = msgs.first().copied();
            let unread = msgs.iter().filter(|m| !read_by_some(m, &my_norm)).count();
            let (last_text, last_message) = match last {
                Some(last) => {
                    let c = dm_content_parts(last, &state.id_secret);
                    let last_text = if c.0.as_ref().map(truthy).unwrap_or(false) {
                        // lastText keeps the raw truthy value (JS `text || …`).
                        c.0.clone().unwrap_or(json!(""))
                    } else if c.1.as_ref().map(truthy).unwrap_or(false) {
                        json!("Sent an image")
                    } else {
                        json!("")
                    };
                    (last_text, Some(masked_like(state, last, &c)))
                }
                None => (json!(""), None),
            };
            let mut out = g.as_object().cloned().unwrap_or_default();
            // publicRef: the client encrypts one group payload per member and
            // keys it by this ref — usernames, not masked emails.
            let members = g
                .get("members")
                .and_then(|v| v.as_array())
                .map(|members| {
                    json!(members
                        .iter()
                        .map(|m| {
                            let r = mitch_lib::dm::resolve_member_ref(
                                &idx,
                                &jsval::string(&jsval::or(Some(m), json!(""))),
                            );
                            if r.is_empty() {
                                return json!(display_of(state, m));
                            }
                            let p = profiles.get(r.as_str()).cloned().unwrap_or(json!({}));
                            match p.get("username").filter(|u| truthy(u)) {
                                Some(u) => u.clone(),
                                None => json!(default_username_for_email(&r)),
                            }
                        })
                        .collect::<Vec<_>>())
                })
                .unwrap_or_else(|| json!([]));
            out.insert("members".into(), members);
            out.insert(
                "createdBy".into(),
                json!(display_of(state, &jsval::or(g.get("createdBy"), json!("")))),
            );
            out.insert("lastText".into(), last_text);
            // lastTs: last ? last.ts : g.createdAt — a present-but-null ts is
            // kept (JSON null); an absent one is dropped (undefined).
            match last {
                Some(l) => {
                    if let Some(ts) = l.get("ts") {
                        out.insert("lastTs".into(), ts.clone());
                    }
                }
                None => {
                    if let Some(ca) = g.get("createdAt") {
                        out.insert("lastTs".into(), ca.clone());
                    }
                }
            }
            match last_message {
                Some(lm) => out.insert("lastMessage".into(), lm),
                None => out.insert("lastMessage".into(), json!(null)),
            };
            out.insert("unread".into(), json!(unread));
            Value::Object(out)
        })
        .collect();
    // result.sort((a, b) => (b.lastTs || 0) - (a.lastTs || 0)) — stable, and
    // a NaN lastTs compares equal (order preserved).
    let last_ts_of = |v: &Value| -> f64 {
        match v.get("lastTs") {
            Some(t) if truthy(t) => jsval::number(t).unwrap_or(f64::NAN),
            _ => 0.0,
        }
    };
    result.sort_by(|a, b| {
        last_ts_of(b)
            .partial_cmp(&last_ts_of(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    json_response(200, json!({ "groups": result }))
}

/// `{...last, text, image, from, to, readBy}` with the JS undefined-drops.
fn masked_like(state: &AppState, m: &Value, c: &(Option<Value>, Option<Value>)) -> Value {
    let mut out = m.as_object().cloned().unwrap_or_default();
    if let Some(t) = &c.0 {
        out.insert("text".into(), t.clone());
    }
    // image: c.image !== undefined ? c.image : m.image — dm_content_parts
    // already resolves the fallback; absent means drop the key.
    if let Some(img) = &c.1 {
        out.insert("image".into(), img.clone());
    }
    out.insert(
        "from".into(),
        json!(display_of(state, &jsval::or(m.get("from"), json!("")))),
    );
    // to: last.to ? displayEmail(last.to) : undefined — falsy drops.
    if let Some(to_v) = m.get("to").filter(|v| truthy(v)) {
        out.insert("to".into(), json!(display_of(state, to_v)));
    }
    // (last.readBy || []).map(displayEmail) — non-array would throw in JS.
    out.insert(
        "readBy".into(),
        json!(m
            .get("readBy")
            .and_then(|v| v.as_array())
            .map(|entries| entries
                .iter()
                .map(|v| json!(display_of(state, &jsval::or(Some(v), json!("")))))
                .collect::<Vec<_>>())
            .unwrap_or_default()),
    );
    Value::Object(out)
}

/// `/api/dm/inbox` — a conversation (`with`/`group`) or the general inbox,
/// with the conversation's auto-delete window attached. No method check in
/// the JS: any verb runs the ladder.
pub(super) fn inbox(state: &Arc<AppState>, headers: &HeaderMap, search: &str) -> Response {
    let auth = match dm_auth(state, headers) {
        Ok(a) => a,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    inbox_inner(state, &auth, headers, search)
}

fn inbox_inner(
    state: &Arc<AppState>,
    auth: &super::DmAuth,
    headers: &HeaderMap,
    search: &str,
) -> Response {
    let store = if is_pickle_host(headers) {
        &DMS_PICKLE
    } else {
        &DMS_MAIN
    };
    let exp_pfx = if store.pickle { "spc:" } else { "" };
    let idx = mitch_lib::dm::address_index_cached(&state.store, &state.cfg.data_dir);
    // withUser = (qs.get('with') || '').toLowerCase() — no trim.
    let with_user = qs_get(search, "with").unwrap_or_default().to_lowercase();
    let with_resolved = {
        let r = mitch_lib::dm::resolve_member_ref(&idx, &with_user);
        if r.is_empty() {
            auth::normalize_email(&with_user)
        } else {
            r
        }
    };
    let group_id = qs_get(search, "group").unwrap_or_default();
    // parseInt(qs.get(x) || '0') || 0 — decimal prefix parse, NaN → 0.
    let since = parse_int_or_zero(&qs_get(search, "since").unwrap_or_default());
    let before = parse_int_or_zero(&qs_get(search, "before").unwrap_or_default());
    let limit = parse_int_or_zero(&qs_get(search, "limit").unwrap_or_default());

    let dms: Vec<Value> = state
        .store
        .read_document(&data_file(state, store.dms), json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let my_norm = auth::normalize_email(&auth.email);
    let my_cleared = mitch_lib::dm::dm_cleared_get(&state.store, store, &my_norm);
    let groups: Vec<Value> = state
        .store
        .read_document(&data_file(state, store.groups), json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let my_groups: Vec<&Value> = groups
        .iter()
        .filter(|g| {
            g.get("members")
                .and_then(|v| v.as_array())
                .map(|members| members.iter().any(|m| member_norm(&idx, m) == my_norm))
                .unwrap_or(false)
        })
        .collect();
    // Set of raw ids — Set.has() uses SameValueZero, so a numeric m.groupId
    // never matches a string group id.
    let my_group_ids: Vec<Value> = my_groups
        .iter()
        .filter_map(|g| g.get("id").cloned())
        .collect();
    let profiles = state
        .store
        .read_document(&data_file(state, "profiles.json"), json!({}));
    let my_masked = mask_email(&auth.email);
    let my_username = {
        let u = profiles
            .get(my_norm.as_str())
            .and_then(|p| p.get("username"))
            .filter(|u| truthy(u))
            .cloned()
            .unwrap_or_else(|| json!(default_username_for_email(&my_norm)));
        normalize_username(&jsval::string(&u))
    };
    let my_display = auth::normalize_email(&display_email(
        &state.store,
        &state.cfg.data_dir,
        &state.id_secret,
        &my_norm,
    ));
    let is_me_ref = |v: &Value| -> bool {
        if !truthy(v) {
            return false;
        }
        let nv = norm_of(v);
        if nv == my_norm || nv == my_masked {
            return true;
        }
        if !my_display.is_empty() && nv == my_display {
            return true;
        }
        !jsval::string(v).contains('@') && norm_username(v) == my_username
    };
    let with_norm = auth::normalize_email(&if with_resolved.is_empty() {
        with_user.clone()
    } else {
        with_resolved.clone()
    });
    let with_masked = mask_email(&with_norm);
    let with_display = auth::normalize_email(&display_email(
        &state.store,
        &state.cfg.data_dir,
        &state.id_secret,
        &with_norm,
    ));
    let with_username = {
        let u = profiles
            .get(with_norm.as_str())
            .and_then(|p| p.get("username"))
            .filter(|u| truthy(u))
            .cloned()
            .unwrap_or_else(|| {
                let du = default_username_for_email(&with_norm);
                if !du.is_empty() {
                    json!(du)
                } else if !with_user.contains('@') {
                    json!(with_user)
                } else {
                    json!("")
                }
            });
        normalize_username(&jsval::string(&u))
    };
    let is_peer_ref = |v: &Value| -> bool {
        if !truthy(v) {
            return false;
        }
        let nv = norm_of(v);
        if nv == with_norm
            || (!with_resolved.is_empty() && nv == auth::normalize_email(&with_resolved))
        {
            return true;
        }
        if nv == auth::normalize_email(&with_user) {
            return true;
        }
        if !with_masked.is_empty() && nv == auth::normalize_email(&with_masked) {
            return true;
        }
        if !with_display.is_empty() && nv == with_display {
            return true;
        }
        !jsval::string(v).contains('@')
            && !with_username.is_empty()
            && norm_username(v) == with_username
    };
    let now = mitch_lib::school::now_millis() as f64;
    let resolve_of = |v: Option<&Value>| -> String { member_norm(&idx, v.unwrap_or(&Value::Null)) };
    let mut msgs: Vec<Value> = dms
        .iter()
        .filter(|m| {
            if mitch_lib::dm::is_message_expired(
                &state.store,
                &state.cfg.data_dir,
                m,
                store.pickle,
                now,
            ) {
                return false;
            }
            let ts = js_ts(m);
            if since != 0.0 && ts <= since {
                return false;
            }
            if !group_id.is_empty() {
                if string_prop(m.get("kind")) != "group"
                    || string_prop(m.get("groupId")) != group_id
                {
                    return false;
                }
                if !my_group_ids.iter().any(|g| js_eq(g, &json!(group_id))) {
                    return false;
                }
                let cleared_at =
                    cleared_lookup(&my_cleared, &format!("group:{group_id}")).unwrap_or(0.0);
                return ts > cleared_at;
            }
            if !with_user.is_empty() {
                if string_prop(m.get("kind")) == "group" {
                    return false;
                }
                let cleared_at = {
                    let mut at = cleared_lookup(&my_cleared, &format!("dm:{with_norm}"));
                    if at.is_none() {
                        at = cleared_lookup(
                            &my_cleared,
                            &format!("dm:{}", auth::normalize_email(&with_user)),
                        );
                    }
                    if at.is_none() && !with_username.is_empty() {
                        at = cleared_lookup(&my_cleared, &format!("dm:{with_username}"));
                    }
                    at.unwrap_or(0.0)
                };
                let from_me = m.get("from").map(&is_me_ref).unwrap_or(false);
                let to_me = m.get("to").map(&is_me_ref).unwrap_or(false);
                let from_peer = m.get("from").map(&is_peer_ref).unwrap_or(false);
                let to_peer = m.get("to").map(&is_peer_ref).unwrap_or(false);
                return ((from_me && to_peer) || (from_peer && to_me)) && ts > cleared_at;
            }
            // general inbox: my DMs + group messages for my groups
            if string_prop(m.get("kind")) == "group" {
                let gid = m.get("groupId").cloned().unwrap_or(Value::Null);
                if !my_group_ids.iter().any(|g| js_eq(g, &gid)) {
                    return false;
                }
                let cleared_at =
                    cleared_lookup(&my_cleared, &concat_key("group:", Some(&gid))).unwrap_or(0.0);
                return ts > cleared_at;
            }
            let from_me = m.get("from").map(&is_me_ref).unwrap_or(false);
            let peer = if from_me {
                resolve_of(m.get("to"))
            } else {
                resolve_of(m.get("from"))
            };
            let cleared_at = {
                let mut at = cleared_lookup(&my_cleared, &format!("dm:{peer}"));
                if at.is_none() {
                    at = cleared_lookup(
                        &my_cleared,
                        &format!("dm:{}", m.get("from").map(norm_of).unwrap_or_default()),
                    );
                }
                if at.is_none() {
                    at = cleared_lookup(
                        &my_cleared,
                        &format!("dm:{}", m.get("to").map(norm_of).unwrap_or_default()),
                    );
                }
                at.unwrap_or(0.0)
            };
            let to_me = m.get("to").map(&is_me_ref).unwrap_or(false);
            (from_me || to_me) && ts > cleared_at
        })
        .cloned()
        .collect();
    // Sort chronologically (stable; NaN ts compares equal).
    msgs.sort_by(|a, b| {
        js_ts(a)
            .partial_cmp(&js_ts(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if before != 0.0 {
        msgs.retain(|m| js_ts(m) < before);
    }
    if limit > 0.0 {
        let lim = limit as usize;
        if msgs.len() > lim {
            msgs = msgs.split_off(msgs.len() - lim);
        }
    }
    let result_msgs: Vec<Value> = msgs
        .iter()
        .map(|m| {
            let c = dm_content_parts(m, &state.id_secret);
            let mut out = m.as_object().cloned().unwrap_or_default();
            if let Some(t) = &c.0 {
                out.insert("text".into(), t.clone());
            }
            if let Some(img) = &c.1 {
                out.insert("image".into(), img.clone());
            }
            out.insert(
                "from".into(),
                json!(display_of(state, &jsval::or(m.get("from"), json!("")))),
            );
            // NOTE: inbox maps to: displayEmail(m.to) UNGATED — a group
            // message yields "to": "" here (displayEmail(undefined)).
            out.insert(
                "to".into(),
                json!(display_of(state, &jsval::or(m.get("to"), json!("")))),
            );
            out.insert(
                "readBy".into(),
                json!(m
                    .get("readBy")
                    .and_then(|v| v.as_array())
                    .map(|entries| entries
                        .iter()
                        .map(|v| json!(display_of(state, &jsval::or(Some(v), json!("")))))
                        .collect::<Vec<_>>())
                    .unwrap_or_default()),
            );
            Value::Object(out)
        })
        .collect();
    // The conversation's auto-delete window so both sides render the same
    // toggle state; `undefined` (no `with`) drops the key in JSON.stringify.
    let expiry = if !group_id.is_empty() {
        Some(mitch_lib::dm::get_chat_expiry(
            &state.store,
            &state.cfg.data_dir,
            &format!("{}{}", exp_pfx, mitch_lib::dm::group_expiry_key(&group_id)),
        ))
    } else if !with_resolved.is_empty() {
        Some(mitch_lib::dm::get_chat_expiry(
            &state.store,
            &state.cfg.data_dir,
            &format!(
                "{}{}",
                exp_pfx,
                mitch_lib::dm::dm_expiry_key(&my_norm, &with_resolved)
            ),
        ))
    } else {
        None
    };
    let mut resp = Map::new();
    resp.insert("messages".into(), json!(result_msgs));
    resp.insert("myEmail".into(), json!(mask_email(&auth.email)));
    if let Some(e) = expiry {
        resp.insert("expiry".into(), js_num_value(e));
    }
    json_response(200, Value::Object(resp))
}

/// `parseInt(s, 10) || 0` — decimal prefix parse, NaN → 0.
fn parse_int_or_zero(s: &str) -> f64 {
    let t = s.trim_start();
    let bytes = t.as_bytes();
    let mut idx = 0;
    let mut neg = false;
    if idx < bytes.len() && (bytes[idx] == b'+' || bytes[idx] == b'-') {
        neg = bytes[idx] == b'-';
        idx += 1;
    }
    let start = idx;
    while idx < bytes.len() && bytes[idx].is_ascii_digit() {
        idx += 1;
    }
    if idx == start {
        return 0.0;
    }
    let mut n: f64 = 0.0;
    for c in t[start..idx].bytes() {
        n = n * 10.0 + f64::from(c - b'0');
    }
    if neg {
        -n
    } else {
        n
    }
}
