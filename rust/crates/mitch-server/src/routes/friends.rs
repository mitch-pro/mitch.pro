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
        return Some(request_cancel(state, headers, body_bytes));
    }
    if path == "/api/friends/request/respond" && method == Method::POST {
        return Some(request_respond(state, headers, body_bytes).await);
    }
    if path == "/api/friends/remove" && method == Method::POST {
        return Some(friends_remove(state, headers, body_bytes));
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

    let already = mitch_lib::friends::friends_get(&state.store, &norm)
        .as_array()
        .map(|list| {
            list.iter()
                .any(|f| auth::normalize_email(&jsval::string(f)) == friend_email)
        })
        .unwrap_or(false);
    if already {
        return json_response(400, json!({ "error": "already friends" }));
    }

    let requests_file = data_file(state, "friend_requests.json");
    let mut requests = state.store.read_document(&requests_file, json!([]));
    if let Some(arr) = requests.as_array_mut() {
        // Reverse request exists → auto-accept (server.js:16105-16131).
        let reverse_idx = arr.iter().position(|r| {
            auth::normalize_email(&jsval::string(&jsval::or(r.get("from"), json!(""))))
                == friend_email
                && auth::normalize_email(&jsval::string(&jsval::or(r.get("to"), json!("")))) == norm
        });
        if let Some(idx) = reverse_idx {
            arr.remove(idx);
            let _ = state.store.write_document(&requests_file, &requests);

            mitch_lib::friends::friends_add(&state.store, &norm, &friend_email);
            mitch_lib::friends::friends_add(&state.store, &friend_email, &norm);

            send_web_push_clean(
                state,
                &friend_email,
                &json!({
                    "title": "Friend Request Accepted",
                    "body": format!("{} accepted your friend request!", mitch_lib::admin::mask_email(&email)),
                    "url": notification_url("/"),
                }),
            )
            .await;
            return json_response(200, json!({ "ok": true, "status": "accepted" }));
        }

        // Duplicate request exists (server.js:16133-16137).
        let dup_idx = arr.iter().position(|r| {
            auth::normalize_email(&jsval::string(&jsval::or(r.get("from"), json!("")))) == norm
                && auth::normalize_email(&jsval::string(&jsval::or(r.get("to"), json!(""))))
                    == friend_email
        });
        if dup_idx.is_some() {
            return json_response(400, json!({ "error": "request already sent" }));
        }
    }

    // Create the pending request (server.js:16139-16154).
    if let Some(arr) = requests.as_array_mut() {
        arr.push(json!({
            "from": norm,
            "to": friend_email,
            "timestamp": mitch_lib::school::now_millis(),
        }));
    }
    let _ = state.store.write_document(&requests_file, &requests);

    send_web_push_clean(
        state,
        &friend_email,
        &json!({
            "title": "New Friend Request",
            "body": format!("{} sent you a friend request!", mitch_lib::admin::mask_email(&email)),
            "url": notification_url("/"),
        }),
    )
    .await;
    json_response(200, json!({ "ok": true, "status": "pending" }))
}

/// `/api/friends/request/cancel` (server.js:16154-16173).
fn request_cancel(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let email = match session_email(state, headers) {
        Ok(e) => e,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let resolved = profile::resolve_target_email(
        &state.store,
        state.data_dir(),
        &state.id_secret,
        &jsval::string(&jsval::or(body.get("email"), json!(""))),
    );
    let Some(resolved) = resolved else {
        return json_response(400, json!({ "error": "invalid email" }));
    };
    let friend_email = auth::normalize_email(&resolved);
    let norm = auth::normalize_email(&email);

    let requests_file = data_file(state, "friend_requests.json");
    let mut requests = state.store.read_document(&requests_file, json!([]));
    let idx = requests.as_array().map(|arr| {
        arr.iter().position(|r| {
            auth::normalize_email(&jsval::string(&jsval::or(r.get("from"), json!("")))) == norm
                && auth::normalize_email(&jsval::string(&jsval::or(r.get("to"), json!(""))))
                    == friend_email
        })
    });
    match idx {
        Some(Some(i)) => {
            if let Some(arr) = requests.as_array_mut() {
                arr.remove(i);
            }
        }
        _ => {
            return json_response(
                400,
                json!({ "error": "no pending request found to this user" }),
            )
        }
    }
    let _ = state.store.write_document(&requests_file, &requests);
    json_response(200, json!({ "ok": true }))
}

/// `/api/friends/request/respond` (server.js:16182-16229).
async fn request_respond(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Response {
    let email = match session_email(state, headers) {
        Ok(e) => e,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let resolved = profile::resolve_target_email(
        &state.store,
        state.data_dir(),
        &state.id_secret,
        &jsval::string(&jsval::or(body.get("email"), json!(""))),
    );
    let Some(resolved) = resolved else {
        return json_response(400, json!({ "error": "invalid email" }));
    };
    let friend_email = auth::normalize_email(&resolved);
    let action = jsval::string(&jsval::or(body.get("action"), json!("")))
        .trim()
        .to_lowercase();
    if action != "accept" && action != "reject" {
        return json_response(400, json!({ "error": "invalid action" }));
    }

    let norm = auth::normalize_email(&email);
    let requests_file = data_file(state, "friend_requests.json");
    let mut requests = state.store.read_document(&requests_file, json!([]));
    let idx = requests.as_array().map(|arr| {
        arr.iter().position(|r| {
            auth::normalize_email(&jsval::string(&jsval::or(r.get("from"), json!(""))))
                == friend_email
                && auth::normalize_email(&jsval::string(&jsval::or(r.get("to"), json!("")))) == norm
        })
    });
    match idx {
        Some(Some(i)) => {
            if let Some(arr) = requests.as_array_mut() {
                arr.remove(i);
            }
        }
        _ => {
            return json_response(
                400,
                json!({ "error": "no pending request found from this user" }),
            )
        }
    }
    let _ = state.store.write_document(&requests_file, &requests);

    if action == "accept" {
        mitch_lib::friends::friends_add(&state.store, &norm, &friend_email);
        mitch_lib::friends::friends_add(&state.store, &friend_email, &norm);

        send_web_push_clean(
            state,
            &friend_email,
            &json!({
                "title": "Friend Request Accepted",
                "body": format!("{} accepted your friend request!", mitch_lib::admin::mask_email(&email)),
                "url": notification_url("/"),
            }),
        )
        .await;
    }
    json_response(200, json!({ "ok": true }))
}

/// `/api/friends/remove` (server.js:16230-16264) — always `{ok:true}`.
fn friends_remove(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let email = match session_email(state, headers) {
        Ok(e) => e,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let resolved = profile::resolve_target_email(
        &state.store,
        state.data_dir(),
        &state.id_secret,
        &jsval::string(&jsval::or(body.get("email"), json!(""))),
    );
    let Some(resolved) = resolved else {
        return json_response(400, json!({ "error": "invalid email" }));
    };
    let friend_email = auth::normalize_email(&resolved);
    let norm = auth::normalize_email(&email);

    for (key, other) in [(&norm, &friend_email), (&friend_email, &norm)] {
        let mut list = mitch_lib::friends::friends_get(&state.store, key);
        if let Some(arr) = list.as_array_mut() {
            arr.retain(|f| auth::normalize_email(&jsval::string(f)) != *other);
            mitch_lib::friends::friends_set(&state.store, key, &list);
        }
    }
    json_response(200, json!({ "ok": true }))
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
    let presence_map = state
        .user_presence
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
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
            let is_online = is_user_present(state, &f_norm, now);
            let playing = if is_online {
                presence_map
                    .get(&f_norm)
                    .map(|p| p.playing.clone())
                    .unwrap_or_default()
            } else {
                String::new()
            };
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
                "online": is_online,
                "playing": playing,
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

/// `isUserPresent(email, now)` (server.js:1077-1084) — both legs live since
/// the Step 11 WS batch; the map-only port moved to `crate::ws`.
pub(crate) fn is_user_present(state: &std::sync::Arc<AppState>, email: &str, now: i64) -> bool {
    crate::ws::is_user_present(state, email, now)
}

/// `notificationUrl` (server.js:2053-2055).
fn notification_url(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    }
}

/// `encodeURIComponent` (JS semantics — see `auth::encode_uri_component`).
pub(crate) fn encode_uri_component(s: &str) -> String {
    auth::encode_uri_component(s)
}
