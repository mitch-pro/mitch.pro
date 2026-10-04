//! `/calls/ws/{room_id}` — binary WebSocket relay for the Matrix voice/video
//! calling fallback.
//!
//! LiveKit (WebRTC/SRTP over UDP, with a TURN/TLS fallback) is the primary
//! calling path. This exists because that still needs *some* UDP or
//! TURN-reachable TLS path to work at all, and on some networks (the home
//! server's own unforwarded RTC ports, restrictive wifi) neither is
//! available. This relay carries pre-encoded media chunks (produced
//! client-side by `MediaRecorder`, played back client-side via
//! `MediaSource`) over the same plain WebSocket/TCP connection that already
//! reaches the site reliably — lower quality and more sensitive to packet
//! loss than real WebRTC, but it only has to ride on a port that's already
//! proven to work everywhere.
//!
//! Not a mixer, not an SFU: every binary frame one participant sends is
//! relayed verbatim to every *other* participant currently connected to the
//! same room id. No history, no persistence — a participant who joins late
//! just doesn't see frames sent before they connected, same as joining a
//! live broadcast partway through.

use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::state::AppState;

const MAX_ROOM_ID_LEN: usize = 256;
/// Caps a single relayed chunk — a few seconds of opus/webm at a sane
/// bitrate is nowhere near this; it exists to stop one misbehaving client
/// from forcing huge allocations onto every other participant's send queue.
const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;

pub fn handle_ws_upgrade(
    state: &Arc<AppState>,
    path: &str,
    headers: &HeaderMap,
    upgrade: Option<WebSocketUpgrade>,
) -> Option<Response> {
    let room_id = path.strip_prefix("/calls/ws/")?;
    if room_id.is_empty() || room_id.len() > MAX_ROOM_ID_LEN {
        return Some(crate::routes::me::json_response(
            400,
            json!({ "error": "invalid room id" }),
        ));
    }
    let room_id = room_id.to_string();

    if !crate::hosts::same_origin_request(headers, Some(&state.cfg)) {
        return Some(crate::routes::me::json_response(
            403,
            json!({ "error": "websocket origin rejected" }),
        ));
    }
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cookies.get("id").unwrap_or(""))
        .to_string();
    let authenticated = !sid.is_empty()
        && mitch_lib::auth::valid_id(&sid, &state.id_secret)
        && state.check_password_cookie(headers, Some(&sid))
        && mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid).is_some();
    if !authenticated {
        return Some(crate::routes::me::json_response(
            401,
            json!({ "error": "authentication required" }),
        ));
    }

    let Some(on_upgrade) = upgrade else {
        return Some(crate::routes::me::json_response(
            400,
            json!({ "error": "websocket upgrade failed" }),
        ));
    };
    let st = Arc::clone(state);
    Some(on_upgrade.on_upgrade(move |socket| async move {
        run_relay_socket(st, socket, room_id).await;
    }))
}

async fn run_relay_socket(state: Arc<AppState>, socket: axum::extract::ws::WebSocket, room_id: String) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let id = state.call_relay_next_id.fetch_add(1, Ordering::Relaxed);

    {
        let mut rooms = state
            .call_relay_rooms
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        rooms.entry(room_id.clone()).or_default().insert(id, tx);
    }

    let forward_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sink.send(msg).await.is_err() {
                break;
            }
        }
    });

    while let Some(frame) = stream.next().await {
        let Ok(msg) = frame else { break };
        match msg {
            Message::Binary(data) => {
                if data.len() > MAX_FRAME_BYTES {
                    continue;
                }
                // Tag every relayed frame with the sender's connection id
                // (8-byte little-endian prefix) so a receiver with more than
                // one remote participant can demux back into separate
                // per-sender MediaSource streams instead of interleaving
                // two different encoders' output into one.
                let mut tagged = Vec::with_capacity(8 + data.len());
                tagged.extend_from_slice(&id.to_le_bytes());
                tagged.extend_from_slice(&data);
                let tagged: axum::body::Bytes = tagged.into();
                let rooms = state
                    .call_relay_rooms
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(peers) = rooms.get(&room_id) {
                    for (peer_id, peer_tx) in peers.iter() {
                        if *peer_id != id {
                            let _ = peer_tx.send(Message::Binary(tagged.clone()));
                        }
                    }
                }
            }
            Message::Close(_) => break,
            // Text/ping/pong: nothing in this relay's protocol uses them.
            _ => {}
        }
    }

    {
        let mut rooms = state
            .call_relay_rooms
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(peers) = rooms.get_mut(&room_id) {
            peers.remove(&id);
            if peers.is_empty() {
                rooms.remove(&room_id);
            }
        }
    }
    forward_task.abort();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> (Arc<AppState>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "mitch-server-call-relay-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap_or_default();
        let cfg = crate::hosts::SiteConfig::load();
        let cfg = crate::hosts::SiteConfig {
            data_dir: dir.to_path_buf(),
            ..cfg
        };
        let store = Arc::new(
            mitch_lib::data::DataStore::open(&dir, &dir).unwrap_or_else(|e| panic!("store: {e}")),
        );
        (Arc::new(AppState::new(cfg, Arc::clone(&store))), dir)
    }

    #[test]
    fn non_matching_path_falls_through() {
        let (state, _dir) = test_state();
        let resp = handle_ws_upgrade(&state, "/ws", &HeaderMap::new(), None);
        assert!(resp.is_none());
    }

    #[test]
    fn empty_room_id_is_rejected() {
        let (state, _dir) = test_state();
        let resp = handle_ws_upgrade(&state, "/calls/ws/", &HeaderMap::new(), None)
            .expect("matching path returns Some");
        assert_eq!(resp.status(), 400);
    }

    #[test]
    fn oversized_room_id_is_rejected() {
        let (state, _dir) = test_state();
        let path = format!("/calls/ws/{}", "x".repeat(MAX_ROOM_ID_LEN + 1));
        let resp = handle_ws_upgrade(&state, &path, &HeaderMap::new(), None)
            .expect("matching path returns Some");
        assert_eq!(resp.status(), 400);
    }

    #[test]
    fn valid_room_id_without_origin_header_is_rejected() {
        // No Origin/Host headers at all: same_origin_request has nothing to
        // match against, so this must not silently pass auth.
        let (state, _dir) = test_state();
        let resp = handle_ws_upgrade(&state, "/calls/ws/!room:mitch.pro", &HeaderMap::new(), None)
            .expect("matching path returns Some");
        assert!(resp.status() == 403 || resp.status() == 401);
    }

    #[tokio::test]
    async fn relay_registry_add_and_remove_round_trips() {
        // Exercises the same room-registry operations run_relay_socket
        // performs, without needing a live socket.
        let (state, _dir) = test_state();
        let (tx, _rx) = mpsc::unbounded_channel::<Message>();
        {
            let mut rooms = state.call_relay_rooms.lock().unwrap();
            rooms
                .entry("room1".to_string())
                .or_default()
                .insert(1, tx);
        }
        {
            let rooms = state.call_relay_rooms.lock().unwrap();
            assert_eq!(rooms.get("room1").map(|p| p.len()), Some(1));
        }
        {
            let mut rooms = state.call_relay_rooms.lock().unwrap();
            if let Some(peers) = rooms.get_mut("room1") {
                peers.remove(&1);
                if peers.is_empty() {
                    rooms.remove("room1");
                }
            }
        }
        {
            let rooms = state.call_relay_rooms.lock().unwrap();
            assert!(rooms.get("room1").is_none());
        }
    }
}
