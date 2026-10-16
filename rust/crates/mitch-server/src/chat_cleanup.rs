//! Ban cleanup and permanent history tombstones, independent of ban expiry.
use crate::{routes::matrix, state::AppState};
use axum::{
    body::Bytes,
    http::{HeaderMap, HeaderValue, Method},
};
use mitch_lib::{auth::encode_uri_component as enc, chat_cleanup as data};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc};

pub fn clear_relay(state: &AppState, email: &str) {
    let ids: Vec<String> = state
        .e2e_users
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|(_, u)| mitch_lib::auth::normalize_email(&u.email) == email)
        .map(|(id, _)| id.clone())
        .collect();
    state
        .e2e_messages
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values_mut()
        .for_each(|messages| messages.retain(|m| !ids.contains(&m.from)));
}

fn sender_cutoffs(state: &AppState) -> HashMap<String, (String, i64)> {
    let removals = data::account_removals(&state.store);
    let mut result = data::identity_cutoffs(&state.store);
    for (email, cutoff) in removals {
        let ids = matrix::matrix_user_ids_for_account(state, &email);
        data::remember_identities(&state.store, &email, cutoff, &ids);
        for id in ids {
            result.entry(id).or_insert((email.clone(), cutoff));
        }
    }
    result
}

fn message_event(event: &Value) -> bool {
    event.get("event_id").is_some()
        && event.get("state_key").is_none()
        && event.get("type").and_then(Value::as_str) != Some("m.room.redaction")
}
fn collect_media(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::String(uri) if uri.starts_with("mxc://") => found.push(uri.clone()),
        Value::Array(values) => values.iter().for_each(|v| collect_media(v, found)),
        Value::Object(values) => values.values().for_each(|v| collect_media(v, found)),
        _ => {}
    }
}
fn redact_tree(
    value: &mut Value,
    cutoffs: &HashMap<String, (String, i64)>,
    media: &mut Vec<(String, String, i64)>,
) {
    if message_event(value) {
        let sender = value.get("sender").and_then(Value::as_str).unwrap_or("");
        if let Some((email, cutoff)) = cutoffs.get(sender) {
            let ts = value
                .get("origin_server_ts")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            if ts <= *cutoff {
                let mut uris = Vec::new();
                collect_media(&value["content"], &mut uris);
                media.extend(uris.into_iter().map(|uri| (uri, email.clone(), ts)));
                value["content"] = json!({});
                value["unsigned"] = json!({"redacted_because":{"type":"m.room.redaction","sender":"@mitch_admin:mitch.pro","content":{"reason":"Removed by moderation"}}});
            }
        }
    }
    match value {
        Value::Array(values) => values
            .iter_mut()
            .for_each(|v| redact_tree(v, cutoffs, media)),
        Value::Object(values) => values
            .values_mut()
            .for_each(|v| redact_tree(v, cutoffs, media)),
        _ => {}
    }
}

pub fn filter_matrix_response(state: &AppState, bytes: Bytes) -> Bytes {
    let cutoffs = sender_cutoffs(state);
    if cutoffs.is_empty() {
        return bytes;
    }
    let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
        return bytes;
    };
    let mut media = Vec::new();
    redact_tree(&mut value, &cutoffs, &mut media);
    for (uri, email, ts) in media {
        data::remember_media(&state.store, &uri, &email, ts, true);
    }
    Bytes::from(serde_json::to_vec(&value).unwrap_or_else(|_| bytes.to_vec()))
}

pub fn blocked_media_path(state: &AppState, path: &str) -> bool {
    let parts: Vec<_> = path.split('/').collect();
    let Some(pos) = parts
        .iter()
        .position(|p| matches!(*p, "download" | "thumbnail"))
    else {
        return false;
    };
    if !path.starts_with("/_matrix/") || parts.len() <= pos + 2 {
        return false;
    }
    let decode = |s: &str| {
        url::form_urlencoded::parse(format!("v={}", s.replace('+', "%2B")).as_bytes())
            .next()
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default()
    };
    data::media_blocked(
        &state.store,
        &format!(
            "mxc://{}/{}",
            decode(parts[pos + 1]),
            decode(parts[pos + 2])
        ),
    )
}

async fn request(
    token: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let mut headers = HeaderMap::new();
    headers.insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| "Invalid cleanup token")?,
    );
    headers.insert("Content-Type", HeaderValue::from_static("application/json"));
    let (status, _, bytes) = matrix::call_conduit_with_timeout(
        path,
        method,
        Some(headers),
        body.map(|v| Bytes::from(v.to_string())),
        Some(std::time::Duration::from_secs(20)),
    )
    .await
    .map_err(|_| "Chat server unavailable")?;
    if !status.is_success() {
        return Err(format!("Chat server returned {}", status.as_u16()));
    }
    serde_json::from_slice(&bytes).map_err(|_| "Invalid chat response".into())
}

async fn cleanup_matrix(state: &AppState, email: &str, cutoff: i64) -> Result<(), String> {
    let mut ids: Vec<String> = sender_cutoffs(state)
        .into_iter()
        .filter(|(_, (owner, _))| owner == email)
        .map(|(id, _)| id)
        .collect();
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        return Ok(());
    }
    let Some(username) = matrix::matrix_username_for_email(state, email).await else {
        return Ok(());
    };
    let password = matrix::get_matrix_password_for_email(email, &state.id_secret);
    let payload = json!({"type":"m.login.password","identifier":{"type":"m.id.user","user":username},"password":password,"device_id":"MITCH_MODERATION_CLEANUP","initial_device_display_name":"Account moderation"});
    // A fixed device avoids creating a new device on every retry. Never deactivates the account.
    let mut login_headers = HeaderMap::new();
    login_headers.insert("Content-Type", HeaderValue::from_static("application/json"));
    let (_, _, bytes) = matrix::call_conduit(
        "/_matrix/client/v3/login",
        Method::POST,
        Some(login_headers),
        Some(Bytes::from(payload.to_string())),
    )
    .await
    .map_err(|_| "Could not open chat cleanup session")?;
    let login: Value = serde_json::from_slice(&bytes).map_err(|_| "Invalid chat login response")?;
    let Some(token) = login["access_token"].as_str() else {
        if login["errcode"] == "M_FORBIDDEN" {
            // No Matrix account has been created yet: no room history to erase.
            let mapped = state
                .store
                .read_document(&state.data_dir().join("matrix_users.json"), json!({}));
            if mapped.get(email).is_none() {
                return Ok(());
            }
        }
        return Err("Could not authenticate chat cleanup".into());
    };
    let sync=request(token,Method::GET,"/_matrix/client/v3/sync?full_state=true&timeout=0&filter=%7B%22room%22%3A%7B%22include_leave%22%3Atrue%2C%22timeline%22%3A%7B%22limit%22%3A0%7D%7D%7D",None).await?;
    let mut rooms = Vec::new();
    for category in ["join", "leave"] {
        if let Some(map) = sync["rooms"][category].as_object() {
            rooms.extend(map.keys().cloned());
        }
    }
    let admin = matrix::get_system_admin_matrix_token(&state.id_secret)
        .await
        .ok();
    for room in rooms {
        let base = format!("/_matrix/client/v3/rooms/{}", enc(&room));
        let filter = enc(&json!({"senders":ids,"not_types":["m.room.member"]}).to_string());
        let mut from = String::new();
        let mut finished = false;
        for _ in 0..10000 {
            let url = format!(
                "{base}/messages?dir=b&limit=100&filter={filter}{}",
                if from.is_empty() {
                    String::new()
                } else {
                    format!("&from={}", enc(&from))
                }
            );
            let page = request(token, Method::GET, &url, None).await?;
            let chunk = page["chunk"]
                .as_array()
                .ok_or("Invalid chat history response")?;
            for event in chunk {
                let ts = event["origin_server_ts"].as_i64().unwrap_or(0);
                if ts > cutoff
                    || !message_event(event)
                    || !ids
                        .iter()
                        .any(|id| Some(id.as_str()) == event["sender"].as_str())
                {
                    continue;
                }
                let mut media = Vec::new();
                collect_media(&event["content"], &mut media);
                for uri in media {
                    data::remember_media(&state.store, &uri, email, ts, true);
                }
                if event["unsigned"].get("redacted_because").is_some() {
                    continue;
                }
                let Some(id) = event["event_id"].as_str() else {
                    continue;
                };
                let txn = mitch_lib::crypto::hmac_sha256_hex(&state.id_secret, id.as_bytes());
                let redact = format!("{base}/redact/{}/{txn}", enc(id));
                let body = json!({"reason":"Removed by account moderation"});
                if request(token, Method::PUT, &redact, Some(body.clone()))
                    .await
                    .is_err()
                {
                    if let Some(admin) = admin.as_ref() {
                        request(admin, Method::PUT, &redact, Some(body)).await?;
                    } else {
                        return Err("Could not redact chat event".into());
                    }
                }
            }
            let next = page["end"].as_str().unwrap_or("");
            if chunk.is_empty() || next.is_empty() || next == from {
                finished = true;
                break;
            }
            from = next.to_string();
        }
        if !finished {
            return Err("Chat cleanup needs another pass".into());
        }
    }
    Ok(())
}

pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let _ = data::remove_files(&state.store);
            let jobs = data::pending_jobs(&state.store);
            for (email, cutoff) in jobs {
                clear_relay(&state, &email);
                let result = cleanup_matrix(&state, &email, cutoff).await;
                let (done, error) = match result {
                    Ok(()) => (true, String::new()),
                    Err(error) => (false, error),
                };
                data::finish_job(&state.store, &email, cutoff, done, &error);
                if !done {
                    tracing::warn!("Chat cleanup is pending; it will retry automatically");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tombstones_cover_encrypted_gifs_and_search_without_removing_new_messages_or_membership() {
        let mut value = json!({"chunk":[
            {"event_id":"$a","sender":"@bad:mitch.pro","origin_server_ts":5,"type":"m.room.message","content":{"body":"gif","url":"mxc://mitch.pro/gif"}},
            {"event_id":"$b","sender":"@bad:mitch.pro","origin_server_ts":6,"type":"m.room.encrypted","content":{"ciphertext":"secret"}},
            {"event_id":"$c","sender":"@good:mitch.pro","origin_server_ts":5,"type":"m.room.message","content":{"body":"keep"}},
            {"event_id":"$d","sender":"@bad:mitch.pro","origin_server_ts":20,"type":"m.room.message","content":{"body":"new"}},
            {"event_id":"$e","sender":"@bad:mitch.pro","origin_server_ts":1,"type":"m.room.member","state_key":"@bad:mitch.pro","content":{"membership":"join"}}
        ]});
        let cutoffs = HashMap::from([(
            "@bad:mitch.pro".to_string(),
            ("bad@example.com".to_string(), 10),
        )]);
        let mut media = Vec::new();
        redact_tree(&mut value, &cutoffs, &mut media);
        assert_eq!(value["chunk"][0]["content"], json!({}));
        assert_eq!(value["chunk"][1]["content"], json!({}));
        assert_eq!(value["chunk"][2]["content"]["body"], "keep");
        assert_eq!(value["chunk"][3]["content"]["body"], "new");
        assert_eq!(value["chunk"][4]["content"]["membership"], "join");
        assert_eq!(media.len(), 1);
    }
}
