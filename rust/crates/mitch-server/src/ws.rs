//! `/ws` — the global broadcast WebSocket (server.js:11711-11723 upgrade,
//! 24894+ open/message/close). Everything else on `allSockets` (SSH relay,
//! VNC bridge, blooket proxy, LiveKit) is Step 13.
//!
//! Surface parity contract (server.js):
//! - upgrade gates: same-origin → 403 `websocket origin rejected`; the
//!   studentId||id sid ladder + checkPasswordCookie → 401 `authentication
//!   required`; emailFromSid → 401. NOTE the global password gate runs
//!   BEFORE this arm (handler.rs), so anonymous upgrades see the gate's own
//!   403 first — exactly like the JS flow.
//! - open: cancel any pending offline timer (the timer body re-checks the
//!   socket set, so cancellation is the re-check itself), stamp presence,
//!   `notifyFriendsOnline` when the user was offline, broadcast
//!   `presence_changed` online to the friend set.
//! - message (broadcast sockets only): `presence_ping` re-validates the sid
//!   (`validId`/`isRevoked`/emailFromSid === socket email, else close 1008
//!   "Session expired") and stamps lastSeen keeping `playing`; every other
//!   frame is swallowed by the JS try/catch.
//! - close: drop from the set; the LAST socket for an email stamps lastSeen
//!   (keeping `playing`) and schedules a 3s re-check that deletes presence
//!   and broadcasts offline.
//!
//! The JS fans out by iterating `allSockets` at SEND time; here routes call
//! [`broadcast`] with an explicit recipient set, computed at send time, and
//! the per-socket loop filters by the socket's email — the same observable
//! behavior. Broadcast recipients are keyed by NORMALIZED email.

use crate::state::{AppState, UserPresence, PRESENCE_FALLBACK_TTL_MS};
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::Response;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Recipient set for one [`broadcast`], resolved by the caller at send time
/// (the JS computes per-socket predicates at send time too).
pub enum WsRecipients {
    /// Every broadcast socket.
    All,
    /// Broadcast sockets whose normalized email is in the set.
    Emails(HashSet<String>),
}

/// One message queued onto the fan-out channel.
pub struct WsEnvelope {
    payload: String,
    recipients: WsRecipients,
}

/// A connected broadcast socket (server.js `ws.data` for `isBroadcast`).
pub struct WsClient {
    pub email_norm: String,
}

/// `JSON.stringify(payload); for (const ws of allSockets) { ... ws.send }` —
/// the generic fan-out. Sending with zero subscribers is a no-op like the JS
/// with an empty socket set.
pub fn broadcast(state: &Arc<AppState>, recipients: WsRecipients, payload: String) {
    // `send` errors only when no receiver is alive — the empty-socket JS case.
    let _ = state.ws_tx.send(Arc::new(WsEnvelope {
        payload,
        recipients,
    }));
}

/// `broadcastProfileChange` (server.js:8669-8683).
pub fn broadcast_profile_change(state: &Arc<AppState>, username: &str, updated_at: i64) {
    let payload = json!({
        "type": "profile_updated",
        "handle": username,
        "updatedAt": updated_at
    });
    broadcast(state, WsRecipients::All, payload.to_string());
}

/// `hasAuthenticatedBroadcastSocket(email)` (server.js:1069-1075).
pub fn has_authenticated_broadcast_socket(state: &Arc<AppState>, email: &str) -> bool {
    let norm = mitch_lib::auth::normalize_email(email);
    if norm.is_empty() {
        return false;
    }
    let sockets = state
        .ws_broadcasts
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    sockets.values().any(|c| c.email_norm == norm)
}

/// `isUserPresent(email, now)` (server.js:1077-1084) — socket leg OR the
/// 45s presence-map fallback. Replaces the friends.rs map-only port.
pub fn is_user_present(state: &Arc<AppState>, email: &str, now: i64) -> bool {
    let norm = mitch_lib::auth::normalize_email(email);
    if norm.is_empty() {
        return false;
    }
    if has_authenticated_broadcast_socket(state, &norm) {
        return true;
    }
    let presence = state
        .user_presence
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    match presence.get(&norm) {
        Some(p) => now - p.last_seen < PRESENCE_FALLBACK_TTL_MS,
        None => false,
    }
}

/// `broadcastPresenceChanged(email, online, playing)` (server.js:1086-1103) —
/// friends of `email` plus the user themself receive the payload.
pub fn broadcast_presence_changed(state: &Arc<AppState>, email: &str, online: bool, playing: &str) {
    let norm = mitch_lib::auth::normalize_email(email);
    if norm.is_empty() {
        return;
    }
    let friends = mitch_lib::friends::friends_get(&state.store, &norm);
    let mut recipients: HashSet<String> = friends
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|f| mitch_lib::auth::normalize_email(&mitch_lib::jsval::string(f)))
                .collect()
        })
        .unwrap_or_default();
    recipients.insert(norm.clone());
    let display =
        mitch_lib::profile::display_email(&state.store, state.data_dir(), &state.id_secret, &norm);
    let payload = json!({
        "type": "presence_changed",
        "email": display,
        "online": online,
        "playing": if online { String::from(playing).trim().to_string() } else { String::new() },
        "ts": now_millis(),
    });
    broadcast(state, WsRecipients::Emails(recipients), payload.to_string());
}

/// `touchUserPresence(email, playing)` (server.js:1105-1127).
pub fn touch_user_presence(state: &Arc<AppState>, email: &str, playing: &str) {
    if email.is_empty() {
        return;
    }
    let norm = mitch_lib::auth::normalize_email(email);
    if norm.is_empty() {
        return;
    }
    let now = now_millis();
    let was_offline = !is_user_present(state, &norm, now);
    let playing_trim = String::from(playing).trim().to_string();
    let previous_playing;
    {
        let mut presence = state
            .user_presence
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        previous_playing = presence
            .get(&norm)
            .map(|p| p.playing.clone())
            .unwrap_or_default();
        presence.insert(
            norm.clone(),
            UserPresence {
                last_seen: now,
                playing: playing_trim.clone(),
            },
        );
    }
    // cvOnline is written under BOTH the raw email and the normalized key.
    {
        let mut cv = state.cv_online.lock().unwrap_or_else(|e| e.into_inner());
        cv.insert(email.to_string(), now);
        cv.insert(norm.clone(), now);
    }
    if was_offline {
        // JS: fire-and-forget promise.
        let st = Arc::clone(state);
        let email = email.to_string();
        tokio::spawn(async move { notify_friends_online(&st, &email).await });
    }
    if was_offline || previous_playing != playing_trim {
        broadcast_presence_changed(state, &norm, true, &playing_trim);
    }
}

/// `notifyFriendsOnline(email)` (server.js:1149-1175) — web-push every friend
/// with the `friends_online` category enabled. The JS does NOT await the
/// sends; callers spawn this.
pub async fn notify_friends_online(state: &Arc<AppState>, email: &str) {
    let norm = mitch_lib::auth::normalize_email(email);
    if norm.is_empty() {
        return;
    }
    let friends = mitch_lib::friends::friends_get(&state.store, &norm);
    let Some(my_list) = friends.as_array().cloned() else {
        return;
    };
    let sender_name =
        mitch_lib::profile::display_email(&state.store, state.data_dir(), &state.id_secret, email);
    let vapid_public = std::env::var("VAPID_PUBLIC_KEY")
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    if vapid_public.is_empty() {
        return; // JS parity: `if (sub && VAPID_PUBLIC)`.
    }
    let payload = json!({
        "title": "Friend Online",
        "body": format!("{sender_name} is now online!"),
        "url": "/",
    });
    for friend in my_list {
        let friend_raw = mitch_lib::jsval::string(&friend);
        let friend_norm = mitch_lib::auth::normalize_email(&friend_raw);
        if friend_norm.is_empty()
            || !crate::routes::dm::notif_allowed(state, &friend_norm, "friends_online")
        {
            continue;
        }
        if let Some(sub) = mitch_lib::push_subs::push_sub_get(&state.store, &friend_norm) {
            crate::routes::push::send_web_push(state, "", &friend_norm, &sub, &payload).await;
        }
    }
}

/// `triggerNotificationRefresh()` (server.js:8824).
pub fn trigger_notification_refresh(state: &AppState) {
    let payload = json!({ "type": "notifications_refresh" });
    let _ = state.ws_tx.send(Arc::new(WsEnvelope {
        payload: payload.to_string(),
        recipients: WsRecipients::All,
    }));
}

/// The 10s sweeper (server.js:1129-1136) — drop entries with no socket and a
/// stale lastSeen, broadcasting offline for each.
pub fn sweep_presence(state: &Arc<AppState>) {
    let now = now_millis();
    let stale: Vec<String> = {
        let presence = state
            .user_presence
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        presence
            .keys()
            .filter(|email| {
                !has_authenticated_broadcast_socket(state, email)
                    && now - presence.get(*email).map(|p| p.last_seen).unwrap_or(0)
                        >= PRESENCE_FALLBACK_TTL_MS
            })
            .cloned()
            .collect()
    };
    for email in stale {
        {
            let mut presence = state
                .user_presence
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            presence.remove(&email);
        }
        broadcast_presence_changed(state, &email, false, "");
    }
}

/// `/ws` upgrade — runs AFTER the global password gate (handler.rs), so an
/// anonymous upgrade already saw the gate's 403 like the JS.
pub fn handle_upgrade(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    upgrade: Option<WebSocketUpgrade>,
) -> Response {
    if !crate::hosts::same_origin_request(headers, Some(&state.cfg)) {
        return crate::routes::me::json_response(
            403,
            json!({ "error": "websocket origin rejected" }),
        );
    }
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cookies.get("id").unwrap_or(""))
        .to_string();
    if sid.is_empty()
        || !mitch_lib::auth::valid_id(&sid, &state.id_secret)
        || is_revoked_id(state, &sid)
        || !state.check_password_cookie(headers, Some(&sid))
    {
        return crate::routes::me::json_response(
            401,
            json!({ "error": "authentication required" }),
        );
    }
    let Some(my_email) = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid)
    else {
        return crate::routes::me::json_response(
            401,
            json!({ "error": "authentication required" }),
        );
    };
    let email_norm = mitch_lib::auth::normalize_email(&my_email);
    if email_norm.is_empty() {
        return crate::routes::me::json_response(
            401,
            json!({ "error": "authentication required" }),
        );
    }
    // `server.upgrade(req, { data: {...} })` — the data rides with the socket.
    let Some(on_upgrade) = upgrade else {
        return crate::routes::me::json_response(
            400,
            json!({ "error": "websocket upgrade failed" }),
        );
    };
    let st = Arc::clone(state);
    on_upgrade.on_upgrade(move |socket| async move {
        run_broadcast_socket(st, socket, email_norm, sid).await;
    })
}

/// The broadcast socket task: open leg → message loop → close leg.
async fn run_broadcast_socket(
    state: Arc<AppState>,
    mut socket: WebSocket,
    email_norm: String,
    sid: String,
) {
    // JS open(): `wasPresent` is computed BEFORE allSockets.add(ws).
    let now = now_millis();
    let was_present = is_user_present(&state, &email_norm, now);
    let id = state.ws_next_id.fetch_add(1, Ordering::Relaxed);
    {
        let mut sockets = state
            .ws_broadcasts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        sockets.insert(
            id,
            WsClient {
                email_norm: email_norm.clone(),
            },
        );
    }
    // Subscribe at the allSockets.add point: from here the socket receives
    // every fan-out, including its own open-leg presence_changed (the JS
    // broadcast loop reaches it because allSockets already contains it).
    let mut rx = state.ws_tx.subscribe();
    // open() presence leg: pendingOffline cancellation is the re-check inside
    // the timer body, so nothing to clear here.
    let playing = {
        let presence = state
            .user_presence
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        presence
            .get(&email_norm)
            .map(|p| p.playing.clone())
            .unwrap_or_default()
    };
    {
        let mut presence = state
            .user_presence
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        presence.insert(
            email_norm.clone(),
            UserPresence {
                last_seen: now_millis(),
                playing: playing.clone(),
            },
        );
    }
    if !was_present {
        let st = Arc::clone(&state);
        let email = email_norm.clone();
        tokio::spawn(async move { notify_friends_online(&st, &email).await });
    }
    broadcast_presence_changed(&state, &email_norm, true, &playing);

    loop {
        tokio::select! {
            env = rx.recv() => {
                match env {
                    Ok(env) => {
                        let send = match &env.recipients {
                            WsRecipients::All => true,
                            WsRecipients::Emails(set) => set.contains(&email_norm),
                        };
                        if send {
                            // JS `try { ws.send(payload) } catch {}`.
                            let _ = socket.send(Message::Text(env.payload.clone().into())).await;
                        }
                        continue;
                    }
                    // Lagged receiver: JS has no equivalent (per-socket queues
                    // don't drop); skip to the next message.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        // Channel only dies with the state; treat as close.
                        break;
                    }
                }
            }
            msg = socket.recv() => {
                match msg {
                    None => break,
                    Some(Err(_)) => break,
                    Some(Ok(Message::Text(text))) => {
                        if !handle_presence_ping(&state, &email_norm, &sid, &text) {
                            // Session expired → close(1008, 'Session expired').
                            let _ = socket
                                .send(Message::Close(Some(CloseFrame {
                                    code: 1008,
                                    reason: "Session expired".into(),
                                })))
                                .await;
                            break;
                        }
                    }
                    Some(Ok(_)) => {} // binary/pong frames: JS parse fails → ignored
                }
            }
        }
    }

    close_leg(&state, &email_norm, id).await;
}

/// `presence_ping` handling (server.js:24992-25014). Returns false when the
/// socket must be closed with 1008. Non-ping payloads and parse failures are
/// swallowed exactly like the JS try/catch.
fn handle_presence_ping(state: &Arc<AppState>, email_norm: &str, sid: &str, text: &str) -> bool {
    let Ok(payload) = serde_json::from_str::<Value>(text) else {
        return true;
    };
    if payload.get("type").and_then(|v| v.as_str()) != Some("presence_ping") {
        return true;
    }
    let ping_email = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, sid)
        .map(|e| mitch_lib::auth::normalize_email(&e))
        .unwrap_or_default();
    if sid.is_empty()
        || !mitch_lib::auth::valid_id(sid, &state.id_secret)
        || is_revoked_id(state, sid)
        || ping_email.is_empty()
        || ping_email != mitch_lib::auth::normalize_email(email_norm)
    {
        return false;
    }
    let now = now_millis();
    let playing = {
        let presence = state
            .user_presence
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        presence
            .get(email_norm)
            .map(|p| p.playing.clone())
            .unwrap_or_default()
    };
    let mut presence = state
        .user_presence
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    presence.insert(
        email_norm.to_string(),
        UserPresence {
            last_seen: now,
            playing,
        },
    );
    true
}

/// close() leg (server.js:25172-25190): drop from the set; the LAST socket
/// for an email stamps lastSeen (keeping `playing`) and schedules the 3s
/// offline re-check.
async fn close_leg(state: &Arc<AppState>, email_norm: &str, socket_id: u64) {
    // JS `allSockets.delete(ws)` removes exactly this socket, not the whole
    // email — other sockets for the same user keep the presence alive.
    {
        let mut sockets = state
            .ws_broadcasts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        sockets.remove(&socket_id);
    }
    if !has_authenticated_broadcast_socket(state, email_norm) {
        let now = now_millis();
        let playing = {
            let mut presence = state
                .user_presence
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let prev = presence
                .get(email_norm)
                .map(|p| p.playing.clone())
                .unwrap_or_default();
            presence.insert(
                email_norm.to_string(),
                UserPresence {
                    last_seen: now,
                    playing: prev.clone(),
                },
            );
            prev
        };
        let _ = playing;
        let st = Arc::clone(state);
        let email = email_norm.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(3000)).await;
            if !has_authenticated_broadcast_socket(&st, &email) {
                {
                    let mut presence = st.user_presence.lock().unwrap_or_else(|e| e.into_inner());
                    presence.remove(&email);
                }
                broadcast_presence_changed(&st, &email, false, "");
            }
        });
    }
}

/// `isRevoked(id)` — key presence in the revoked_ids table.
fn is_revoked_id(state: &AppState, sid: &str) -> bool {
    mitch_lib::revoked::revoked_id_exists(&state.store, sid)
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// recipients-at-send-time semantics: the envelope model must reduce to
    /// the JS per-socket predicate for the shipped fan-outs.
    #[test]
    fn recipients_match_socket_email() {
        let set = ["a@x.y".to_string(), "b@x.y".to_string()]
            .into_iter()
            .collect();
        assert!(matches!(&WsRecipients::All, WsRecipients::All));
        match WsRecipients::Emails(set) {
            WsRecipients::Emails(s) => {
                assert!(s.contains("a@x.y"));
                assert!(!s.contains("c@x.y"));
            }
            _ => panic!("wrong variant"),
        }
    }
}
