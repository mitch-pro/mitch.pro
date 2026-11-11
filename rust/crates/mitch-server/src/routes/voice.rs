//! Voice channels (`/voice/`): audio-only LiveKit rooms joined straight from
//! the browser with `livekit-client`. This replaces the MatrixRTC / Element
//! Call path, which never connected. See `docs/voice-channels.md`.
//!
//! - `GET  /api/voice/channels` — channel list with who is in each one.
//! - `POST /api/voice/token`    — `{ "channel": "lounge" }` → `{ url, token }`.
//!
//! Both need a signed-in, non-banned account. The token is limited to one
//! fixed channel, microphone audio only, and no data messages, so it can't be
//! used to join arbitrary rooms (unlike the legacy `/livekit/get_token`).

use crate::errors::json_resp;
use crate::routes::me::{cookies_of, me_uid};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::auth;
use serde_json::{json, Value};
use std::sync::Arc;

/// `(room id, label)`. Room ids are namespaced so they can't collide with
/// legacy Matrix call rooms on the same SFU.
pub const CHANNELS: &[(&str, &str)] = &[
    ("voice-lounge", "Lounge"),
    ("voice-gaming", "Gaming"),
    ("voice-study", "Study"),
];

const TOKEN_TTL_SECS: i64 = 6 * 3600;

pub fn channel_label(room: &str) -> Option<&'static str> {
    CHANNELS.iter().find(|(r, _)| *r == room).map(|(_, l)| *l)
}

/// Token for one voice channel: join that room only, publish microphone
/// audio only, no data channel.
pub fn voice_token(identity: &str, name: &str, room: &str, now: i64) -> String {
    crate::routes::livekit::sign_jwt(&json!({
        "iss": crate::routes::livekit::api_key(),
        "sub": identity,
        "name": name,
        "iat": now,
        "nbf": now - 10,
        "exp": now + TOKEN_TTL_SECS,
        "video": {
            "room": room,
            "roomJoin": true,
            "canPublish": true,
            "canSubscribe": true,
            "canPublishData": false,
            "canPublishSources": ["microphone"],
        },
    }))
}

/// Short-lived admin token for the LiveKit server API (list participants).
fn admin_token(room: &str, now: i64) -> String {
    crate::routes::livekit::sign_jwt(&json!({
        "iss": crate::routes::livekit::api_key(),
        "sub": "mitch-server",
        "iat": now,
        "nbf": now - 10,
        "exp": now + 60,
        "video": { "roomAdmin": true, "room": room },
    }))
}

fn livekit_http_base() -> String {
    let host = std::env::var("LIVEKIT_HOST").unwrap_or_else(|_| {
        if std::path::Path::new("/.dockerenv").exists() {
            "livekit".to_string()
        } else {
            "127.0.0.1".to_string()
        }
    });
    let port = std::env::var("LIVEKIT_PORT").unwrap_or_else(|_| "7880".to_string());
    format!("http://{host}:{port}")
}

/// Display names of everyone currently in `room`. Best effort: an SFU that is
/// down or a room that doesn't exist yet is just an empty list.
async fn participants(client: &reqwest::Client, room: &str, now: i64) -> Vec<String> {
    let res = client
        .post(format!(
            "{}/twirp/livekit.RoomService/ListParticipants",
            livekit_http_base()
        ))
        .bearer_auth(admin_token(room, now))
        .json(&json!({ "room": room }))
        .send()
        .await;
    let Ok(res) = res else { return vec![] };
    if !res.status().is_success() {
        return vec![];
    }
    let Ok(body) = res.json::<Value>().await else {
        return vec![];
    };
    body.get("participants")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|p| {
                    p.get("name")
                        .and_then(Value::as_str)
                        .filter(|n| !n.is_empty())
                        .or_else(|| p.get("identity").and_then(Value::as_str))
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The signed-in account's normalized email, or the error response.
#[allow(clippy::result_large_err)]
fn signed_in_email(state: &Arc<AppState>, headers: &HeaderMap) -> Result<String, Response> {
    let uid = me_uid(&cookies_of(state, headers));
    if !auth::valid_id(&uid, &state.id_secret) {
        return Err(json_resp(401, json!({ "error": "Sign in to use voice" })));
    }
    if auth::banned_info_for_sid(&state.store, &state.id_secret, &uid).is_some() {
        return Err(json_resp(403, json!({ "error": "account banned" })));
    }
    match auth::email_from_sid(&state.store, &state.id_secret, &uid) {
        Some(e) if !e.is_empty() => Ok(auth::normalize_email(&e)),
        _ => Err(json_resp(401, json!({ "error": "Sign in to use voice" }))),
    }
}

pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Option<Response> {
    let client_ip = crate::handler::get_real_ip(headers, None);
    match (method.clone(), path) {
        (Method::GET, "/api/voice/channels") => {
            if let Some((code, msg)) = state.rate_limit_check(&client_ip, "anon", path) {
                return Some(json_resp(code, json!({ "error": msg })));
            }
            if let Err(resp) = signed_in_email(state, headers) {
                return Some(resp);
            }
            let now = mitch_lib::school::now_millis() / 1000;
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(2))
                .build()
                .ok()?;
            let lists = futures_util::future::join_all(
                CHANNELS
                    .iter()
                    .map(|(room, _)| participants(&client, room, now)),
            )
            .await;
            let channels: Vec<Value> = CHANNELS
                .iter()
                .zip(lists)
                .map(|((id, label), people)| json!({ "id": id, "name": label, "participants": people }))
                .collect();
            Some(json_resp(200, json!({ "channels": channels })))
        }
        (Method::POST, "/api/voice/token") => {
            if let Some((code, msg)) = state.rate_limit_check(&client_ip, "anon", path) {
                return Some(json_resp(code, json!({ "error": msg })));
            }
            let email = match signed_in_email(state, headers) {
                Ok(e) => e,
                Err(resp) => return Some(resp),
            };
            let body: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
            let room = body.get("channel").and_then(Value::as_str).unwrap_or("");
            if channel_label(room).is_none() {
                return Some(json_resp(400, json!({ "error": "Unknown channel" })));
            }
            let identity = crate::routes::matrix::resolve_matrix_user_id_for_email(state, &email);
            let prof = mitch_lib::profiles::profile_get(&state.store, &email);
            let name = prof
                .get("displayName")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    identity
                        .trim_start_matches('@')
                        .split(':')
                        .next()
                        .unwrap_or("user")
                        .to_string()
                });
            let now = mitch_lib::school::now_millis() / 1000;
            let host = crate::hosts::request_host(headers);
            let host = if host.is_empty() {
                "mitch.pro".to_string()
            } else {
                host
            };
            let scheme = if host.starts_with("localhost") || host.starts_with("127.0.0.1") {
                "ws"
            } else {
                "wss"
            };
            Some(json_resp(
                200,
                json!({
                    // livekit-client appends `/rtc`; the site proxies that to the SFU.
                    "url": format!("{scheme}://{host}/livekit"),
                    "token": voice_token(&identity, &name, room, now),
                }),
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn token_is_scoped_to_one_room_and_audio() {
        let jwt = voice_token("@a:mitch.pro", "A", "voice-lounge", 1_000);
        let part = jwt.split('.').nth(1).unwrap();
        let v: Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(part)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(v["video"]["room"], "voice-lounge");
        assert_eq!(v["video"]["canPublishData"], false);
        assert_eq!(v["video"]["canPublishSources"][0], "microphone");
        assert_eq!(v["exp"], 1_000 + TOKEN_TTL_SECS);
    }

    #[test]
    fn only_listed_channels_exist() {
        assert!(channel_label("voice-lounge").is_some());
        assert!(channel_label("default").is_none());
        assert!(channel_label("").is_none());
    }
}
