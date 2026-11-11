//! Voice channels (`/voice/`): audio-only rooms relayed over a WebSocket on
//! the site's own origin. See `docs/voice-channels.md`.
//!
//! No WebRTC, no SFU, no TURN: the browser sends small PCM frames over
//! `/api/voice/ws` and this module fans them out to everyone else in the
//! channel. Because it rides the same hostname and port as the site, it works
//! on every mirror domain and any network that can load the page.
//!
//! - `GET /api/voice/channels` — channel list with who is in each one.
//! - `GET /api/voice/ws?channel=voice-lounge` (WebSocket) — join a channel.
//!
//! Wire protocol (all audio is 16 kHz mono signed 16-bit little-endian):
//! - client → server, binary: one audio frame (≤ [`MAX_FRAME_BYTES`]).
//! - client → server, text: `{"muted": bool}`.
//! - server → client, text: `{"type":"hello","id":N}` once, then
//!   `{"type":"roster","people":[{"id","name","muted"}]}` on every change.
//! - server → client, binary: `u32 big-endian sender id` + the sender's frame.
//!
//! Channel membership lives only in memory and is never written anywhere.
//! Joining needs a signed-in, non-banned account, a same-origin request (so
//! other sites can't open the socket with a visitor's cookies), and a free
//! seat; frames are size- and rate-limited per connection.

use crate::errors::json_resp;
use crate::routes::me::{cookies_of, me_uid};
use crate::state::AppState;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::auth;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::{broadcast, Notify};

/// `(room id, label)`.
pub const CHANNELS: &[(&str, &str)] = &[
    ("voice-lounge", "Lounge"),
    ("voice-gaming", "Gaming"),
    ("voice-study", "Study"),
];

/// People per channel. The relay sends every frame to every listener, so
/// bandwidth grows with the square of this.
pub const MAX_PEERS: usize = 12;
/// 20 ms of 16 kHz mono 16-bit is 640 bytes; allow some slack.
pub const MAX_FRAME_BYTES: usize = 2048;
/// Sustained frames per second a client may send (50 is real-time).
const FRAMES_PER_SEC: f64 = 60.0;
const FRAME_BURST: f64 = 40.0;

enum Event {
    /// Audio from the peer with this id (not delivered back to the sender).
    Audio { from: u32, data: Vec<u8> },
    /// Serialized roster message; the room's membership changed.
    Roster(String),
}

struct Peer {
    id: u32,
    email: String,
    name: String,
    muted: bool,
    /// Wakes the connection's task so it can close itself.
    kick: Arc<Notify>,
}

struct Room {
    tx: broadcast::Sender<Arc<Event>>,
    peers: Vec<Peer>,
}

fn rooms() -> &'static Mutex<HashMap<&'static str, Room>> {
    static ROOMS: OnceLock<Mutex<HashMap<&'static str, Room>>> = OnceLock::new();
    ROOMS.get_or_init(|| {
        Mutex::new(
            CHANNELS
                .iter()
                .map(|(id, _)| {
                    (
                        *id,
                        Room {
                            tx: broadcast::channel(256).0,
                            peers: Vec::new(),
                        },
                    )
                })
                .collect(),
        )
    })
}

fn lock_rooms() -> std::sync::MutexGuard<'static, HashMap<&'static str, Room>> {
    rooms().lock().unwrap_or_else(|e| e.into_inner())
}

fn roster_message(room: &Room) -> String {
    json!({
        "type": "roster",
        "people": room.peers.iter()
            .map(|p| json!({ "id": p.id, "name": p.name, "muted": p.muted }))
            .collect::<Vec<_>>(),
    })
    .to_string()
}

fn announce(room: &Room) {
    // No receivers just means the room emptied.
    let _ = room.tx.send(Arc::new(Event::Roster(roster_message(room))));
}

struct Joined {
    id: u32,
    rx: broadcast::Receiver<Arc<Event>>,
    kick: Arc<Notify>,
}

/// Adds `email` to `room_id`. An account holds one seat per channel: a second
/// join replaces (and kicks) the first, so a stale tab can't hold the seat.
fn join(room_id: &str, email: &str, name: &str) -> Result<Joined, &'static str> {
    static NEXT_ID: AtomicU32 = AtomicU32::new(1);
    let mut map = lock_rooms();
    let room = map.get_mut(room_id).ok_or("Unknown channel")?;
    let (old, keep): (Vec<Peer>, Vec<Peer>) = std::mem::take(&mut room.peers)
        .into_iter()
        .partition(|p| p.email == email);
    room.peers = keep;
    for p in &old {
        p.kick.notify_one();
    }
    if room.peers.len() >= MAX_PEERS {
        announce(room);
        return Err("Channel is full");
    }
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let kick = Arc::new(Notify::new());
    room.peers.push(Peer {
        id,
        email: email.to_string(),
        name: name.to_string(),
        muted: false,
        kick: kick.clone(),
    });
    let rx = room.tx.subscribe();
    announce(room);
    Ok(Joined { id, rx, kick })
}

fn leave(room_id: &str, id: u32) {
    let mut map = lock_rooms();
    if let Some(room) = map.get_mut(room_id) {
        let before = room.peers.len();
        room.peers.retain(|p| p.id != id);
        if room.peers.len() != before {
            announce(room);
        }
    }
}

fn set_muted(room_id: &str, id: u32, muted: bool) {
    let mut map = lock_rooms();
    if let Some(room) = map.get_mut(room_id) {
        if let Some(p) = room.peers.iter_mut().find(|p| p.id == id) {
            if p.muted != muted {
                p.muted = muted;
                announce(room);
            }
        }
    }
}

fn relay_audio(room_id: &str, from: u32, data: Vec<u8>) {
    let map = lock_rooms();
    if let Some(room) = map.get(room_id) {
        let _ = room.tx.send(Arc::new(Event::Audio { from, data }));
    }
}

fn names_in(room_id: &str) -> Vec<String> {
    lock_rooms()
        .get(room_id)
        .map(|r| r.peers.iter().map(|p| p.name.clone()).collect())
        .unwrap_or_default()
}

/// Token bucket: refills at [`FRAMES_PER_SEC`], holds up to [`FRAME_BURST`].
struct Limiter {
    tokens: f64,
    last: std::time::Instant,
}

impl Limiter {
    fn new() -> Self {
        Self {
            tokens: FRAME_BURST,
            last: std::time::Instant::now(),
        }
    }
    fn allow(&mut self) -> bool {
        let now = std::time::Instant::now();
        let dt = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + dt * FRAMES_PER_SEC).min(FRAME_BURST);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
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

/// Plain HTTP routes (`/api/voice/channels`).
pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
) -> Option<Response> {
    if *method != Method::GET || path != "/api/voice/channels" {
        return None;
    }
    let client_ip = crate::handler::get_real_ip(headers, None);
    if let Some((code, msg)) = state.rate_limit_check(&client_ip, "anon", path) {
        return Some(json_resp(code, json!({ "error": msg })));
    }
    if let Err(resp) = signed_in_email(state, headers) {
        return Some(resp);
    }
    let channels: Vec<Value> = CHANNELS
        .iter()
        .map(|(id, label)| json!({ "id": id, "name": label, "participants": names_in(id) }))
        .collect();
    Some(json_resp(200, json!({ "channels": channels })))
}

/// `GET /api/voice/ws?channel=…` upgrade. Returns `None` if this isn't a
/// voice socket so the caller can fall through.
pub fn handle_ws_upgrade(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    search: &str,
    upgrade: Option<WebSocketUpgrade>,
) -> Option<Response> {
    let Some(upgrade) = upgrade else {
        return Some(json_resp(
            400,
            json!({ "error": "websocket upgrade failed" }),
        ));
    };
    if !crate::hosts::same_origin_request(headers, Some(&state.cfg)) {
        return Some(json_resp(
            403,
            json!({ "error": "websocket origin rejected" }),
        ));
    }
    let email = match signed_in_email(state, headers) {
        Ok(e) => e,
        Err(resp) => return Some(resp),
    };
    let room = form_urlencoded::parse(search.trim_start_matches('?').as_bytes())
        .find(|(k, _)| k == "channel")
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default();
    let Some(room_id) = CHANNELS.iter().find(|(r, _)| *r == room).map(|(r, _)| *r) else {
        return Some(json_resp(400, json!({ "error": "Unknown channel" })));
    };
    let identity = crate::routes::matrix::resolve_matrix_user_id_for_email(state, &email);
    let prof = mitch_lib::profiles::profile_get(&state.store, &email);
    let name = prof
        .get("displayName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            identity
                .trim_start_matches('@')
                .split(':')
                .next()
                .unwrap_or("user")
                .to_string()
        });
    let state = Arc::clone(state);
    Some(upgrade.on_upgrade(move |socket| async move {
        run_peer(state, socket, room_id, email, name).await;
    }))
}

fn close_frame(code: u16, reason: &'static str) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: reason.into(),
    }))
}

async fn run_peer(
    state: Arc<AppState>,
    mut socket: WebSocket,
    room_id: &'static str,
    email: String,
    name: String,
) {
    let Joined { id, mut rx, kick } = match join(room_id, &email, &name) {
        Ok(j) => j,
        Err(reason) => {
            let _ = socket.send(close_frame(4001, reason)).await;
            return;
        }
    };
    let hello = json!({ "type": "hello", "id": id }).to_string();
    if socket.send(Message::Text(hello.into())).await.is_err() {
        leave(room_id, id);
        return;
    }
    let mut limiter = Limiter::new();
    let mut ban_check = tokio::time::interval(std::time::Duration::from_secs(2));
    loop {
        tokio::select! {
            _ = kick.notified() => {
                let _ = socket.send(close_frame(4000, "Joined from another tab")).await;
                break;
            }
            _ = ban_check.tick() => {
                if auth::banned_info_for_email(&state.store, &email).is_some() {
                    let _ = socket.send(close_frame(1008, "Account banned")).await;
                    break;
                }
            }
            ev = rx.recv() => match ev {
                Ok(ev) => {
                    let msg = match &*ev {
                        Event::Audio { from, .. } if *from == id => continue,
                        Event::Audio { from, data } => {
                            let mut out = Vec::with_capacity(4 + data.len());
                            out.extend_from_slice(&from.to_be_bytes());
                            out.extend_from_slice(data);
                            Message::Binary(out.into())
                        }
                        Event::Roster(json) => Message::Text(json.clone().into()),
                    };
                    if socket.send(msg).await.is_err() {
                        break;
                    }
                }
                // A slow listener just misses audio; a roster it missed is
                // corrected by the next change.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            },
            frame = socket.recv() => match frame {
                Some(Ok(Message::Binary(data))) => {
                    if data.len() <= MAX_FRAME_BYTES && !data.is_empty() && limiter.allow() {
                        relay_audio(room_id, id, data.to_vec());
                    }
                }
                Some(Ok(Message::Text(text))) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&text) {
                        if let Some(m) = v.get("muted").and_then(Value::as_bool) {
                            set_muted(room_id, id, m);
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
    leave(room_id, id);
}

#[cfg(test)]
mod tests {
    use super::*;

    // The registry is process-global; each test uses its own email prefix and
    // checks only its own seats, and `reset` clears a room between tests that
    // depend on its size.
    fn reset(room: &str) {
        let mut map = lock_rooms();
        if let Some(r) = map.get_mut(room) {
            r.peers.clear();
        }
    }

    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn only_listed_channels_exist() {
        assert!(join("default", "a@x", "A").is_err());
        assert!(join("", "a@x", "A").is_err());
    }

    #[test]
    fn join_leave_updates_roster() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        reset("voice-study");
        let a = join("voice-study", "a@x", "Ann").unwrap();
        let b = join("voice-study", "b@x", "Bob").unwrap();
        assert_eq!(names_in("voice-study"), vec!["Ann", "Bob"]);
        assert_ne!(a.id, b.id);
        leave("voice-study", a.id);
        assert_eq!(names_in("voice-study"), vec!["Bob"]);
        reset("voice-study");
    }

    #[test]
    fn second_join_replaces_the_first_seat() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        reset("voice-gaming");
        let first = join("voice-gaming", "a@x", "Ann").unwrap();
        let second = join("voice-gaming", "a@x", "Ann").unwrap();
        assert_eq!(names_in("voice-gaming").len(), 1);
        assert_ne!(first.id, second.id);
        reset("voice-gaming");
    }

    #[test]
    fn channel_full_is_rejected() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        reset("voice-lounge");
        for i in 0..MAX_PEERS {
            join("voice-lounge", &format!("u{i}@x"), "U").unwrap();
        }
        assert!(join("voice-lounge", "extra@x", "X").is_err());
        // An existing member re-joining still gets their seat.
        assert!(join("voice-lounge", "u0@x", "U").is_ok());
        reset("voice-lounge");
    }

    #[test]
    fn audio_reaches_others_with_sender_id() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        reset("voice-study");
        let a = join("voice-study", "a@x", "Ann").unwrap();
        let mut b = join("voice-study", "b@x", "Bob").unwrap();
        relay_audio("voice-study", a.id, vec![1, 2, 3]);
        let mut saw = false;
        while let Ok(ev) = b.rx.try_recv() {
            if let Event::Audio { from, data } = &*ev {
                assert_eq!(*from, a.id);
                assert_eq!(data, &vec![1, 2, 3]);
                saw = true;
            }
        }
        assert!(saw);
        reset("voice-study");
    }

    #[test]
    fn limiter_caps_a_flood_but_allows_realtime() {
        let mut l = Limiter::new();
        let burst = (0..1000).filter(|_| l.allow()).count();
        assert!(burst <= FRAME_BURST as usize + 2, "{burst}");
        let mut l = Limiter::new();
        // 50 fps for a second of simulated time stays under the refill rate.
        l.tokens = 1.0;
        std::thread::sleep(std::time::Duration::from_millis(40));
        assert!(l.allow());
    }
}
