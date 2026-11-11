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
//! Matrix calls (the chat page at `/matrix/`): a call in a Matrix room is a
//! channel named `dm:<room id>` that exists only while someone is in it.
//! - `POST /api/voice/call {room_id, mx_token}` — checks the caller really is
//!   in that room (using their own Matrix token), rings the other members and
//!   returns a one-time `ticket` for the socket (`&ticket=` on the URL).
//! - `GET /api/voice/incoming` — calls ringing for the signed-in account.
//! - `POST /api/voice/decline {room_id}` — dismiss a ring.
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
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, Notify};

/// `(room id, label)`.
pub const CHANNELS: &[(&str, &str)] = &[
    ("voice-lounge", "Lounge"),
    ("voice-gaming", "Gaming"),
    ("voice-study", "Study"),
];

/// Matrix rooms with more joined members than this can't be called (the
/// official public rooms have hundreds, and a call rings every member).
pub const MAX_CALL_ROOM_MEMBERS: usize = 8;
/// How long a call keeps ringing for people who haven't answered.
const RING_SECS: u64 = 45;
/// A ticket must be used within this long of being issued.
const TICKET_TTL: Duration = Duration::from_secs(60);
const DM_PREFIX: &str = "dm:";

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

fn rooms() -> &'static Mutex<HashMap<String, Room>> {
    static ROOMS: OnceLock<Mutex<HashMap<String, Room>>> = OnceLock::new();
    ROOMS.get_or_init(|| {
        Mutex::new(
            CHANNELS
                .iter()
                .map(|(id, _)| {
                    (
                        (*id).to_string(),
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

fn lock_rooms() -> std::sync::MutexGuard<'static, HashMap<String, Room>> {
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
///
/// `dm:` rooms are created on first join (callers must have passed
/// [`redeem_ticket`] first); fixed channels always exist.
fn join(room_id: &str, email: &str, name: &str) -> Result<Joined, &'static str> {
    static NEXT_ID: AtomicU32 = AtomicU32::new(1);
    let mut map = lock_rooms();
    if room_id.starts_with(DM_PREFIX) && !map.contains_key(room_id) {
        map.insert(
            room_id.to_string(),
            Room {
                tx: broadcast::channel(256).0,
                peers: Vec::new(),
            },
        );
    }
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
    let mut emptied_call = false;
    if let Some(room) = map.get_mut(room_id) {
        let before = room.peers.len();
        room.peers.retain(|p| p.id != id);
        if room.peers.len() != before {
            announce(room);
        }
        if room.peers.is_empty() && room_id.starts_with(DM_PREFIX) {
            emptied_call = true;
        }
    }
    if emptied_call {
        map.remove(room_id);
        lock_calls().remove(room_id);
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

// ── Matrix calls ─────────────────────────────────────────────────────────────

struct Call {
    caller_email: String,
    caller_name: String,
    /// Normalized emails of everyone in the Matrix room (including the caller).
    members: Vec<String>,
    started: Instant,
    declined: HashSet<String>,
}

fn lock_calls() -> std::sync::MutexGuard<'static, HashMap<String, Call>> {
    static CALLS: OnceLock<Mutex<HashMap<String, Call>>> = OnceLock::new();
    CALLS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

struct Ticket {
    email: String,
    channel: String,
    expires: Instant,
}

fn lock_tickets() -> std::sync::MutexGuard<'static, HashMap<String, Ticket>> {
    static TICKETS: OnceLock<Mutex<HashMap<String, Ticket>>> = OnceLock::new();
    TICKETS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// One-time permission for `email` to open the socket for a `dm:` channel.
fn issue_ticket(email: &str, channel: &str) -> String {
    let id = mitch_lib::crypto::random_bytes_hex(24);
    let mut tickets = lock_tickets();
    let now = Instant::now();
    tickets.retain(|_, t| t.expires > now);
    tickets.insert(
        id.clone(),
        Ticket {
            email: email.to_string(),
            channel: channel.to_string(),
            expires: now + TICKET_TTL,
        },
    );
    id
}

/// Consumes `ticket`; true only if it was issued to `email` for `channel`
/// and hasn't expired. Single use.
fn redeem_ticket(ticket: &str, email: &str, channel: &str) -> bool {
    match lock_tickets().remove(ticket) {
        Some(t) => t.email == email && t.channel == channel && t.expires > Instant::now(),
        None => false,
    }
}

/// A Matrix room id as the client sends it: `!opaque:server`.
fn valid_room_id(id: &str) -> bool {
    id.len() <= 255
        && id.starts_with('!')
        && id.contains(':')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "!:._-=/+".contains(c))
}

struct RoomMember {
    email: String,
}

/// Proves the signed-in account (`email`) is a joined member of `room_id`,
/// using the caller's own Matrix token, and returns the room's members that
/// have site accounts. The token must belong to the same account as the site
/// session, so one user can't pass off another's room.
async fn verified_room_members(
    state: &Arc<AppState>,
    email: &str,
    room_id: &str,
    mx_token: &str,
) -> Result<Vec<RoomMember>, (u16, &'static str)> {
    use crate::routes::matrix::{call_conduit, email_for_user_id};
    let mut headers = HeaderMap::new();
    let bearer = axum::http::HeaderValue::from_str(&format!("Bearer {mx_token}"))
        .map_err(|_| (401, "Chat session invalid"))?;
    headers.insert("Authorization", bearer);

    let who = call_conduit(
        "/_matrix/client/v3/account/whoami",
        Method::GET,
        Some(headers.clone()),
        None,
    )
    .await
    .map_err(|_| (502, "Chat server unreachable"))?;
    if !who.0.is_success() {
        return Err((401, "Chat session invalid"));
    }
    let who: Value = serde_json::from_slice(&who.2).unwrap_or(json!({}));
    let my_user = who.get("user_id").and_then(Value::as_str).unwrap_or("");
    let owner = email_for_user_id(state, my_user).map(|e| auth::normalize_email(&e));
    if owner.as_deref() != Some(email) {
        return Err((403, "Chat session doesn't match your account"));
    }

    let encoded: String = form_urlencoded::byte_serialize(room_id.as_bytes()).collect();
    let res = call_conduit(
        &format!("/_matrix/client/v3/rooms/{encoded}/joined_members"),
        Method::GET,
        Some(headers),
        None,
    )
    .await
    .map_err(|_| (502, "Chat server unreachable"))?;
    if !res.0.is_success() {
        return Err((403, "You're not in that room"));
    }
    let body: Value = serde_json::from_slice(&res.2).unwrap_or(json!({}));
    let joined = body
        .get("joined")
        .and_then(Value::as_object)
        .ok_or((403, "You're not in that room"))?;
    if joined.len() > MAX_CALL_ROOM_MEMBERS {
        return Err((400, "Calls work in direct messages and small rooms"));
    }
    if !joined.contains_key(my_user) {
        return Err((403, "You're not in that room"));
    }
    Ok(joined
        .keys()
        .filter_map(|u| email_for_user_id(state, u))
        .map(|e| RoomMember {
            email: auth::normalize_email(&e),
        })
        .collect())
}

fn display_name_for(state: &AppState, email: &str) -> String {
    let prof = mitch_lib::profiles::profile_get(&state.store, email);
    prof.get("displayName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let id = crate::routes::matrix::resolve_matrix_user_id_for_email(state, email);
            id.trim_start_matches('@')
                .split(':')
                .next()
                .unwrap_or("user")
                .to_string()
        })
}

fn body_json(body: &[u8]) -> Value {
    serde_json::from_slice(body).unwrap_or(json!({}))
}

/// `POST /api/voice/call`: start or answer a call in a Matrix room.
async fn start_or_answer_call(state: &Arc<AppState>, email: &str, body: &[u8]) -> Response {
    let body = body_json(body);
    let room_id = body.get("room_id").and_then(Value::as_str).unwrap_or("");
    let token = body.get("mx_token").and_then(Value::as_str).unwrap_or("");
    if !valid_room_id(room_id) || token.is_empty() || token.len() > 512 {
        return json_resp(400, json!({ "error": "Open a chat room first" }));
    }
    let members = match verified_room_members(state, email, room_id, token).await {
        Ok(m) => m,
        Err((code, msg)) => return json_resp(code, json!({ "error": msg })),
    };
    let channel = format!("{DM_PREFIX}{room_id}");
    let live = lock_rooms()
        .get(&channel)
        .map(|r| !r.peers.is_empty())
        .unwrap_or(false);
    {
        let mut calls = lock_calls();
        if !live || !calls.contains_key(&channel) {
            calls.insert(
                channel.clone(),
                Call {
                    caller_email: email.to_string(),
                    caller_name: display_name_for(state, email),
                    members: members.into_iter().map(|m| m.email).collect(),
                    started: Instant::now(),
                    declined: HashSet::new(),
                },
            );
        }
    }
    let ticket = issue_ticket(email, &channel);
    json_resp(200, json!({ "channel": channel, "ticket": ticket }))
}

/// Calls ringing for `email`: in a room they belong to, started by someone
/// else, still rung recently, with someone actually on the line, and neither
/// declined nor already joined by them.
fn incoming_for(email: &str) -> Vec<Value> {
    let rooms = lock_rooms();
    let calls = lock_calls();
    calls
        .iter()
        .filter(|(channel, c)| {
            c.caller_email != email
                && c.members.iter().any(|m| m == email)
                && !c.declined.contains(email)
                && c.started.elapsed() < Duration::from_secs(RING_SECS)
                && rooms.get(*channel).is_some_and(|r| {
                    !r.peers.is_empty() && !r.peers.iter().any(|p| p.email == email)
                })
        })
        .map(|(channel, c)| {
            json!({
                "room_id": channel.trim_start_matches(DM_PREFIX),
                "from": c.caller_name,
            })
        })
        .collect()
}

fn decline_call(email: &str, room_id: &str) {
    if let Some(c) = lock_calls().get_mut(&format!("{DM_PREFIX}{room_id}")) {
        if c.members.iter().any(|m| m == email) {
            c.declined.insert(email.to_string());
        }
    }
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

/// Plain HTTP routes.
pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Option<Response> {
    let is_get = *method == Method::GET;
    let is_post = *method == Method::POST;
    let known = (is_get && matches!(path, "/api/voice/channels" | "/api/voice/incoming"))
        || (is_post && matches!(path, "/api/voice/call" | "/api/voice/decline"));
    if !known {
        return None;
    }
    let client_ip = crate::handler::get_real_ip(headers, None);
    if let Some((code, msg)) = state.rate_limit_check(&client_ip, "anon", path) {
        return Some(json_resp(code, json!({ "error": msg })));
    }
    let email = match signed_in_email(state, headers) {
        Ok(e) => e,
        Err(resp) => return Some(resp),
    };
    Some(match path {
        "/api/voice/channels" => {
            let channels: Vec<Value> = CHANNELS
                .iter()
                .map(|(id, label)| json!({ "id": id, "name": label, "participants": names_in(id) }))
                .collect();
            json_resp(200, json!({ "channels": channels }))
        }
        "/api/voice/incoming" => json_resp(200, json!({ "calls": incoming_for(&email) })),
        "/api/voice/decline" => {
            let room = body_json(body);
            decline_call(
                &email,
                room.get("room_id").and_then(Value::as_str).unwrap_or(""),
            );
            json_resp(200, json!({ "ok": true }))
        }
        _ => start_or_answer_call(state, &email, body).await,
    })
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
    let query: HashMap<String, String> =
        form_urlencoded::parse(search.trim_start_matches('?').as_bytes())
            .into_owned()
            .collect();
    let channel = query.get("channel").cloned().unwrap_or_default();
    let room_id = if CHANNELS.iter().any(|(r, _)| *r == channel) {
        channel
    } else if channel.starts_with(DM_PREFIX)
        && valid_room_id(&channel[DM_PREFIX.len()..])
        && redeem_ticket(
            query.get("ticket").map(String::as_str).unwrap_or(""),
            &email,
            &channel,
        )
    {
        channel
    } else {
        return Some(json_resp(400, json!({ "error": "Unknown channel" })));
    };
    let name = display_name_for(state, &email);
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
    room_id: String,
    email: String,
    name: String,
) {
    let room_id = room_id.as_str();
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

    #[test]
    fn room_ids_are_validated() {
        assert!(valid_room_id("!abc123:mitch.pro"));
        assert!(!valid_room_id("#general:mitch.pro"));
        assert!(!valid_room_id("!no-server"));
        assert!(!valid_room_id("!a:b c"));
        assert!(!valid_room_id(&format!("!{}:x", "a".repeat(300))));
    }

    #[test]
    fn tickets_are_single_use_and_bound_to_account_and_channel() {
        let t = issue_ticket("a@x", "dm:!r:mitch.pro");
        assert!(
            !redeem_ticket(&t, "b@x", "dm:!r:mitch.pro"),
            "other account"
        );
        // A failed attempt still burns the ticket.
        assert!(!redeem_ticket(&t, "a@x", "dm:!r:mitch.pro"));
        let t = issue_ticket("a@x", "dm:!r:mitch.pro");
        assert!(
            !redeem_ticket(&t, "a@x", "dm:!other:mitch.pro"),
            "other room"
        );
        let t = issue_ticket("a@x", "dm:!r:mitch.pro");
        assert!(redeem_ticket(&t, "a@x", "dm:!r:mitch.pro"));
        assert!(!redeem_ticket(&t, "a@x", "dm:!r:mitch.pro"), "reuse");
        assert!(!redeem_ticket("nonsense", "a@x", "dm:!r:mitch.pro"));
    }

    #[test]
    fn matrix_call_rings_other_members_until_answered_or_declined() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let ch = "dm:!ringtest:mitch.pro";
        lock_calls().insert(
            ch.to_string(),
            Call {
                caller_email: "a@x".into(),
                caller_name: "Ann".into(),
                members: vec!["a@x".into(), "b@x".into()],
                started: Instant::now(),
                declined: HashSet::new(),
            },
        );
        // Nobody on the line yet: not ringing.
        assert!(incoming_for("b@x").is_empty());
        let a = join(ch, "a@x", "Ann").unwrap();
        let rings = incoming_for("b@x");
        assert_eq!(rings.len(), 1);
        assert_eq!(rings[0]["room_id"], "!ringtest:mitch.pro");
        assert_eq!(rings[0]["from"], "Ann");
        assert!(incoming_for("a@x").is_empty(), "caller isn't rung");
        assert!(incoming_for("c@x").is_empty(), "non-members aren't rung");
        decline_call("b@x", "!ringtest:mitch.pro");
        assert!(incoming_for("b@x").is_empty());
        // Hanging up empties the room and forgets the call.
        leave(ch, a.id);
        assert!(!lock_rooms().contains_key(ch));
        assert!(!lock_calls().contains_key(ch));
    }

    #[test]
    fn answering_removes_the_ring_for_that_person() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let ch = "dm:!answertest:mitch.pro";
        lock_calls().insert(
            ch.to_string(),
            Call {
                caller_email: "a@x".into(),
                caller_name: "Ann".into(),
                members: vec!["a@x".into(), "b@x".into()],
                started: Instant::now(),
                declined: HashSet::new(),
            },
        );
        let a = join(ch, "a@x", "Ann").unwrap();
        assert_eq!(incoming_for("b@x").len(), 1);
        let b = join(ch, "b@x", "Bob").unwrap();
        assert!(incoming_for("b@x").is_empty());
        leave(ch, a.id);
        assert!(lock_rooms().contains_key(ch), "still one person in it");
        leave(ch, b.id);
        assert!(!lock_rooms().contains_key(ch));
    }
}
