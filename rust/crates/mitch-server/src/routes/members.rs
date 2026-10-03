//! Member directory + userdata storage (plan Step 9 batch 6).
//!
//! - `/api/members` — server.js:19113-19275: the full member roster with DM
//!   previews, friend status, presence and E2E keys.
//! - `/api/admin-members`, `/api/moderator-members`, `/api/owner-members` —
//!   server.js:19281-19400: the public role rosters.
//! - `/api/userdata` — server.js:17262-17317: per-sid JSON blob storage under
//!   `USERDATA_DIR` with a 1 GiB quota and newsletter-unsub syncing.
//!
//! JS quirk preserved: the `/api/userdata` branch checks the path only, so
//! every method (GET included) lands in it; the later GET-specific branch at
//! server.js:19048 is unreachable dead code.

use super::friends::{encode_uri_component, is_user_present};
use super::me::{
    cookies_of, data_file, dm_content_parts, json_response, me_uid, parse_body_strict,
};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::{admin, auth, crypto, e2e, jsval, profile, shop};
use serde_json::{json, Map, Value};
use std::sync::Arc;

/// `TEST_ACCOUNT_EMAIL` (server.js:438).
const TEST_ACCOUNT_EMAIL: &str = "tingtongsuperman@linux.com";
/// `USERDATA_DIR` (server.js:427).
const USERDATA_DIR: &str = "/opt/userdata";
/// `QUOTA` (server.js:17315) — 1 GiB of serialized JSON.
const USERDATA_QUOTA: usize = 1_073_741_824;

pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Option<Response> {
    match (method.as_str(), path) {
        // The JS branches check the path only — every method lands here.
        (_, "/api/userdata") => Some(userdata(state, headers, body_bytes)),
        (_, "/api/members") => Some(members(state, headers)),
        (_, "/api/premium-members") => Some(premium_members(state, headers)),
        (_, "/api/admin-members") => Some(admin_members(state, headers)),
        (_, "/api/moderator-members") => Some(moderator_members(state, headers)),
        (_, "/api/owner-members") => Some(owner_members(state, headers)),
        _ => None,
    }
}

// ── /api/members ─────────────────────────────────────────────────────────────

/// `server.js:19113-19275`.
fn members(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let Some((sid, viewer_email)) = member_gate(state, headers) else {
        return json_response(401, json!({ "error": "auth required" }));
    };
    let node_env_test = std::env::var("NODE_ENV").unwrap_or_default() == "test";
    let is_admin = auth::is_admin_id(&state.store, &state.id_secret, &sid, node_env_test);
    let now = mitch_lib::school::now_millis();
    let viewer_norm = auth::normalize_email(&viewer_email);
    touch_active_email(state, &viewer_email, now);

    let tokens = state
        .store
        .read_document(&data_file(state, "tokens.json"), json!({}));
    let profiles = state
        .store
        .read_document(&data_file(state, "profiles.json"), json!({}));
    let cosmetics = state
        .store
        .read_document(&data_file(state, "cosmetics.json"), json!({}));
    let dms = state
        .store
        .read_document(&data_file(state, "dms.json"), json!([]));
    let cleared = state
        .store
        .read_document(&data_file(state, "dm_cleared.json"), json!({}));
    let my_cleared = cleared
        .get(viewer_norm.as_str())
        .cloned()
        .unwrap_or(json!({}));
    let e2e_keys = state
        .store
        .read_document(&data_file(state, "e2e_keys.json"), json!({}));
    let friends = state
        .store
        .read_document(&data_file(state, "friends.json"), json!({}));
    let my_friends: Vec<String> = friends
        .get(viewer_norm.as_str())
        .and_then(|v| v.as_array())
        .map(|a| a.iter().map(jsval::string).collect())
        .unwrap_or_default();
    let friend_requests = state
        .store
        .read_document(&data_file(state, "friend_requests.json"), json!([]));
    let presence_map = state
        .user_presence
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();

    let mut seen: Vec<String> = Vec::new();
    let mut out: Vec<Value> = Vec::new();
    let Some(token_map) = tokens.as_object() else {
        return json_response(200, json!({ "members": [] }));
    };
    for (tok, data) in token_map {
        let email = jsval::string(&jsval::or(data.get("email"), json!("")))
            .to_lowercase()
            .trim()
            .to_string();
        if email.is_empty() || seen.contains(&email) || is_revoked_id(state, tok) {
            continue;
        }
        if email == TEST_ACCOUNT_EMAIL && !is_admin {
            continue;
        }
        seen.push(email.clone());
        let norm = auth::normalize_email(&email);
        let empty = json!({});
        let prof = profiles
            .get(norm.as_str())
            .cloned()
            .unwrap_or(empty.clone());
        let cosm = cosmetics.get(norm.as_str()).cloned().unwrap_or(empty);
        let username = profile::normalize_username(&jsval::string(&jsval::or(
            prof.get("username"),
            json!(profile::default_username_for_email(&norm)),
        )));
        let role = member_role(state, &email);

        // `e2eUsers[norm]?.pub_key` — the live WS chat-key leg arrives with the
        // Step 11 WS work; here the map is empty, so the stored/derived key wins.
        let e2e_entry = e2e_keys.get(norm.as_str()).filter(|v| jsval::truthy(v));
        let entry_pub = e2e_entry.and_then(|e| e.get("pubKeyHex")).cloned();
        let pub_key = match e2e_entry {
            Some(_) => entry_pub, // undefined when pubKeyHex is missing → key dropped
            None => Some(json!(e2e::derive_user_e2e_keys(&state.id_secret, &email).1)),
        };
        let e2e_ready = e2e_entry
            .map(|e| jsval::truthy(e.get("pubKeyHex").unwrap_or(&Value::Null)))
            .unwrap_or(false);

        let processed = profile::process_member_fields(
            &state.store,
            state.data_dir(),
            &email,
            Some(&prof),
            Some(&viewer_email),
        );

        // Find last message and unread count (server.js:19156-19206).
        let cleared_at = jsval::number(&jsval::or(
            my_cleared.get(format!("dm:{norm}")),
            jsval::or(my_cleared.get(format!("dm:{username}")), json!(0)),
        ))
        .unwrap_or(f64::NAN);
        let empty_arr: Vec<Value> = Vec::new();
        let dms_arr = dms.as_array().unwrap_or(&empty_arr);
        let mut member_dms: Vec<&Value> = Vec::new();
        for m in dms_arr {
            if m.get("kind").and_then(|v| v.as_str()) == Some("group") {
                continue;
            }
            let from = jsval::string(&jsval::or(m.get("from"), json!("")));
            let to = jsval::string(&jsval::or(m.get("to"), json!("")));
            let in_conv = (auth::normalize_email(&from) == viewer_norm
                && auth::normalize_email(&to) == norm)
                || (auth::normalize_email(&from) == norm
                    && auth::normalize_email(&to) == viewer_norm)
                || (auth::normalize_email(&from) == viewer_norm
                    && profile::normalize_username(&to) == username)
                || (profile::normalize_username(&from) == username
                    && auth::normalize_email(&to) == viewer_norm);
            if !in_conv {
                continue;
            }
            let ts = jsval::number(&jsval::or(m.get("ts"), json!(0))).unwrap_or(f64::NAN);
            // JS `ts > clearedAt` — NaN comparisons are false (skip).
            if !matches!(
                ts.partial_cmp(&cleared_at),
                Some(std::cmp::Ordering::Greater)
            ) {
                continue;
            }
            member_dms.push(m);
        }
        let unread = member_dms
            .iter()
            .filter(|m| {
                auth::normalize_email(&jsval::string(&jsval::or(m.get("to"), json!(""))))
                    == viewer_norm
                    && !m.get("read").is_some_and(jsval::truthy)
            })
            .count();

        let mut member = Map::new();
        member.insert(
            "email".into(),
            processed.get("email").cloned().unwrap_or(Value::Null),
        );
        member.insert("handle".into(), json!(username));
        member.insert(
            "profileUrl".into(),
            json!(format!("/profile/?u={}", encode_uri_component(&username))),
        );
        member.insert(
            "pfp".into(),
            json!(profile::sanitize_profile_image_url(
                &jsval::string(&jsval::or(prof.get("pfp"), json!(""))),
                true,
                1000,
                120_000,
            )),
        );
        let member_online = is_user_present(state, &email, now);
        member.insert("online".into(), json!(member_online));
        member.insert(
            "playing".into(),
            json!(if member_online {
                presence_map
                    .get(&norm)
                    .map(|p| p.playing.clone())
                    .unwrap_or_default()
            } else {
                String::new()
            }),
        );
        member.insert("role".into(), json!(role));
        member.insert(
            "displayName".into(),
            processed.get("displayName").cloned().unwrap_or(Value::Null),
        );
        member.insert(
            "bio".into(),
            json!(jsval::js_slice_utf16(
                &jsval::string(&jsval::or(prof.get("bio"), json!(""))),
                160,
            )),
        );
        member.insert(
            "color".into(),
            shop::public_active_color(
                &state.store,
                &email,
                cosm.get("activeColor").unwrap_or(&Value::Null),
            )
            .map(|c| json!(c))
            .unwrap_or(Value::Null),
        );
        member.insert(
            "badge".into(),
            jsval::or(cosm.get("activeBadge"), json!(Value::Null)),
        );

        let is_self = viewer_norm == norm;
        let is_friend = my_friends.iter().any(|f| auth::normalize_email(f) == norm);
        let req_arr = friend_requests.as_array();
        let incoming =
            req_arr.is_some_and(|a| a.iter().any(|r| member_req_between(r, &norm, &viewer_norm)));
        let outgoing =
            req_arr.is_some_and(|a| a.iter().any(|r| member_req_between(r, &viewer_norm, &norm)));
        let friend_status = if is_self {
            "self"
        } else if is_friend {
            "friends"
        } else if incoming {
            "incoming"
        } else if outgoing {
            "outgoing"
        } else {
            "none"
        };
        member.insert("friendStatus".into(), json!(friend_status));
        // `pubKey`/`lastMessage` are JS `undefined` when unset → key dropped.
        if let Some(pk) = pub_key {
            member.insert("pubKey".into(), pk);
        }
        member.insert(
            "legacyPubKey".into(),
            json!(e2e::derive_user_e2e_keys(&state.id_secret, &email).1),
        );
        member.insert("e2eReady".into(), json!(e2e_ready));
        let last_message = last_message_of(state, &member_dms);
        match last_message {
            Some(lm) => {
                member.insert("lastMessage".into(), lm);
            }
            None => {
                member.insert("lastMessage".into(), Value::Null);
            }
        }
        member.insert("unread".into(), json!(unread));
        out.push(Value::Object(member));
    }

    // server.js:19237-19257 — ts desc, role rank desc, online desc, email asc.
    out.sort_by(|a, b| {
        let ts = |v: &Value| -> f64 {
            match v.get("lastMessage") {
                Some(lm) if jsval::truthy(lm) => {
                    jsval::number(&jsval::or(lm.get("ts"), json!(0))).unwrap_or(f64::NAN)
                }
                _ => 0.0,
            }
        };
        let (ts_a, ts_b) = (ts(a), ts(b));
        let ord_ts = ts_b.partial_cmp(&ts_a).unwrap_or(std::cmp::Ordering::Equal);
        if ord_ts != std::cmp::Ordering::Equal {
            return ord_ts;
        }
        let rank_ord = role_rank(b.get("role").unwrap_or(&Value::Null))
            .cmp(&role_rank(a.get("role").unwrap_or(&Value::Null)));
        if rank_ord != std::cmp::Ordering::Equal {
            return rank_ord;
        }
        let online_ord = jsval::truthy(b.get("online").unwrap_or(&Value::Null))
            .cmp(&jsval::truthy(a.get("online").unwrap_or(&Value::Null)));
        if online_ord != std::cmp::Ordering::Equal {
            return online_ord;
        }
        jsval::string(a.get("email").unwrap_or(&Value::Null))
            .cmp(&jsval::string(b.get("email").unwrap_or(&Value::Null)))
    });

    json_response(200, json!({ "members": out }))
}

/// `friendRequests.some(...)` leg for the friendStatus ladder.
fn member_req_between(req: &Value, from_norm: &str, to_norm: &str) -> bool {
    auth::normalize_email(&jsval::string(&jsval::or(req.get("from"), json!("")))) == from_norm
        && auth::normalize_email(&jsval::string(&jsval::or(req.get("to"), json!("")))) == to_norm
}

/// The role ladder (server.js:19133-19141).
fn member_role(state: &Arc<AppState>, email: &str) -> &'static str {
    let norm = auth::normalize_email(email);
    if auth::is_co_owner_email(&state.store, email) {
        "co-owner/developer"
    } else if auth::owner_member_emails(&state.store)
        .iter()
        .any(|o| auth::normalize_email(o) == norm)
    {
        "owner/developer"
    } else if auth::admin_member_emails(&state.store)
        .iter()
        .any(|a| auth::normalize_email(a) == norm)
    {
        "admin/developer"
    } else if auth::is_moderator_email(&state.store, email) {
        "moderator"
    } else if admin::is_blog_contributor_email(&state.store, state.data_dir(), email) {
        "contributor"
    } else if auth::is_premium_email(&state.store, email) {
        "premium"
    } else {
        "member"
    }
}

/// `backendRoleRank` (server.js:19238-19244).
fn role_rank(role: &Value) -> u8 {
    let s = jsval::string(&jsval::or(Some(role), json!("")));
    if s.contains("owner") {
        5
    } else if s.contains("admin") {
        4
    } else if s.contains("moderator") {
        3
    } else if s.contains("premium") {
        2
    } else {
        1
    }
}

/// The `lastMessage` spread (server.js:19172-19197): `{...maxMsg, text, image,
/// from, to, readBy}` — overridden keys keep their original position, keys
/// whose JS value is `undefined` are dropped by `JSON.stringify`.
fn last_message_of(state: &Arc<AppState>, member_dms: &[&Value]) -> Option<Value> {
    let mut max_ts = 0.0_f64;
    let mut max_msg: Option<&Value> = None;
    for m in member_dms {
        let ts = jsval::number(&jsval::or(m.get("ts"), json!(0))).unwrap_or(f64::NAN);
        if ts >= max_ts {
            max_ts = ts;
            max_msg = Some(m);
        }
    }
    let msg = max_msg?;
    let mut obj = msg.clone();
    if !obj.is_object() {
        obj = json!({});
    }
    let (text, image) = dm_content_parts(msg, &state.id_secret);
    let from_disp = profile::display_email(
        &state.store,
        state.data_dir(),
        &state.id_secret,
        &jsval::string(&jsval::or(msg.get("from"), json!(""))),
    );
    let to_disp = profile::display_email(
        &state.store,
        state.data_dir(),
        &state.id_secret,
        &jsval::string(&jsval::or(msg.get("to"), json!(""))),
    );
    let empty_arr: Vec<Value> = Vec::new();
    let read_by: Vec<Value> = msg
        .get("readBy")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty_arr)
        .iter()
        .map(|r| {
            json!(profile::display_email(
                &state.store,
                state.data_dir(),
                &state.id_secret,
                &jsval::string(&jsval::or(Some(r), json!(""))),
            ))
        })
        .collect();
    if let Some(map) = obj.as_object_mut() {
        match text {
            Some(t) => {
                map.insert("text".into(), t);
            }
            None => {
                map.remove("text");
            }
        }
        match image {
            Some(i) => {
                map.insert("image".into(), i);
            }
            None => {
                map.remove("image");
            }
        }
        map.insert("from".into(), json!(from_disp));
        map.insert("to".into(), json!(to_disp));
        map.insert("readBy".into(), json!(read_by));
    }
    Some(obj)
}

// ── /api/admin-members, /api/moderator-members, /api/owner-members ──────────

/// `server.js:19281-19307` — public list of site administrators.
fn admin_members(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let Some((_, viewer_email)) = member_gate(state, headers) else {
        return json_response(401, json!({ "error": "auth required" }));
    };
    let profiles = state
        .store
        .read_document(&data_file(state, "profiles.json"), json!({}));
    let cosmetics = state
        .store
        .read_document(&data_file(state, "cosmetics.json"), json!({}));
    let members: Vec<Value> = auth::admin_member_emails(&state.store)
        .iter()
        .filter(|e| e.as_str() != TEST_ACCOUNT_EMAIL && !auth::is_owner_email(&state.store, e))
        .map(|email| {
            let norm = auth::normalize_email(email);
            let prof = profiles.get(norm.as_str()).cloned().unwrap_or(json!({}));
            let cosm = cosmetics.get(norm.as_str()).cloned().unwrap_or(json!({}));
            let processed = profile::process_member_fields(
                &state.store,
                state.data_dir(),
                email,
                Some(&prof),
                Some(&viewer_email),
            );
            let role = if auth::is_co_owner_email(&state.store, &norm) {
                "Co-owner"
            } else {
                "Admin"
            };
            json!({
                "displayName": processed.get("displayName").cloned().unwrap_or(Value::Null),
                "email": processed.get("email").cloned().unwrap_or(Value::Null),
                "role": role,
                "color": shop::public_active_color(&state.store, email, cosm.get("activeColor").unwrap_or(&Value::Null))
                    .map(|c| json!(c))
                    .unwrap_or(Value::Null),
                "badge": jsval::or(cosm.get("activeBadge"), json!(Value::Null)),
            })
        })
        .collect();
    json_response(200, json!({ "members": members }))
}

/// `server.js:19310-19338` — public list of site moderators.
fn moderator_members(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let Some((_, viewer_email)) = member_gate(state, headers) else {
        return json_response(401, json!({ "error": "auth required" }));
    };
    let profiles = state
        .store
        .read_document(&data_file(state, "profiles.json"), json!({}));
    let cosmetics = state
        .store
        .read_document(&data_file(state, "cosmetics.json"), json!({}));
    let mut excluded: Vec<String> = Vec::new();
    for email in auth::site_admin_emails(&state.store)
        .iter()
        .chain(auth::owner_member_emails(&state.store).iter())
        .chain(auth::co_owner_member_emails(&state.store).iter())
    {
        let norm = auth::normalize_email(email);
        if !excluded.contains(&norm) {
            excluded.push(norm);
        }
    }
    let mut seen: Vec<String> = Vec::new();
    let members: Vec<Value> = auth::moderator_emails(&state.store)
        .iter()
        .map(|email| jsval::string(&json!(email)).to_lowercase().trim().to_string())
        .filter(|email| {
            if email.is_empty() || email == TEST_ACCOUNT_EMAIL {
                return false;
            }
            let norm = auth::normalize_email(email);
            if excluded.contains(&norm) || seen.contains(&norm) {
                return false;
            }
            seen.push(norm);
            true
        })
        .map(|email| {
            let norm = auth::normalize_email(&email);
            let prof = profiles.get(norm.as_str()).cloned().unwrap_or(json!({}));
            let cosm = cosmetics.get(norm.as_str()).cloned().unwrap_or(json!({}));
            let processed = profile::process_member_fields(
                &state.store,
                state.data_dir(),
                &email,
                Some(&prof),
                Some(&viewer_email),
            );
            json!({
                "displayName": processed.get("displayName").cloned().unwrap_or(Value::Null),
                "email": processed.get("email").cloned().unwrap_or(Value::Null),
                "role": "Moderator",
                "color": shop::public_active_color(&state.store, &email, cosm.get("activeColor").unwrap_or(&Value::Null))
                    .map(|c| json!(c))
                    .unwrap_or(Value::Null),
                "badge": jsval::or(cosm.get("activeBadge"), json!(Value::Null)),
            })
        })
        .collect();
    json_response(200, json!({ "members": members }))
}

/// `server.js:19340-19365` — public list of site owners.
fn owner_members(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let Some((_, viewer_email)) = member_gate(state, headers) else {
        return json_response(401, json!({ "error": "auth required" }));
    };
    let profiles = state
        .store
        .read_document(&data_file(state, "profiles.json"), json!({}));
    let cosmetics = state
        .store
        .read_document(&data_file(state, "cosmetics.json"), json!({}));
    // [...new Set([...owners, ...coOwners].map(normalizeEmail))] — insertion order.
    let mut norms: Vec<String> = Vec::new();
    for email in auth::owner_member_emails(&state.store)
        .iter()
        .chain(auth::co_owner_member_emails(&state.store).iter())
    {
        let norm = auth::normalize_email(email);
        if !norms.contains(&norm) {
            norms.push(norm);
        }
    }
    let members: Vec<Value> = norms
        .iter()
        .filter(|n| n.as_str() != TEST_ACCOUNT_EMAIL)
        .map(|email| {
            let norm = auth::normalize_email(email);
            let prof = profiles.get(norm.as_str()).cloned().unwrap_or(json!({}));
            let cosm = cosmetics.get(norm.as_str()).cloned().unwrap_or(json!({}));
            let processed = profile::process_member_fields(
                &state.store,
                state.data_dir(),
                email,
                Some(&prof),
                Some(&viewer_email),
            );
            json!({
                "displayName": jsval::or(processed.get("displayName"), json!("mitch")),
                "email": processed.get("email").cloned().unwrap_or(Value::Null),
                "role": if auth::is_co_owner_email(&state.store, email) {
                    "co-owner/developer"
                } else {
                    "owner/developer"
                },
                "color": shop::public_active_color(&state.store, email, cosm.get("activeColor").unwrap_or(&Value::Null))
                    .map(|c| json!(c))
                    .unwrap_or(Value::Null),
                "badge": jsval::or(cosm.get("activeBadge"), json!(Value::Null)),
            })
        })
        .collect();
    json_response(200, json!({ "members": members }))
}

/// `server.js:21525-21555` — public list of approved premium users.
fn premium_members(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let Some((_, viewer_email)) = member_gate(state, headers) else {
        return json_response(401, json!({ "error": "auth required" }));
    };
    let applications = state
        .store
        .read_document(&data_file(state, "applications.json"), json!([]));
    let profiles = state
        .store
        .read_document(&data_file(state, "profiles.json"), json!({}));
    let cosmetics = state
        .store
        .read_document(&data_file(state, "cosmetics.json"), json!({}));

    let mut seen: Vec<String> = Vec::new();
    let mut members: Vec<Value> = Vec::new();
    if let Some(apps) = applications.as_array() {
        for a in apps {
            let status = a.get("status").and_then(|v| v.as_str()).unwrap_or("");
            let app_type = a.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let grant_premium = a.get("grantPremium").and_then(|v| v.as_bool()).unwrap_or(false);
            let email = a.get("email").and_then(|v| v.as_str()).unwrap_or("");
            if status == "approved" && (app_type == "premium" || grant_premium) && email != TEST_ACCOUNT_EMAIL {
                let norm = auth::normalize_email(email);
                if seen.contains(&norm) {
                    continue;
                }
                seen.push(norm.clone());
                let prof = profiles.get(norm.as_str()).cloned().unwrap_or(json!({}));
                let cosm = cosmetics.get(norm.as_str()).cloned().unwrap_or(json!({}));
                let processed = profile::process_member_fields(
                    &state.store,
                    state.data_dir(),
                    email,
                    Some(&prof),
                    Some(&viewer_email),
                );
                members.push(json!({
                    "displayName": processed.get("displayName").cloned().unwrap_or(Value::Null),
                    "email": processed.get("email").cloned().unwrap_or(Value::Null),
                    "color": shop::public_active_color(&state.store, email, cosm.get("activeColor").unwrap_or(&Value::Null))
                        .map(|c| json!(c))
                        .unwrap_or(Value::Null),
                    "badge": jsval::or(cosm.get("activeBadge"), json!(Value::Null)),
                }));
            }
        }
    }
    json_response(200, json!({ "members": members }))
}

// ── /api/userdata ────────────────────────────────────────────────────────────

/// `userdataPath(idVal)` (server.js:2755-2761) — sha256(id)[0..32] dir under
/// `USERDATA_DIR`; `null` on any filesystem failure → 503.
pub(crate) fn userdata_path(id_val: &str) -> Option<std::path::PathBuf> {
    let hex = crypto::sha256_hex(id_val.as_bytes());
    let dir = std::path::Path::new(USERDATA_DIR).join(&hex[..32]);
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("data.json"))
}

/// `/api/userdata` (server.js:17262-17317) — the path-only branch, so GETs
/// (empty body → `{}`) land here too and persist `_updated`.
fn userdata(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let cookies = cookies_of(state, headers);
    let uid = me_uid(&cookies);
    if !auth::valid_id(&uid, &state.id_secret) {
        return json_response(401, json!({ "error": "Not authenticated" }));
    }
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "Bad JSON" }));
    };
    // JS `typeof body !== 'object' || Array.isArray(body)` — typeof null is
    // 'object', so a null body passes this gate (Object.assign then no-ops).
    let is_js_object = body.is_object() || body.is_null();
    if !is_js_object || body.is_array() {
        return json_response(400, json!({ "error": "Expected object" }));
    }
    let Some(fpath) = userdata_path(&uid) else {
        return json_response(503, json!({ "error": "Storage unavailable" }));
    };
    let mut existing: Value = std::fs::read_to_string(&fpath)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}));
    if let Value::Object(map) = &mut existing {
        // Object.assign(existing, body) — existing keys keep their position.
        if let Some(body_map) = body.as_object() {
            for (k, v) in body_map {
                map.insert(k.clone(), v.clone());
            }
        }
        // `existing._updated = Date.now() / 1000` (seconds, float).
        map.insert(
            "_updated".into(),
            json!(mitch_lib::school::now_millis() as f64 / 1000.0),
        );
    } else if existing.is_null() {
        // JS: Object.assign(null, body) throws TypeError → uncaught 500.
        return json_response(500, json!({ "error": "userdata target invalid" }));
    }
    // Primitives/arrays: Object.assign no-ops on primitives and stringify drops
    // array expandos, so the stored value passes through unchanged.

    // Update newsletter unsub list based on preference (server.js:17277-17298,
    // wrapped in try/catch — every failure path just skips the sync).
    sync_newsletter_unsub(state, &uid, &body, &existing);

    let new_json = serde_json::to_string(&existing).unwrap_or_default();
    let used = new_json.len();
    if used > USERDATA_QUOTA {
        return json_response(
            413,
            json!({
                "error": "quota_exceeded",
                "used": used,
                "limit": USERDATA_QUOTA
            }),
        );
    }
    let _ = std::fs::write(&fpath, new_json.as_bytes());
    json_response(200, json!({ "ok": true }))
}

/// The newsletter-unsub sync block (server.js:17278-17297).
fn sync_newsletter_unsub(state: &Arc<AppState>, uid: &str, body: &Value, existing: &Value) {
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, uid) else {
        return;
    };
    newsletter_sync_core(&state.store, state.data_dir(), &email, body, existing);
}

/// The sync core over `(store, data_dir, email)` so it is unit-testable.
fn newsletter_sync_core(
    store: &mitch_lib::data::DataStore,
    data_dir: &std::path::Path,
    email: &str,
    body: &Value,
    existing: &Value,
) {
    // snap = body._snapshot || existing._snapshot (after the merge).
    let body_snap = body.get("_snapshot");
    let snap = if body_snap.is_some_and(jsval::truthy) {
        body_snap.cloned().unwrap_or(Value::Null)
    } else {
        existing.get("_snapshot").cloned().unwrap_or(Value::Null)
    };
    if !jsval::truthy(&snap) {
        return;
    }
    let Some(raw_pref) = snap.get("_prefPrivacy").filter(|v| jsval::truthy(v)) else {
        return;
    };
    // pref = typeof _prefPrivacy === 'string' ? JSON.parse(...) : raw.
    // A JSON.parse failure throws → caught by the JS outer try → skip.
    let pref: Value = match raw_pref {
        Value::String(s) => match serde_json::from_str(s) {
            Ok(v) => v,
            Err(_) => return,
        },
        other => other.clone(),
    };
    let low = email.to_lowercase().trim().to_string();
    let file = data_dir.join("newsletter_unsub.json");
    let unsub = store.read_document(&file, json!([]));
    let Some(list) = unsub.as_array() else {
        return; // JS .includes/.indexOf on a non-array throws → outer catch
    };
    let mut new_list: Vec<Value> = list.clone();
    let mut changed = false;
    match pref.get("newsletter") {
        Some(Value::Bool(false)) => {
            if !list.iter().any(|e| e.as_str() == Some(low.as_str())) {
                new_list.push(json!(low));
                changed = true;
            }
        }
        Some(Value::Bool(true)) => {
            if let Some(pos) = list.iter().position(|e| e.as_str() == Some(low.as_str())) {
                new_list.remove(pos);
                changed = true;
            }
        }
        _ => {}
    }
    if !changed {
        return;
    }
    // saveJson(..., [...new Set(unsub)].sort()) — dedupe (Set keeps the first
    // occurrence), then JS default string sort.
    let mut deduped: Vec<Value> = Vec::new();
    for e in &new_list {
        if !deduped.contains(e) {
            deduped.push(e.clone());
        }
    }
    deduped.sort_by_key(jsval::string);
    let _ = store.write_document(&file, &json!(deduped));
    let _ = std::fs::write(&file, mitch_lib::data::js_stringify_pretty(&json!(deduped)));
}

// ── shared helpers ───────────────────────────────────────────────────────────

/// The member-directory session gate (server.js:19116): sid present, valid,
/// unrevoked. Returns `Some((sid, email))`, or `None` → 401
/// `{error:'auth required'}` (built at the call sites).
fn member_gate(state: &Arc<AppState>, headers: &HeaderMap) -> Option<(String, String)> {
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    if sid.is_empty() || !auth::valid_id(&sid, &state.id_secret) || is_revoked_id(state, &sid) {
        return None;
    }
    let email = auth::email_from_sid(&state.store, &state.id_secret, &sid).unwrap_or_default();
    Some((sid, email))
}

/// `isRevoked(id)` (server.js:2769) — key presence in revoked.json.
fn is_revoked_id(state: &Arc<AppState>, sid: &str) -> bool {
    state
        .store
        .read_document(&data_file(state, "revoked.json"), json!({}))
        .get(sid)
        .is_some()
}

/// `touchActiveEmail(email, now)` (server.js:3396-3404) — the 10s-throttled
/// `last_active_at` stamp in user_stats.json. JS keeps `userStats` as an
/// in-process cache; the Rust port reads/writes the document directly.
fn touch_active_email(state: &Arc<AppState>, email: &str, now: i64) {
    if email.is_empty() {
        return;
    }
    let norm = auth::normalize_email(email);
    let file = data_file(state, "user_stats.json");
    let mut stats = state.store.read_document(&file, json!({}));
    let Some(map) = stats.as_object_mut() else {
        return;
    };
    // `if (!stats[norm]) stats[norm] = {}` — falsy entries are replaced.
    let needs_reset = map.get(&norm).map(|v| !jsval::truthy(v)).unwrap_or(true);
    if needs_reset {
        map.insert(norm.clone(), json!({}));
    }
    let Some(entry) = map.get_mut(&norm).and_then(|e| e.as_object_mut()) else {
        return; // JS: property set on a truthy primitive silently no-ops
    };
    let last = entry
        .get("last_active_at")
        .and_then(jsval::number)
        .unwrap_or(0.0);
    if (now as f64) - last < 10_000.0 {
        return;
    }
    entry.insert("last_active_at".into(), json!(now));
    let _ = state.store.write_document(&file, &stats);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `sha256(sid).hex[0..32]` — golden from bun for a sample sid.
    #[test]
    fn userdata_dir_key_matches_js() {
        let key = crypto::sha256_hex(b"e8a915a51367eb4a3727c6617.0ce0b5e127897972");
        assert_eq!(&key[..32], "7e523c1316648cffdc2eb7a03d69e1ec");
    }

    /// Object.assign positional semantics: overwrites keep position, new keys
    /// append in source order.
    #[test]
    fn assign_merge_keeps_positions() {
        let mut existing = json!({"a": 1, "b": 2, "c": 3});
        let body = json!({"b": 9, "d": 4});
        if let Value::Object(map) = &mut existing {
            for (k, v) in body.as_object().unwrap() {
                map.insert(k.clone(), v.clone());
            }
        }
        let keys: Vec<&str> = existing
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, vec!["a", "b", "c", "d"]);
        assert_eq!(existing["b"], json!(9));
    }

    /// The newsletter-unsub sync ladder (strict === true/false on pref.newsletter).
    #[test]
    fn newsletter_unsub_ladder() {
        let base = std::env::temp_dir().join(format!(
            "members-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        let store = mitch_lib::data::DataStore::open(&base, &base.join("data")).unwrap();
        let dir = base.join("data");
        let file = dir.join("newsletter_unsub.json");
        let read_list =
            |store: &mitch_lib::data::DataStore| -> Value { store.read_document(&file, json!([])) };
        let body = json!({"_snapshot": {"_prefPrivacy": "{\"newsletter\":false}"}});
        let existing = body.clone();
        newsletter_sync_core(&store, &dir, "User@student.rjuhsd.us", &body, &existing);
        assert_eq!(read_list(&store), json!(["user@student.rjuhsd.us"]));
        // Already present → unchanged list (dedupe, not a second entry).
        newsletter_sync_core(&store, &dir, "user@student.rjuhsd.us", &body, &existing);
        assert_eq!(read_list(&store), json!(["user@student.rjuhsd.us"]));
        // newsletter:true removes it.
        let on = json!({"_snapshot": {"_prefPrivacy": "{\"newsletter\":true}"}});
        newsletter_sync_core(&store, &dir, "user@student.rjuhsd.us", &on, &on);
        assert_eq!(read_list(&store), json!([]));
        // newsletter:0 (neither strict false nor true) → untouched.
        let off_num = json!({"_snapshot": {"_prefPrivacy": "{\"newsletter\":0}"}});
        store.write_document(&file, &json!(["keep@x"])).unwrap();
        newsletter_sync_core(&store, &dir, "keep@x", &off_num, &off_num);
        assert_eq!(read_list(&store), json!(["keep@x"]));
        std::fs::remove_dir_all(&base).ok();
    }

    /// Sort comparator parity: ts desc → role rank desc → online desc → email asc.
    #[test]
    fn members_sort_ladder() {
        let mk = |email: &str, ts: f64, role: &str, online: bool| {
            json!({
                "email": email,
                "role": role,
                "online": online,
                "lastMessage": if ts > 0.0 { json!({"ts": ts}) } else { Value::Null },
            })
        };
        let mut members = [
            mk("b@x", 0.0, "member", false),
            mk("a@x", 0.0, "member", true),
            mk("c@x", 5.0, "premium", false),
            mk("d@x", 5.0, "member", false),
        ];
        members.sort_by(|a, b| {
            let ts = |v: &Value| -> f64 {
                match v.get("lastMessage") {
                    Some(lm) if jsval::truthy(lm) => {
                        jsval::number(&jsval::or(lm.get("ts"), json!(0))).unwrap_or(f64::NAN)
                    }
                    _ => 0.0,
                }
            };
            let (ts_a, ts_b) = (ts(a), ts(b));
            let ord_ts = ts_b.partial_cmp(&ts_a).unwrap_or(std::cmp::Ordering::Equal);
            if ord_ts != std::cmp::Ordering::Equal {
                return ord_ts;
            }
            let rank_ord = role_rank(b.get("role").unwrap_or(&Value::Null))
                .cmp(&role_rank(a.get("role").unwrap_or(&Value::Null)));
            if rank_ord != std::cmp::Ordering::Equal {
                return rank_ord;
            }
            let online_ord = jsval::truthy(b.get("online").unwrap_or(&Value::Null))
                .cmp(&jsval::truthy(a.get("online").unwrap_or(&Value::Null)));
            if online_ord != std::cmp::Ordering::Equal {
                return online_ord;
            }
            jsval::string(a.get("email").unwrap_or(&Value::Null))
                .cmp(&jsval::string(b.get("email").unwrap_or(&Value::Null)))
        });
        let emails: Vec<&str> = members
            .iter()
            .map(|m| m["email"].as_str().unwrap())
            .collect();
        // c and d lead on ts; c outranks d on role; then online a; then b.
        assert_eq!(emails, vec!["c@x", "d@x", "a@x", "b@x"]);
    }
}
