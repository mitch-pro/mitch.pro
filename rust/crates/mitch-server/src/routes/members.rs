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

use super::friends::encode_uri_component;
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

    let tokens = mitch_lib::tokens::token_all(&state.store);
    let profiles = mitch_lib::profiles::profile_all(&state.store);
    let dms = state
        .store
        .read_document(&data_file(state, "dms.json"), json!([]));
    let my_cleared =
        mitch_lib::dm::dm_cleared_get(&state.store, &mitch_lib::dm::DMS_MAIN, &viewer_norm);
    let my_friends: Vec<String> = mitch_lib::friends::friends_get(&state.store, &viewer_norm)
        .as_array()
        .map(|a| a.iter().map(jsval::string).collect())
        .unwrap_or_default();
    let friend_requests = state
        .store
        .read_document(&data_file(state, "friend_requests.json"), json!([]));
    let presence_map = crate::presence::snapshot(state, now);

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
        let norm = auth::normalize_email(&email);
        if norm.is_empty() || seen.contains(&norm) || is_revoked_id(state, tok) {
            continue;
        }
        if email == TEST_ACCOUNT_EMAIL && !is_admin {
            continue;
        }
        seen.push(norm.clone());
        let empty = json!({});
        let prof = profiles
            .get(norm.as_str())
            .cloned()
            .unwrap_or(empty.clone());
        let cosm = mitch_lib::shop::cosmetics_get(&state.store, norm.as_str()).unwrap_or(empty);
        let username = profile::normalize_username(&jsval::string(&jsval::or(
            prof.get("username"),
            json!(profile::default_username_for_email(&norm)),
        )));
        let role = member_role(state, &email);

        // `e2eUsers[norm]?.pub_key` — the live WS chat-key leg arrives with the
        // Step 11 WS work; here the map is empty, so the stored/derived key wins.
        let e2e_entry = mitch_lib::e2e::user_e2e_key_get(&state.store, &norm);
        let e2e_entry = e2e_entry.as_ref().filter(|v| jsval::truthy(v));
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
        let is_self = viewer_norm == norm;
        let is_friend = my_friends.iter().any(|f| auth::normalize_email(f) == norm);
        let can_view_activity = is_self || is_friend || is_admin;
        let presence = presence_map.get(&norm).cloned().unwrap_or_default();
        let public_status = if presence.online { "online" } else { "offline" };
        member.insert("online".into(), json!(presence.online));
        member.insert("lastActive".into(), json!(presence.last_active));
        member.insert(
            "playing".into(),
            json!(if can_view_activity {
                presence.playing
            } else {
                String::new()
            }),
        );
        member.insert(
            "activity".into(),
            json!(if can_view_activity {
                presence.activity
            } else {
                String::new()
            }),
        );
        member.insert(
            "page".into(),
            json!(if can_view_activity {
                presence.page
            } else {
                String::new()
            }),
        );
        member.insert(
            "status".into(),
            json!(if can_view_activity {
                presence.status
            } else {
                public_status.to_string()
            }),
        );
        member.insert(
            "lastSeen".into(),
            json!(if can_view_activity {
                presence.last_seen
            } else {
                0
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
pub(crate) fn member_role(state: &Arc<AppState>, email: &str) -> &'static str {
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
    let profiles = mitch_lib::profiles::profile_all(&state.store);
    let members: Vec<Value> = auth::admin_member_emails(&state.store)
        .iter()
        .filter(|e| e.as_str() != TEST_ACCOUNT_EMAIL && !auth::is_owner_email(&state.store, e))
        .map(|email| {
            let norm = auth::normalize_email(email);
            let prof = profiles.get(norm.as_str()).cloned().unwrap_or(json!({}));
            let cosm = mitch_lib::shop::cosmetics_get(&state.store, norm.as_str()).unwrap_or(json!({}));
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
    let profiles = mitch_lib::profiles::profile_all(&state.store);
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
            let cosm = mitch_lib::shop::cosmetics_get(&state.store, norm.as_str()).unwrap_or(json!({}));
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
    let profiles = mitch_lib::profiles::profile_all(&state.store);
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
            let cosm = mitch_lib::shop::cosmetics_get(&state.store, norm.as_str()).unwrap_or(json!({}));
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
    let profiles = mitch_lib::profiles::profile_all(&state.store);
    let mut seen: Vec<String> = Vec::new();
    let mut members: Vec<Value> = Vec::new();
    if let Some(apps) = applications.as_array() {
        for a in apps {
            let status = a.get("status").and_then(|v| v.as_str()).unwrap_or("");
            let app_type = a.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let grant_premium = a
                .get("grantPremium")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let email = a.get("email").and_then(|v| v.as_str()).unwrap_or("");
            if status == "approved"
                && (app_type == "premium" || grant_premium)
                && email != TEST_ACCOUNT_EMAIL
            {
                let norm = auth::normalize_email(email);
                if seen.contains(&norm) {
                    continue;
                }
                seen.push(norm.clone());
                let prof = profiles.get(norm.as_str()).cloned().unwrap_or(json!({}));
                let cosm = mitch_lib::shop::cosmetics_get(&state.store, norm.as_str())
                    .unwrap_or(json!({}));
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
    sync_privacy_prefs(state, &uid, &body, &existing);

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

fn sync_privacy_prefs(state: &Arc<AppState>, uid: &str, body: &Value, existing: &Value) {
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, uid) else {
        return;
    };
    let norm = auth::normalize_email(&email);
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
    let pref: Value = match raw_pref {
        Value::String(s) => match serde_json::from_str(s) {
            Ok(v) => v,
            Err(_) => return,
        },
        other => other.clone(),
    };
    let mut presence = state
        .user_presence
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let rec = presence.entry(norm).or_default();
    rec.hide_location = pref
        .get("hideLocation")
        .or_else(|| pref.get("hidePageLocation"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    rec.hide_online = pref
        .get("hideOnline")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    rec.share_activity = pref
        .get("shareActivity")
        .and_then(Value::as_bool)
        .unwrap_or(false);
}

/// The newsletter-unsub sync block (server.js:17278-17297).
fn sync_newsletter_unsub(state: &Arc<AppState>, uid: &str, body: &Value, existing: &Value) {
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, uid) else {
        return;
    };
    newsletter_sync_core(&state.store, &email, body, existing);
}

/// The sync core over `(store, email)` so it is unit-testable.
fn newsletter_sync_core(
    store: &mitch_lib::data::DataStore,
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
    match pref.get("newsletter") {
        Some(Value::Bool(false)) => {
            mitch_lib::newsletter_unsub::newsletter_unsub_add(store, &low);
        }
        Some(Value::Bool(true)) => {
            mitch_lib::newsletter_unsub::newsletter_unsub_remove(store, &low);
        }
        _ => {}
    }
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

/// `isRevoked(id)` (server.js:2769) — key presence in the revoked_ids table.
fn is_revoked_id(state: &Arc<AppState>, sid: &str) -> bool {
    mitch_lib::revoked::revoked_id_exists(&state.store, sid)
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
        let exists = |store: &mitch_lib::data::DataStore, email: &str| -> bool {
            mitch_lib::newsletter_unsub::newsletter_unsub_exists(store, email)
        };
        let body = json!({"_snapshot": {"_prefPrivacy": "{\"newsletter\":false}"}});
        let existing = body.clone();
        newsletter_sync_core(&store, "User@student.rjuhsd.us", &body, &existing);
        assert!(exists(&store, "user@student.rjuhsd.us"));
        // Already present → unchanged (idempotent add, not a second entry).
        newsletter_sync_core(&store, "user@student.rjuhsd.us", &body, &existing);
        assert!(exists(&store, "user@student.rjuhsd.us"));
        // newsletter:true removes it.
        let on = json!({"_snapshot": {"_prefPrivacy": "{\"newsletter\":true}"}});
        newsletter_sync_core(&store, "user@student.rjuhsd.us", &on, &on);
        assert!(!exists(&store, "user@student.rjuhsd.us"));
        // newsletter:0 (neither strict false nor true) → untouched.
        let off_num = json!({"_snapshot": {"_prefPrivacy": "{\"newsletter\":0}"}});
        mitch_lib::newsletter_unsub::newsletter_unsub_add(&store, "keep@x");
        newsletter_sync_core(&store, "keep@x", &off_num, &off_num);
        assert!(exists(&store, "keep@x"));
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

    #[tokio::test]
    async fn members_presence_activity_visible_only_to_friends_self_and_admins() {
        let base = std::env::temp_dir().join(format!(
            "members-presence-privacy-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        let cfg = crate::hosts::SiteConfig {
            data_dir: base.join("data"),
            ..crate::hosts::SiteConfig::load()
        };
        let store = Arc::new(mitch_lib::data::DataStore::open(&base, &cfg.data_dir).unwrap());
        let state = Arc::new(AppState::new(cfg, store));

        let mk_user = |email: &str| -> String {
            let sess = auth::create_auth_session(
                &state.store,
                &state.id_secret,
                email,
                email,
                "test",
                "127.0.0.1",
                false,
            );
            mitch_lib::tokens::token_set(
                &state.store,
                &sess.sid,
                &json!({ "email": email, "used": true }),
            );
            sess.token
        };
        let alice_tok = mk_user("alice@example.test");
        let bob_tok = mk_user("bob@example.test");
        let carol_tok = mk_user("carol@example.test");
        let admin_tok = mk_user("boss@example.test");
        let _ = state.store.write_document(
            &state.data_dir().join("admins.json"),
            &json!({ "admins": ["boss@example.test"] }),
        );
        mitch_lib::friends::friends_add(&state.store, "alice@example.test", "carol@example.test");
        mitch_lib::friends::friends_add(&state.store, "carol@example.test", "alice@example.test");

        crate::presence::update(
            &state,
            "alice@example.test",
            &json!({
                "clientId": "tab-1",
                "seq": 1,
                "page": "/shop/",
                "activity": "Browsing the shop",
                "visible": true,
                "focused": false,
                "shareActivity": true,
                "interaction": 1
            }),
            None,
        )
        .unwrap();

        let fetch_alice_as = |tok: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(
                axum::http::header::COOKIE,
                format!("mitch_session={tok}").parse().unwrap(),
            );
            let resp = members(&state, &headers);
            futures_util::FutureExt::now_or_never(axum::body::to_bytes(
                resp.into_body(),
                usize::MAX,
            ))
            .unwrap()
            .unwrap()
        };

        let parse_alice = |bytes: axum::body::Bytes| -> Value {
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            body["members"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["handle"].as_str().unwrap_or("").contains("alice"))
                .unwrap()
                .clone()
        };

        // Non-friend non-admin sees online boolean only; no page, activity, or away status.
        let as_bob = parse_alice(fetch_alice_as(&bob_tok));
        assert_eq!(as_bob["online"], json!(true));
        assert_eq!(as_bob["page"], json!(""));
        assert_eq!(as_bob["activity"], json!(""));
        assert_eq!(as_bob["playing"], json!(""));
        assert_eq!(as_bob["status"], json!("online"));
        assert_eq!(as_bob["lastSeen"], json!(0));

        // Friend sees page, activity, playing, and away status.
        let as_carol = parse_alice(fetch_alice_as(&carol_tok));
        assert_eq!(as_carol["online"], json!(true));
        assert_eq!(as_carol["page"], json!("/shop/"));
        assert_eq!(as_carol["activity"], json!("Browsing the shop"));
        assert_eq!(as_carol["playing"], json!("Browsing the shop"));
        assert_eq!(as_carol["status"], json!("away"));

        // Self sees own page and activity.
        let as_alice = parse_alice(fetch_alice_as(&alice_tok));
        assert_eq!(as_alice["page"], json!("/shop/"));
        assert_eq!(as_alice["activity"], json!("Browsing the shop"));

        // Site admin sees page and activity even when not friends.
        let as_admin = parse_alice(fetch_alice_as(&admin_tok));
        assert_eq!(as_admin["page"], json!("/shop/"));
        assert_eq!(as_admin["activity"], json!("Browsing the shop"));
        assert_eq!(as_admin["status"], json!("away"));

        // When Alice turns off page location in preferences (hideLocation: true),
        // nobody - not friends, not Alice, not site admins - can see her page or activity.
        crate::presence::update(
            &state,
            "alice@example.test",
            &json!({
                "clientId": "tab-1",
                "seq": 2,
                "page": "/shop/",
                "activity": "Browsing the shop",
                "visible": true,
                "focused": true,
                "interaction": 2,
                "hideLocation": true
            }),
            None,
        )
        .unwrap();

        let hidden_for_friend = parse_alice(fetch_alice_as(&carol_tok));
        assert_eq!(hidden_for_friend["online"], json!(true));
        assert_eq!(hidden_for_friend["page"], json!(""));
        assert_eq!(hidden_for_friend["activity"], json!(""));
        assert_eq!(hidden_for_friend["playing"], json!(""));

        let hidden_for_admin = parse_alice(fetch_alice_as(&admin_tok));
        assert_eq!(hidden_for_admin["online"], json!(true));
        assert_eq!(hidden_for_admin["page"], json!(""));
        assert_eq!(hidden_for_admin["activity"], json!(""));
        assert_eq!(hidden_for_admin["playing"], json!(""));

        drop(state);
        std::fs::remove_dir_all(&base).ok();
    }
}
