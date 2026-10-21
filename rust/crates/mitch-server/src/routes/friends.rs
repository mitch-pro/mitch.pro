//! `/api/friends/*` route group (plan Step 9 batch 4) — server.js:16078-16264
//! (request/cancel/respond/remove) and server.js:23856-23908 (list +
//! requests/pending). The mutating endpoints run behind `verifyRecaptcha`
//! (request) or plain session auth; `cancel`/`respond`/`remove` parse the
//! body AFTER the session gates, in JS order.

use super::me::{cookies_of, data_file, json_response, me_uid, parse_body_strict};
use crate::routes::push::send_web_push_clean;
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::auth;
use mitch_lib::jsval;
use mitch_lib::profile;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Option<Response> {
    if path == "/api/friends/request" && method == Method::POST {
        return Some(friend_request(state, headers, body_bytes).await);
    }
    if path == "/api/friends/request/cancel" && method == Method::POST {
        return Some(request_cancel(state, headers, body_bytes).await);
    }
    if path == "/api/friends/request/respond" && method == Method::POST {
        return Some(request_respond(state, headers, body_bytes).await);
    }
    if path == "/api/friends/remove" && method == Method::POST {
        return Some(friends_remove(state, headers, body_bytes).await);
    }
    if path == "/api/friends/list" {
        return Some(friends_list(state, headers));
    }
    if path == "/api/friends/requests/pending" {
        return Some(requests_pending(state, headers));
    }
    None
}

/// Shared gate: validId → 401 unauthorized, emailFromSid → 401 email not
/// found. Returns the original (unnormalized) email, or a (status, error)
/// pair small enough for `Result`.
fn session_email(
    state: &Arc<AppState>,
    headers: &HeaderMap,
) -> Result<String, (u16, &'static str)> {
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    if !auth::valid_id(&sid, &state.id_secret) {
        return Err((401, "unauthorized"));
    }
    match auth::email_from_sid(&state.store, &state.id_secret, &sid) {
        Some(email) => Ok(email),
        None => Err((401, "email not found")),
    }
}

/// `/api/friends/request` (server.js:16078-16152).
async fn friend_request(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let email = match session_email(state, headers) {
        Ok(e) => e,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    let ip = crate::handler::get_real_ip(headers, None);
    if !crate::routes::push::verify_recaptcha(
        state,
        &jsval::string(&jsval::or(body.get("recaptcha_token"), json!(""))),
        &ip,
        "",
    )
    .await
    {
        return json_response(
            400,
            json!({ "error": "reCAPTCHA failed. Please try again." }),
        );
    }

    let resolved = profile::resolve_target_email(
        &state.store,
        state.data_dir(),
        &state.id_secret,
        &jsval::string(&jsval::or(body.get("email"), json!(""))),
    );
    let Some(resolved) = resolved else {
        return json_response(400, json!({ "error": "user not found" }));
    };
    let friend_email = auth::normalize_email(&resolved);
    let norm = auth::normalize_email(&email);
    if norm == friend_email {
        return json_response(400, json!({ "error": "cannot friend request yourself" }));
    }

    apply_friend_action(state, &email, &friend_email, "request").await
}

fn action_target(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    bytes: &[u8],
) -> Result<(String, String, Value), (u16, &'static str)> {
    let email = session_email(state, headers)?;
    let body = parse_body_strict(bytes).ok_or((400, "Invalid request."))?;
    let target = profile::resolve_target_email(
        &state.store,
        state.data_dir(),
        &state.id_secret,
        body["email"].as_str().unwrap_or(""),
    )
    .ok_or((400, "Account not found."))?;
    Ok((email, target, body))
}

async fn apply_friend_action(
    state: &Arc<AppState>,
    email: &str,
    target: &str,
    action: &str,
) -> Response {
    let (status, changed) =
        match mitch_lib::friends::change_request(&state.store, email, target, action) {
            Ok(result) => result,
            Err(message) => return json_response(400, json!({"error":message})),
        };
    if changed && matches!(status, "pending" | "accepted") {
        let title = if status == "pending" {
            "New friend request"
        } else {
            "Friend request accepted"
        };
        send_web_push_clean(state,target,&json!({"title":title,"body":mitch_lib::admin::mask_email(email),"url":"/friends/","tag":format!("friend-request:{}",auth::normalize_email(email))})).await;
    }
    crate::ws::trigger_notification_refresh(state);
    json_response(200, json!({"ok":true,"status":status}))
}

async fn request_cancel(state: &Arc<AppState>, headers: &HeaderMap, bytes: &[u8]) -> Response {
    let (email, target, _) = match action_target(state, headers, bytes) {
        Ok(value) => value,
        Err((code, error)) => return json_response(code, json!({"error":error})),
    };
    apply_friend_action(state, &email, &target, "cancel").await
}
async fn request_respond(state: &Arc<AppState>, headers: &HeaderMap, bytes: &[u8]) -> Response {
    let (email, target, body) = match action_target(state, headers, bytes) {
        Ok(value) => value,
        Err((code, error)) => return json_response(code, json!({"error":error})),
    };
    let action = body["action"].as_str().unwrap_or("");
    if !matches!(action, "accept" | "reject") {
        return json_response(400, json!({"error":"Choose accept or reject."}));
    }
    apply_friend_action(state, &email, &target, action).await
}
async fn friends_remove(state: &Arc<AppState>, headers: &HeaderMap, bytes: &[u8]) -> Response {
    let (email, target, _) = match action_target(state, headers, bytes) {
        Ok(value) => value,
        Err((code, error)) => return json_response(code, json!({"error":error})),
    };
    apply_friend_action(state, &email, &target, "remove").await
}

/// `/api/friends/list` (server.js:23856-23887).
fn friends_list(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let email = match session_email(state, headers) {
        Ok(e) => e,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    let norm = auth::normalize_email(&email);
    let my_list: Vec<String> = mitch_lib::friends::friends_get(&state.store, &norm)
        .as_array()
        .map(|a| a.iter().map(jsval::string).collect())
        .unwrap_or_default();
    let profiles = mitch_lib::profiles::profile_all(&state.store);
    let now = mitch_lib::school::now_millis();
    let presence_map = crate::presence::snapshot(state, now);
    let res: Vec<Value> = my_list
        .iter()
        .map(|f| {
            let f_norm = auth::normalize_email(f);
            let empty = json!({});
            let prof = profiles.get(&f_norm).cloned().unwrap_or(empty);
            let processed =
                profile::process_member_fields(&state.store, state.data_dir(), f, Some(&prof), Some(&email));
            let username = profile::normalize_username(&jsval::string(&jsval::or(
                prof.get("username").filter(|v| jsval::truthy(v)),
                json!(profile::default_username_for_email(&f_norm)),
            )));
            let presence = presence_map.get(&f_norm).cloned().unwrap_or_default();
            let cosm = mitch_lib::shop::cosmetics_get(&state.store, &f_norm).unwrap_or(json!({}));
            json!({
                "color": mitch_lib::shop::public_active_color(&state.store, &f_norm, cosm.get("activeColor").unwrap_or(&Value::Null)),
                "badge": cosm.get("activeBadge").cloned().unwrap_or(Value::Null),
                "email": f,
                "maskedEmail": mitch_lib::admin::mask_email(f),
                "handle": username,
                "displayName": processed.get("displayName").cloned().unwrap_or(Value::Null),
                "bio": jsval::string(&jsval::or(prof.get("bio"), json!(""))).chars().take(120).collect::<String>(),
                "pfp": profile::sanitize_profile_image_url(
                    &jsval::string(&jsval::or(prof.get("pfp"), json!(""))),
                    true,
                    1000,
                    120_000,
                ),
                "profileUrl": format!("/profile/?u={}", encode_uri_component(&username)),
                "online": presence.online,
                "playing": presence.playing,
                "activity": presence.activity,
                "page": presence.page,
                "status": presence.status,
                "lastSeen": presence.last_seen,
                "lastActive": presence.last_active,
                "role": super::members::member_role(state, f),
            })
        })
        .collect();
    json_response(200, json!({ "friends": res }))
}

/// `/api/friends/requests/pending` (server.js:23889-23908).
fn requests_pending(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let email = match session_email(state, headers) {
        Ok(e) => e,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    let norm = auth::normalize_email(&email);
    let requests = state
        .store
        .read_document(&data_file(state, "friend_requests.json"), json!([]));
    let empty = vec![];
    let arr = requests.as_array().unwrap_or(&empty);
    let incoming: Vec<Value> = arr
        .iter()
        .filter(|r| auth::normalize_email(&jsval::string(&jsval::or(r.get("to"), json!("")))) == norm)
        .map(|r| {
            json!({
                "from": r.get("from").cloned().unwrap_or(Value::Null),
                "maskedFrom": mitch_lib::admin::mask_email(&jsval::string(&jsval::or(r.get("from"), json!("")))),
                "timestamp": r.get("timestamp").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    let outgoing: Vec<Value> = arr
        .iter()
        .filter(|r| {
            auth::normalize_email(&jsval::string(&jsval::or(r.get("from"), json!("")))) == norm
        })
        .map(|r| {
            json!({
                "to": r.get("to").cloned().unwrap_or(Value::Null),
                "maskedTo": mitch_lib::admin::mask_email(&jsval::string(&jsval::or(r.get("to"), json!("")))),
                "timestamp": r.get("timestamp").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    json_response(200, json!({ "incoming": incoming, "outgoing": outgoing }))
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// `encodeURIComponent` (JS semantics — see `auth::encode_uri_component`).
pub(crate) fn encode_uri_component(s: &str) -> String {
    auth::encode_uri_component(s)
}
