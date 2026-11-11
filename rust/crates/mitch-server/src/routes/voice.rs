//! Voice for Matrix rooms: audio relayed over a same-origin WebSocket. See
//! `docs/voice-channels.md`.
//!
//! The chat client (Cinny) starts calls by loading an embedded call page
//! (`/matrix/public/element-call/`). That page asks this module to join the
//! call for the Matrix room it was opened in; there are no fixed channels. A
//! call is a `room:<matrix room id>` channel that is created the first time
//! someone joins and removed when the last person leaves, so every voice room
//! and DM call in the chat client gets its own backend room automatically.
//!
//! - `POST /api/voice/call {room_id, mx_token}` — checks the caller really is
//!   in that Matrix room (using their own Matrix token) and returns a
//!   one-time `ticket` for the socket.
//! - `GET /api/voice/ws?channel=room:<id>&ticket=…` (WebSocket) — the call.
//!
//! Wire protocol. Audio is 16 kHz mono signed 16-bit little-endian PCM.
//! Binary messages start with a type byte.
//! - client → server, `[1][pcm]`: one audio frame (≤ [`MAX_FRAME_BYTES`]).
//! - client → server, `[2][codec][flags][data]`: one video frame (camera
//!   must be on, ≤ [`MAX_VIDEO_BYTES`]). `flags` bit 0 = keyframe. Codec ids
//!   are opaque to the server (the browsers agree on them).
//! - client → server, text: `{"muted":bool}`, `{"video":bool}` (ask to turn
//!   the camera on/off), `{"keyframe":true}` (a decoder needs a new keyframe).
//! - server → client, binary: `[1][u32 BE sender id][pcm]` or
//!   `[2][u32 BE sender id][codec][flags][data]`.
//! - server → client, text: `{"type":"hello","id":N}` once, then
//!   `{"type":"roster","people":[{"id","name","muted","video"}]}` on every
//!   change; `{"type":"video","ok":bool}` answering a `video` request (refused
//!   when [`MAX_VIDEO_SENDERS`] cameras are already on); and
//!   `{"type":"keyframe"}` asking a camera to send a keyframe soon.
//!
//! Video rides its own broadcast channel, polled after audio, so a slow
//! listener drops video frames (and asks for a keyframe) instead of ever
//! delaying or losing audio.
//!
//! Membership lives only in memory and is never written anywhere. Joining
//! needs a signed-in, non-banned account, a same-origin request (so other
//! sites can't open the socket with a visitor's cookies), a single-use ticket
//! and a free seat; frames are size- and rate-limited per connection.

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
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, Notify};

/// Backend rooms are keyed `room:<matrix room id>`.
const ROOM_PREFIX: &str = "room:";
/// A ticket must be used within this long of being issued.
const TICKET_TTL: Duration = Duration::from_secs(60);

/// People per call. The relay sends every frame to every listener, so
/// bandwidth grows with the square of this.
pub const MAX_PEERS: usize = 16;
/// 20 ms of 16 kHz mono 16-bit is 640 bytes; allow some slack.
pub const MAX_FRAME_BYTES: usize = 2048;
/// Cameras on at once in one call. Every video frame is sent to every other
/// listener, so this bounds the room's bandwidth.
pub const MAX_VIDEO_SENDERS: usize = 6;
/// Largest single video frame (a keyframe at 320x240 is well under this).
pub const MAX_VIDEO_BYTES: usize = 128 * 1024;
/// Per-sender video budget: a 350 kbit/s stream is ~45 KB/s.
const VIDEO_BYTES_PER_SEC: f64 = 250_000.0;
const VIDEO_BYTES_BURST: f64 = 262_144.0;
/// Keyframe requests (new viewer, lagging viewer) are coalesced to this.
const KEYFRAME_COOLDOWN: Duration = Duration::from_millis(500);
/// Sustained frames per second a client may send (50 is real-time).
const FRAMES_PER_SEC: f64 = 60.0;
const FRAME_BURST: f64 = 40.0;

enum Event {
    /// Audio from the peer with this id (not delivered back to the sender).
    Audio { from: u32, data: Vec<u8> },
    /// Serialized roster message; the room's membership changed.
    Roster(String),
    /// Cameras should send a keyframe soon (someone new is watching).
    Keyframe,
}

/// A video frame as `[codec][flags][data]` from the peer with id `from`.
struct VideoEvent {
    from: u32,
    data: Vec<u8>,
}

struct Peer {
    id: u32,
    email: String,
    name: String,
    muted: bool,
    /// Camera is on (the server allowed it; see [`MAX_VIDEO_SENDERS`]).
    video: bool,
    /// Wakes the connection's task so it can close itself.
    kick: Arc<Notify>,
}

struct Room {
    /// Audio, roster and keyframe requests.
    tx: broadcast::Sender<Arc<Event>>,
    /// Video frames; small so a slow listener lags (drops) quickly.
    vtx: broadcast::Sender<Arc<VideoEvent>>,
    peers: Vec<Peer>,
    last_keyframe_request: Instant,
}

impl Room {
    fn new() -> Self {
        Self {
            tx: broadcast::channel(256).0,
            vtx: broadcast::channel(16).0,
            peers: Vec::new(),
            last_keyframe_request: Instant::now() - KEYFRAME_COOLDOWN,
        }
    }
}

fn rooms() -> &'static Mutex<HashMap<String, Room>> {
    static ROOMS: OnceLock<Mutex<HashMap<String, Room>>> = OnceLock::new();
    ROOMS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_rooms() -> std::sync::MutexGuard<'static, HashMap<String, Room>> {
    rooms().lock().unwrap_or_else(|e| e.into_inner())
}

fn roster_message(room: &Room) -> String {
    json!({
        "type": "roster",
        "people": room.peers.iter()
            .map(|p| json!({ "id": p.id, "name": p.name, "muted": p.muted, "video": p.video }))
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
    vrx: broadcast::Receiver<Arc<VideoEvent>>,
    kick: Arc<Notify>,
}

/// Asks every camera for a keyframe, at most once per [`KEYFRAME_COOLDOWN`].
fn request_keyframes_locked(room: &mut Room) {
    if room.last_keyframe_request.elapsed() >= KEYFRAME_COOLDOWN {
        room.last_keyframe_request = Instant::now();
        let _ = room.tx.send(Arc::new(Event::Keyframe));
    }
}

fn request_keyframes(room_id: &str) {
    if let Some(room) = lock_rooms().get_mut(room_id) {
        request_keyframes_locked(room);
    }
}

/// Adds `email` to `room_id`, creating the backend room if it doesn't exist
/// yet. An account holds one seat per room: a second join replaces (and
/// kicks) the first, so a stale tab can't hold the seat. Callers must have
/// passed [`redeem_ticket`] first.
fn join(room_id: &str, email: &str, name: &str) -> Result<Joined, &'static str> {
    static NEXT_ID: AtomicU32 = AtomicU32::new(1);
    if !room_id.starts_with(ROOM_PREFIX) {
        return Err("Unknown channel");
    }
    let mut map = lock_rooms();
    let room = map.entry(room_id.to_string()).or_insert_with(Room::new);
    let (old, keep): (Vec<Peer>, Vec<Peer>) = std::mem::take(&mut room.peers)
        .into_iter()
        .partition(|p| p.email == email);
    room.peers = keep;
    for p in &old {
        p.kick.notify_one();
    }
    if room.peers.len() >= MAX_PEERS {
        announce(room);
        return Err("This call is full");
    }
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let kick = Arc::new(Notify::new());
    room.peers.push(Peer {
        id,
        email: email.to_string(),
        name: name.to_string(),
        muted: false,
        video: false,
        kick: kick.clone(),
    });
    let rx = room.tx.subscribe();
    let vrx = room.vtx.subscribe();
    announce(room);
    // Someone new is watching: cameras that are already on need a keyframe.
    request_keyframes_locked(room);
    Ok(Joined { id, rx, vrx, kick })
}

/// Removes a peer; the room itself goes away with its last one.
fn leave(room_id: &str, id: u32) {
    let mut map = lock_rooms();
    let mut empty = false;
    if let Some(room) = map.get_mut(room_id) {
        let before = room.peers.len();
        room.peers.retain(|p| p.id != id);
        if room.peers.len() != before {
            announce(room);
        }
        empty = room.peers.is_empty();
    }
    if empty {
        map.remove(room_id);
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

/// Turns a camera on or off. Returns false (and changes nothing) if turning
/// it on would exceed [`MAX_VIDEO_SENDERS`].
fn set_video(room_id: &str, id: u32, on: bool) -> bool {
    let mut map = lock_rooms();
    let Some(room) = map.get_mut(room_id) else {
        return false;
    };
    let cameras = room.peers.iter().filter(|p| p.video).count();
    let Some(p) = room.peers.iter_mut().find(|p| p.id == id) else {
        return false;
    };
    if on && !p.video && cameras >= MAX_VIDEO_SENDERS {
        return false;
    }
    if p.video != on {
        p.video = on;
        announce(room);
    }
    true
}

fn relay_video(room_id: &str, from: u32, data: Vec<u8>) {
    let map = lock_rooms();
    if let Some(room) = map.get(room_id) {
        let _ = room.vtx.send(Arc::new(VideoEvent { from, data }));
    }
}

fn relay_audio(room_id: &str, from: u32, data: Vec<u8>) {
    let map = lock_rooms();
    if let Some(room) = map.get(room_id) {
        let _ = room.tx.send(Arc::new(Event::Audio { from, data }));
    }
}

// ── Matrix calls ─────────────────────────────────────────────────────────────

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

/// A Matrix room id as the client sends it. Older rooms look like
/// `!opaque:server`; newer room versions have no server part at all
/// (`!<url-safe base64 hash>`), so only the leading `!`, a sane length and a
/// safe character set are required.
fn valid_room_id(id: &str) -> bool {
    id.len() > 1
        && id.len() <= 255
        && id.starts_with('!')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "!:._-=/+".contains(c))
}

/// Proves the signed-in account (`email`) is a joined member of `room_id`,
/// using the caller's own Matrix token. The token must belong to the same
/// account as the site session, so one user can't pass off another's room.
async fn verify_room_member(
    state: &Arc<AppState>,
    email: &str,
    room_id: &str,
    mx_token: &str,
) -> Result<(), (u16, &'static str)> {
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
    let is_member = body
        .get("joined")
        .and_then(Value::as_object)
        .is_some_and(|joined| joined.contains_key(my_user));
    if is_member {
        Ok(())
    } else {
        Err((403, "You're not in that room"))
    }
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

/// `POST /api/voice/call`: permission to join the call in a Matrix room.
async fn call_ticket(state: &Arc<AppState>, email: &str, body: &[u8]) -> Response {
    let body = body_json(body);
    let room_id = body.get("room_id").and_then(Value::as_str).unwrap_or("");
    let token = body.get("mx_token").and_then(Value::as_str).unwrap_or("");
    if !valid_room_id(room_id) || token.is_empty() || token.len() > 512 {
        return json_resp(
            400,
            json!({ "error": "Could not tell which chat room this call is for" }),
        );
    }
    if let Err((code, msg)) = verify_room_member(state, email, room_id, token).await {
        return json_resp(code, json!({ "error": msg }));
    }
    let channel = format!("{ROOM_PREFIX}{room_id}");
    let ticket = issue_ticket(email, &channel);
    json_resp(200, json!({ "channel": channel, "ticket": ticket }))
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

/// Token bucket in bytes, for video.
struct ByteLimiter {
    tokens: f64,
    last: Instant,
}

impl ByteLimiter {
    fn new() -> Self {
        Self {
            tokens: VIDEO_BYTES_BURST,
            last: Instant::now(),
        }
    }
    fn allow(&mut self, bytes: usize) -> bool {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + dt * VIDEO_BYTES_PER_SEC).min(VIDEO_BYTES_BURST);
        if self.tokens >= bytes as f64 {
            self.tokens -= bytes as f64;
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

/// `POST /api/voice/call`.
pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Option<Response> {
    if *method != Method::POST || path != "/api/voice/call" {
        return None;
    }
    let client_ip = crate::handler::get_real_ip(headers, None);
    if let Some((code, msg)) = state.rate_limit_check(&client_ip, "anon", path) {
        return Some(json_resp(code, json!({ "error": msg })));
    }
    Some(match signed_in_email(state, headers) {
        Ok(email) => call_ticket(state, &email, body).await,
        Err(resp) => resp,
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
    let room_id = if channel.starts_with(ROOM_PREFIX)
        && valid_room_id(&channel[ROOM_PREFIX.len()..])
        && redeem_ticket(
            query.get("ticket").map(String::as_str).unwrap_or(""),
            &email,
            &channel,
        ) {
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
    let Joined {
        id,
        mut rx,
        mut vrx,
        kick,
    } = match join(room_id, &email, &name) {
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
    let mut video_limiter = ByteLimiter::new();
    // This peer's camera is on (the server said yes to its request).
    let mut my_video = false;
    let mut ban_check = tokio::time::interval(std::time::Duration::from_secs(2));
    loop {
        // `biased`: audio/roster before video, video before the socket, so a
        // video backlog can't hold up audio.
        tokio::select! {
            biased;
            _ = kick.notified() => {
                let _ = socket.send(close_frame(4000, "Joined from another tab")).await;
                break;
            }
            ev = rx.recv() => match ev {
                Ok(ev) => {
                    let msg = match &*ev {
                        Event::Audio { from, .. } if *from == id => continue,
                        Event::Audio { from, data } => {
                            let mut out = Vec::with_capacity(5 + data.len());
                            out.push(1);
                            out.extend_from_slice(&from.to_be_bytes());
                            out.extend_from_slice(data);
                            Message::Binary(out.into())
                        }
                        Event::Roster(json) => Message::Text(json.clone().into()),
                        Event::Keyframe if my_video => {
                            Message::Text(json!({ "type": "keyframe" }).to_string().into())
                        }
                        Event::Keyframe => continue,
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
            ev = vrx.recv() => match ev {
                Ok(ev) if ev.from == id => {}
                Ok(ev) => {
                    let mut out = Vec::with_capacity(5 + ev.data.len());
                    out.push(2);
                    out.extend_from_slice(&ev.from.to_be_bytes());
                    out.extend_from_slice(&ev.data);
                    if socket.send(Message::Binary(out.into())).await.is_err() {
                        break;
                    }
                }
                // Dropped frames break delta decoding until the next keyframe.
                Err(broadcast::error::RecvError::Lagged(_)) => request_keyframes(room_id),
                Err(broadcast::error::RecvError::Closed) => break,
            },
            _ = ban_check.tick() => {
                if auth::banned_info_for_email(&state.store, &email).is_some() {
                    let _ = socket.send(close_frame(1008, "Account banned")).await;
                    break;
                }
            }
            frame = socket.recv() => match frame {
                Some(Ok(Message::Binary(data))) => match data.first() {
                    Some(1) => {
                        let pcm = &data[1..];
                        if !pcm.is_empty() && pcm.len() <= MAX_FRAME_BYTES && limiter.allow() {
                            relay_audio(room_id, id, pcm.to_vec());
                        }
                    }
                    // [2][codec][flags][data]
                    Some(2) => {
                        let rest = &data[1..];
                        if my_video
                            && rest.len() > 2
                            && rest.len() <= MAX_VIDEO_BYTES
                            && video_limiter.allow(rest.len())
                        {
                            relay_video(room_id, id, rest.to_vec());
                        }
                    }
                    _ => {}
                },
                Some(Ok(Message::Text(text))) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&text) {
                        if let Some(m) = v.get("muted").and_then(Value::as_bool) {
                            set_muted(room_id, id, m);
                        }
                        if let Some(want) = v.get("video").and_then(Value::as_bool) {
                            let ok = set_video(room_id, id, want);
                            my_video = ok && want;
                            let reply = json!({ "type": "video", "ok": ok, "on": my_video });
                            if socket.send(Message::Text(reply.to_string().into())).await.is_err() {
                                break;
                            }
                            if my_video {
                                request_keyframes(room_id);
                            }
                        }
                        if v.get("keyframe").and_then(Value::as_bool) == Some(true) {
                            request_keyframes(room_id);
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

    // The registry is process-global, so tests use their own room keys and
    // run one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn size(room: &str) -> usize {
        lock_rooms().get(room).map_or(0, |r| r.peers.len())
    }

    #[test]
    fn only_matrix_room_channels_can_be_joined() {
        let _g = serial();
        assert!(join("voice-lounge", "a@x", "A").is_err());
        assert!(join("", "a@x", "A").is_err());
    }

    #[test]
    fn rooms_are_created_on_join_and_removed_with_the_last_person() {
        let _g = serial();
        let ch = "room:!lifecycle:mitch.pro";
        assert_eq!(size(ch), 0);
        let a = join(ch, "a@x", "Ann").unwrap();
        let b = join(ch, "b@x", "Bob").unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(size(ch), 2);
        leave(ch, a.id);
        assert_eq!(size(ch), 1);
        assert!(lock_rooms().contains_key(ch));
        leave(ch, b.id);
        assert!(!lock_rooms().contains_key(ch));
    }

    #[test]
    fn second_join_replaces_the_first_seat() {
        let _g = serial();
        let ch = "room:!replace:mitch.pro";
        let first = join(ch, "a@x", "Ann").unwrap();
        let second = join(ch, "a@x", "Ann").unwrap();
        assert_eq!(size(ch), 1);
        assert_ne!(first.id, second.id);
        leave(ch, second.id);
    }

    #[test]
    fn a_full_call_rejects_new_people_but_not_returning_ones() {
        let _g = serial();
        let ch = "room:!full:mitch.pro";
        let mut ids = Vec::new();
        for i in 0..MAX_PEERS {
            ids.push(join(ch, &format!("u{i}@x"), "U").unwrap().id);
        }
        assert!(join(ch, "extra@x", "X").is_err());
        let again = join(ch, "u0@x", "U").unwrap();
        ids.push(again.id);
        for id in ids {
            leave(ch, id);
        }
        assert!(!lock_rooms().contains_key(ch));
    }

    #[test]
    fn audio_reaches_others_with_sender_id() {
        let _g = serial();
        let ch = "room:!audio:mitch.pro";
        let a = join(ch, "a@x", "Ann").unwrap();
        let mut b = join(ch, "b@x", "Bob").unwrap();
        relay_audio(ch, a.id, vec![1, 2, 3]);
        let mut saw = false;
        while let Ok(ev) = b.rx.try_recv() {
            if let Event::Audio { from, data } = &*ev {
                assert_eq!(*from, a.id);
                assert_eq!(data, &vec![1, 2, 3]);
                saw = true;
            }
        }
        assert!(saw);
        leave(ch, a.id);
        leave(ch, b.id);
    }

    #[test]
    fn limiter_caps_a_flood_but_allows_realtime() {
        let mut l = Limiter::new();
        let burst = (0..1000).filter(|_| l.allow()).count();
        assert!(burst <= FRAME_BURST as usize + 2, "{burst}");
        let mut l = Limiter::new();
        l.tokens = 1.0;
        std::thread::sleep(std::time::Duration::from_millis(40));
        assert!(l.allow());
    }

    #[test]
    fn room_ids_are_validated() {
        assert!(valid_room_id("!abc123:mitch.pro"));
        // Newer room versions: no server part, url-safe base64 with - and _.
        assert!(valid_room_id(
            "!1oIV13AJcrH-61ZyaersatED1FD3r0Q8YPmdlHteZfA"
        ));
        assert!(valid_room_id(
            "!6U5_lK6nO2MjyQGLRavglmIk2VCn3LQaDNbsyY4nQ-A"
        ));
        assert!(!valid_room_id("!"));
        assert!(!valid_room_id("#general:mitch.pro"));
        assert!(!valid_room_id("abc:mitch.pro"));
        assert!(!valid_room_id("!a:b c"));
        assert!(!valid_room_id("!a/../b?x=1"));
        assert!(!valid_room_id(&format!("!{}:x", "a".repeat(300))));
    }

    #[test]
    fn tickets_are_single_use_and_bound_to_account_and_channel() {
        let ch = "room:!r:mitch.pro";
        let t = issue_ticket("a@x", ch);
        assert!(!redeem_ticket(&t, "b@x", ch), "other account");
        // A failed attempt still burns the ticket.
        assert!(!redeem_ticket(&t, "a@x", ch));
        let t = issue_ticket("a@x", ch);
        assert!(
            !redeem_ticket(&t, "a@x", "room:!other:mitch.pro"),
            "other room"
        );
        let t = issue_ticket("a@x", ch);
        assert!(redeem_ticket(&t, "a@x", ch));
        assert!(!redeem_ticket(&t, "a@x", ch), "reuse");
        assert!(!redeem_ticket("nonsense", "a@x", ch));
    }

    #[test]
    fn cameras_are_capped_and_the_roster_shows_them() {
        let _g = serial();
        let ch = "room:!cameras:mitch.pro";
        let ids: Vec<u32> = (0..MAX_VIDEO_SENDERS + 1)
            .map(|i| join(ch, &format!("c{i}@x"), "C").unwrap().id)
            .collect();
        for id in &ids[..MAX_VIDEO_SENDERS] {
            assert!(set_video(ch, *id, true));
        }
        assert!(!set_video(ch, ids[MAX_VIDEO_SENDERS], true), "over the cap");
        // Turning one off frees a slot; turning on twice doesn't double count.
        assert!(set_video(ch, ids[0], true));
        assert!(set_video(ch, ids[0], false));
        assert!(set_video(ch, ids[MAX_VIDEO_SENDERS], true));
        let roster = {
            let map = lock_rooms();
            roster_message(map.get(ch).unwrap())
        };
        let v: Value = serde_json::from_str(&roster).unwrap();
        let on = v["people"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["video"] == true)
            .count();
        assert_eq!(on, MAX_VIDEO_SENDERS);
        for id in ids {
            leave(ch, id);
        }
    }

    #[test]
    fn video_reaches_others_on_its_own_channel() {
        let _g = serial();
        let ch = "room:!video:mitch.pro";
        let a = join(ch, "a@x", "Ann").unwrap();
        let mut b = join(ch, "b@x", "Bob").unwrap();
        relay_video(ch, a.id, vec![1, 1, 9, 9]);
        let got = b.vrx.try_recv().expect("video frame");
        assert_eq!(got.from, a.id);
        assert_eq!(got.data, vec![1, 1, 9, 9]);
        // Audio is untouched by video traffic.
        assert!(!matches!(
            b.rx.try_recv().ok().as_deref(),
            Some(Event::Audio { .. })
        ));
        leave(ch, a.id);
        leave(ch, b.id);
    }

    #[test]
    fn keyframe_requests_are_coalesced() {
        let _g = serial();
        let ch = "room:!keyframes:mitch.pro";
        let _a = join(ch, "a@x", "Ann").unwrap();
        let mut b = join(ch, "b@x", "Bob").unwrap();
        let count = |rx: &mut broadcast::Receiver<Arc<Event>>| {
            let mut n = 0;
            while let Ok(ev) = rx.try_recv() {
                if matches!(&*ev, Event::Keyframe) {
                    n += 1;
                }
            }
            n
        };
        // Joining asked once; a burst of further requests is ignored for a moment.
        let _ = count(&mut b.rx);
        request_keyframes(ch);
        request_keyframes(ch);
        request_keyframes(ch);
        assert!(count(&mut b.rx) <= 1);
        std::thread::sleep(KEYFRAME_COOLDOWN + Duration::from_millis(50));
        request_keyframes(ch);
        assert_eq!(count(&mut b.rx), 1);
        let ids: Vec<u32> = lock_rooms()
            .get(ch)
            .unwrap()
            .peers
            .iter()
            .map(|p| p.id)
            .collect();
        for id in ids {
            leave(ch, id);
        }
    }

    #[test]
    fn video_byte_budget_allows_a_normal_stream_but_not_a_flood() {
        let mut l = ByteLimiter::new();
        // A burst of keyframe-sized frames fits once, then runs dry.
        let sent = (0..100).filter(|_| l.allow(30_000)).count();
        assert!((6..=10).contains(&sent), "{sent}");
        let mut l = ByteLimiter::new();
        assert!(!l.allow(MAX_VIDEO_BYTES * 3));
    }
}
