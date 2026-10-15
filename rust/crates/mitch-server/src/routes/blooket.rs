//! `/api/blooket-bot/*` — the Blooket Bot premium queue (plan Step 13 batch 2).
//!
//! Ports (server.js):
//! - the queue machinery (1263-1375): `blooketQueue`/`blooketActive`/
//!   `blooketPinLocks`, `BLOOKET_MAX_ACTIVE = 5`, `processBlooketQueue` and
//!   `startBlooketBotSession` (the microservice WS client).
//! - the `/api/blooket-bot/ws` upgrade (11797-11832) and the five HTTP
//!   endpoints (11834-11988) — status / stop / lock / report-failure /
//!   failure-reports.
//! - the open (24913-24918), message (24970-24997) and close (25198-25205)
//!   legs of the client socket.
//!
//! The JS couples session state to the socket objects (`ws.msWs`,
//! `readyState`). Here every client socket is a task with a control channel
//! (`BlooketCtl`); queue/active records hold the `Sender` side plus an
//! `open` flag that mirrors the JS `readyState === 1` checks. The
//! microservice connection is a spawned task driven by `MsCommand`s, with
//! its close leg reproducing `msWs.onclose`/`onerror`.

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use mitch_lib::jsval;
use mitch_lib::school::now_millis;
use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::state::AppState;

/// `BLOOKET_MAX_ACTIVE` (server.js:1266).
const BLOOKET_MAX_ACTIVE: usize = 5;

/// Queue/session parameters parsed from the upgrade query (server.js:11804-11808).
#[derive(Clone)]
pub struct BlooketParams {
    pub pin: String,
    pub name: String,
    pub auto: String,
    pub headless: bool,
}

/// A connected-but-queued client (server.js `blooketQueue` entries).
pub struct BlooketQueued {
    pub session_id: u64,
    pub email: String,
    pub params: BlooketParams,
    pub ctl: mpsc::UnboundedSender<BlooketCtl>,
    pub open: Arc<AtomicBool>,
}

/// An admitted session (server.js `blooketActive` record). `open` mirrors
/// `session.ws.readyState === 1` and doubles as the `clientWs === ws`
/// identity check — a reaped record is exactly one whose client socket left.
pub struct BlooketActive {
    pub params: BlooketParams,
    pub ctl: mpsc::UnboundedSender<BlooketCtl>,
    pub ms_tx: mpsc::UnboundedSender<MsCommand>,
    pub open: Arc<AtomicBool>,
}

/// Server → client-socket commands. `Admit` hands the socket its
/// microservice pipe once the ms connection is OPEN (the JS gates routing on
/// `ws.msWs.readyState === 1`, so client input during the connect window is
/// dropped — deferring `Admit` reproduces that).
pub enum BlooketCtl {
    Send(String),
    Admit(mpsc::UnboundedSender<MsCommand>),
    Close,
}

/// Client/task → microservice-socket commands.
pub enum MsCommand {
    Send(String),
    Close,
}

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

fn lock_mutex<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `processBlooketQueue` (server.js:1268-1305).
pub(crate) fn process_queue(state: &Arc<AppState>) {
    // 1. Reap active sessions whose client socket is gone; JS also drops
    //    sessions whose msWs is dead — our ms close leg removes those itself,
    //    so the open flag is the whole condition here.
    let ms_to_close: Vec<mpsc::UnboundedSender<MsCommand>> = {
        let mut actives = lock_mutex(&state.blooket_active);
        let mut closed = Vec::new();
        actives.retain(|email, rec| {
            if !rec.open.load(Ordering::SeqCst) {
                tracing::info!("[blooket-queue] Cleaning up inactive active session for {email}");
                closed.push(rec.ms_tx.clone());
                false
            } else {
                true
            }
        });
        closed
    };
    for tx in ms_to_close {
        let _ = tx.send(MsCommand::Close);
    }

    // 2. Drop disconnected queued sockets.
    lock_mutex(&state.blooket_queue).retain(|q| q.open.load(Ordering::SeqCst));

    // 3. Admit while below the cap.
    loop {
        let next = {
            let mut queue = lock_mutex(&state.blooket_queue);
            let actives = lock_mutex(&state.blooket_active);
            if actives.len() >= BLOOKET_MAX_ACTIVE || queue.is_empty() {
                break;
            }
            queue.remove(0)
        };
        if !next.open.load(Ordering::SeqCst) {
            continue;
        }
        {
            let actives = lock_mutex(&state.blooket_active);
            if actives.contains_key(&next.email) {
                tracing::info!(
                    "[blooket-queue] User {} already has an active bot. Skipping.",
                    next.email
                );
                let _ = next.ctl.send(BlooketCtl::Send(
                    json!({ "type": "error", "message": "You already have a bot running." })
                        .to_string(),
                ));
                let _ = next.ctl.send(BlooketCtl::Close);
                continue;
            }
        }
        // startBlooketBotSession: the active record is inserted BEFORE the
        // microservice connection completes (the JS set() runs synchronously
        // next to `new WebSocket`).
        let (ms_tx, ms_rx) = mpsc::unbounded_channel();
        lock_mutex(&state.blooket_active).insert(
            next.email.clone(),
            BlooketActive {
                params: next.params.clone(),
                ctl: next.ctl.clone(),
                ms_tx: ms_tx.clone(),
                open: Arc::clone(&next.open),
            },
        );
        tokio::spawn(run_ms_session(
            Arc::clone(state),
            next.email.clone(),
            next.params.clone(),
            next.ctl.clone(),
            ms_rx,
        ));
        // Hand the client socket its microservice pipe (the deferred routing
        // gate — see BlooketCtl::Admit).
        let _ = next.ctl.send(BlooketCtl::Admit(ms_tx));
    }

    // 4. Position broadcasts to every queued client.
    let (queue_len, actives_len, updates) = {
        let queue = lock_mutex(&state.blooket_queue);
        let actives = lock_mutex(&state.blooket_active);
        let updates: Vec<mpsc::UnboundedSender<BlooketCtl>> = queue
            .iter()
            .filter(|q| q.open.load(Ordering::SeqCst))
            .map(|q| q.ctl.clone())
            .collect();
        (queue.len(), actives.len(), updates)
    };
    for (i, ctl) in updates.iter().enumerate() {
        let _ = ctl.send(BlooketCtl::Send(
            json!({
                "type": "queue",
                "position": i + 1,
                "total": queue_len,
                "activeCount": actives_len
            })
            .to_string(),
        ));
    }
}

/// `startBlooketBotSession` (server.js:1311-1375) as a spawned task: connect
/// to the bot microservice, relay both directions, and reproduce the
/// onopen/onmessage/onclose/onerror semantics on its close leg.
async fn run_ms_session(
    state: Arc<AppState>,
    email: String,
    params: BlooketParams,
    ctl: mpsc::UnboundedSender<BlooketCtl>,
    mut ms_rx: mpsc::UnboundedReceiver<MsCommand>,
) {
    // server.js:1312-1313 — env URL, else host from DOCKER detection.
    let bot_host = std::env::var("BLOOKET_BOT_HOST").unwrap_or_else(|_| {
        if std::env::var("DOCKER_ENV").as_deref() == Ok("1")
            || std::path::Path::new("/.dockerenv").exists()
        {
            "blooket-bot".to_string()
        } else {
            "127.0.0.1".to_string()
        }
    });
    let ms_url =
        std::env::var("BLOOKET_BOT_URL").unwrap_or_else(|_| format!("ws://{bot_host}:8082"));

    tracing::info!("[blooket-bot] Connecting to microservice for {email} at {ms_url}");

    let Ok((ms_ws, _)) = tokio_tungstenite::connect_async(&ms_url).await else {
        // The JS `new WebSocket` fails asynchronously: onerror sends the
        // error frame, then onclose runs the termination leg. Both here.
        tracing::error!("[blooket-bot] Failed to connect to microservice for {email}");
        let _ = ctl.send(BlooketCtl::Send(
            json!({
                "type": "error",
                "message": "Bot connection encountered an error."
            })
            .to_string(),
        ));
        ms_close_leg(&state, &email, &ctl);
        return;
    };
    let (mut ms_sink, mut ms_stream) = ms_ws.split();

    // msWs.onopen → status + start.
    tracing::info!("[blooket-bot] Connected to microservice for {email}. Starting bot...");
    let _ = ctl.send(BlooketCtl::Send(
        json!({ "type": "status", "text": "Bot launching..." }).to_string(),
    ));
    let start = json!({
        "type": "start",
        "pin": params.pin,
        "name": params.name,
        "auto": params.auto,
        "headless": params.headless
    })
    .to_string();
    if ms_sink
        .send(tokio_tungstenite::tungstenite::Message::Text(start.into()))
        .await
        .is_err()
    {
        ms_close_leg(&state, &email, &ctl);
        return;
    }

    loop {
        tokio::select! {
            cmd = ms_rx.recv() => match cmd {
                Some(MsCommand::Send(text)) => {
                    if ms_sink
                        .send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Some(MsCommand::Close) | None => {
                    let _ = ms_sink.close().await;
                    break;
                }
            },
            frame = ms_stream.next() => match frame {
                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t))) => {
                    // msWs.onmessage → ws.send(event.data).
                    let _ = ctl.send(BlooketCtl::Send(t.to_string()));
                }
                Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(b))) => {
                    let _ = ctl.send(BlooketCtl::Send(
                        String::from_utf8_lossy(&b).to_string(),
                    ));
                }
                Some(Ok(_)) => {}
                // msWs.onclose → 'Bot terminated.' + close + delete + process.
                Some(Err(_)) | None => break,
            },
        }
    }
    ms_close_leg(&state, &email, &ctl);
}

/// `msWs.onclose` (server.js:1354-1364): notify + close the client, drop the
/// active record, and re-run the queue.
fn ms_close_leg(state: &Arc<AppState>, email: &str, ctl: &mpsc::UnboundedSender<BlooketCtl>) {
    tracing::info!("[blooket-bot] Microservice connection closed for {email}");
    let _ = ctl.send(BlooketCtl::Send(
        json!({ "type": "status", "text": "Bot terminated." }).to_string(),
    ));
    let _ = ctl.send(BlooketCtl::Close);
    lock_mutex(&state.blooket_active).shift_remove(email);
    process_queue(state);
}

// ── the client socket (upgrade at 11797; open/message/close at 24913+) ──────

/// The `/api/blooket-bot/ws` upgrade arm (server.js:11797-11832). Returns
/// `None` when the path/upgrade shape does not match so the caller falls
/// through; the JS only enters this block on `upgrade: websocket`.
pub fn handle_ws_upgrade(
    state: &Arc<AppState>,
    path: &str,
    search: &str,
    headers: &HeaderMap,
    upgrade: Option<WebSocketUpgrade>,
) -> Option<Response> {
    if path != "/api/blooket-bot/ws" {
        return None;
    }

    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cookies.get("id").unwrap_or(""))
        .to_string();
    let Some(email) = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid) else {
        return Some(crate::routes::me::json_response(
            401,
            json!({ "success": false, "error": "Authentication required" }),
        ));
    };
    if !mitch_lib::auth::is_premium_email(&state.store, &email) {
        return Some(crate::routes::me::json_response(
            403,
            json!({ "success": false, "error": "Premium required to use Blooket Bot" }),
        ));
    }
    // A websocket request that fails the handshake (`server.upgrade` false)
    // falls through to the rest of the pipeline in the JS — no 400 here.
    let on_upgrade = upgrade?;

    let params = ws_params_from_search(search);

    let active_lock_admin = lock_mutex(&state.blooket_pin_locks)
        .get(&params.pin)
        .cloned();
    if let Some(lock_admin) = active_lock_admin {
        if mitch_lib::auth::normalize_email(&lock_admin) != mitch_lib::auth::normalize_email(&email)
        {
            return Some(crate::routes::me::json_response(
                403,
                json!({
                    "success": false,
                    "error": "This game PIN has been exclusively locked by an admin."
                }),
            ));
        }
    }

    tracing::info!(
        "[blooket-bot-ws] Upgrading client connection for {email} (PIN: {})",
        params.pin
    );

    // Stop any existing session first (server.js:11815-11822) — note the JS
    // does NOT process the queue here; the push happens on socket open.
    {
        let existing = lock_mutex(&state.blooket_active).shift_remove(&email);
        if let Some(existing) = existing {
            let _ = existing.ms_tx.send(MsCommand::Close);
        }
        lock_mutex(&state.blooket_queue).retain(|q| q.email != email);
    }

    let st = Arc::clone(state);
    Some(on_upgrade.on_upgrade(move |socket| async move {
        run_client_socket(st, socket, email, params).await;
    }))
}

/// The socket's `open` leg (server.js:24913-24918): push to the queue and
/// run the queue process, then the message/close loop.
async fn run_client_socket(
    state: Arc<AppState>,
    socket: WebSocket,
    email: String,
    params: BlooketParams,
) {
    let session_id = NEXT_SESSION_ID.fetch_add(1, Ordering::SeqCst);
    let (ctl_tx, mut ctl_rx) = mpsc::unbounded_channel();
    let open = Arc::new(AtomicBool::new(true));
    tracing::info!("[blooket-bot-ws] Client socket opened for {email}");
    lock_mutex(&state.blooket_queue).push(BlooketQueued {
        session_id,
        email: email.clone(),
        params: params.clone(),
        ctl: ctl_tx.clone(),
        open: Arc::clone(&open),
    });
    process_queue(&state);

    let (mut client_sink, mut client_stream) = socket.split();
    let mut ms_tx: Option<mpsc::UnboundedSender<MsCommand>> = None;
    let is_admin = mitch_lib::auth::is_admin_email(&state.store, &email);

    let mut ban_check = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        if mitch_lib::auth::banned_info_for_email(&state.store, &email).is_some() {
            let _ = client_sink
                .send(Message::Close(Some(CloseFrame {
                    code: 1008,
                    reason: "Account banned".into(),
                })))
                .await;
            break;
        }
        tokio::select! {
            _ = ban_check.tick() => continue,

            ctl = ctl_rx.recv() => match ctl {
                Some(BlooketCtl::Send(text)) => {
                    if client_sink.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                Some(BlooketCtl::Admit(tx)) => {
                    ms_tx = Some(tx);
                }
                Some(BlooketCtl::Close) => {
                    // JS `ws.close()` (no args) — Bun's default close code is
                    // 1000 on the wire.
                    let _ = client_sink
                        .send(Message::Close(Some(CloseFrame {
                            code: 1000,
                            reason: "".into(),
                        })))
                        .await;
                    break;
                }
                None => {
                    // The state-side senders are gone with the state itself;
                    // treat as close.
                    break;
                }
            },
            frame = client_stream.next() => {
                match frame {
                    Some(Ok(Message::Text(t))) => {
                        route_client_message(&email, is_admin, &ms_tx, &t.to_string());
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => {
                        break;
                    }
                    Some(Ok(_)) => {}
                }
            }
        }
    }

    // The close leg (server.js:25198-25205).
    tracing::info!("[blooket-bot-ws] Client socket closed for {email}");
    open.store(false, Ordering::SeqCst);
    let mut was_queued = false;
    lock_mutex(&state.blooket_queue).retain(|q| {
        if q.session_id == session_id {
            was_queued = true;
            false
        } else {
            true
        }
    });
    if was_queued {
        process_queue(&state);
    }
    // `active.clientWs = null` — the JS keeps the active record until the
    // ms closes or the next process sweep reaps it.
}

/// The message router (server.js:24970-24997). `payload.text` etc. keep the
/// JS `JSON.stringify` semantics: an `undefined` value drops the key.
fn route_client_message(
    email: &str,
    is_admin: bool,
    ms_tx: &Option<mpsc::UnboundedSender<MsCommand>>,
    text: &str,
) {
    let Ok(payload) = serde_json::from_str::<Value>(text) else {
        tracing::error!("[blooket-bot-ws] failed to route client message: parse error");
        return;
    };
    let msg_type = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let forward = |fields: &[&str]| {
        let mut obj = Map::new();
        obj.insert("type".to_string(), json!(msg_type));
        for f in fields {
            if let Some(v) = payload.get(*f) {
                obj.insert((*f).to_string(), v.clone());
            }
        }
        Value::Object(obj).to_string()
    };
    match msg_type {
        "input" | "gui-cheat" => {
            if let Some(tx) = ms_tx {
                let fields: &[&str] = if msg_type == "input" {
                    &["text"]
                } else {
                    &["category", "name", "value"]
                };
                let _ = tx.send(MsCommand::Send(forward(fields)));
            }
        }
        "eval-js" => {
            if is_admin {
                if let Some(tx) = ms_tx {
                    let _ = tx.send(MsCommand::Send(forward(&["code"])));
                }
            } else {
                tracing::warn!("[blooket-bot-ws] Non-admin {email} attempted to execute eval-js");
            }
        }
        _ => {}
    }
}

// ── HTTP endpoints (server.js:11834-11992) ───────────────────────────────────

/// Dispatches `/api/blooket-bot/*`; `None` falls through to the next group.
pub(crate) async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
    search: &str,
) -> Option<Response> {
    let _ = search;
    let get = *method == Method::GET;
    let post = *method == Method::POST;
    if path == "/api/blooket-bot/status" && get {
        return Some(status(state, headers));
    }
    if path == "/api/blooket-bot/stop" && post {
        return Some(stop(state, headers).await);
    }
    if path == "/api/blooket-bot/lock" && post {
        return Some(lock(state, headers, body_bytes).await);
    }
    if path == "/api/blooket-bot/report-failure" && post {
        return Some(report_failure(state, headers, body_bytes));
    }
    if path == "/api/blooket-bot/failure-reports" && get {
        return Some(failure_reports(state, headers));
    }
    None
}

/// The cookie ladder shared by the premium endpoints.
fn email_of(state: &Arc<AppState>, headers: &HeaderMap) -> Option<String> {
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cookies.get("id").unwrap_or(""))
        .to_string();
    mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid)
}

fn status(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let Some(email) = email_of(state, headers) else {
        return crate::routes::me::json_response(
            401,
            json!({ "success": false, "error": "Auth required" }),
        );
    };
    let is_premium = mitch_lib::auth::is_premium_email(&state.store, &email);
    let (active_count, queue_len) = {
        let actives = lock_mutex(&state.blooket_active);
        let queue = lock_mutex(&state.blooket_queue);
        (actives.len(), queue.len())
    };
    if !is_premium {
        return crate::routes::me::json_response(
            200,
            json!({
                "success": true,
                "isPremium": false,
                "activeCount": active_count,
                "queueLength": queue_len
            }),
        );
    }
    let (running, position) = {
        let actives = lock_mutex(&state.blooket_active);
        let queue = lock_mutex(&state.blooket_queue);
        (
            actives.contains_key(&email),
            queue.iter().position(|q| q.email == email),
        )
    };
    let locks = {
        let pin_locks = lock_mutex(&state.blooket_pin_locks);
        let mut obj = Map::new();
        for (pin, admin) in pin_locks.iter() {
            obj.insert(pin.clone(), json!(admin));
        }
        obj
    };
    crate::routes::me::json_response(
        200,
        json!({
            "success": true,
            "isPremium": true,
            "running": running,
            "inQueue": position.is_some(),
            "position": position.map(|i| i + 1),
            "queueLength": queue_len,
            "activeCount": active_count,
            "isAdmin": mitch_lib::auth::is_admin_email(&state.store, &email),
            "isMod": mitch_lib::auth::is_moderator_email(&state.store, &email),
            "locks": locks
        }),
    )
}

async fn stop(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let Some(email) = email_of(state, headers) else {
        return crate::routes::me::json_response(
            401,
            json!({ "success": false, "error": "Auth required" }),
        );
    };
    let mut stopped = false;
    if let Some(active) = lock_mutex(&state.blooket_active).shift_remove(&email) {
        // msWs.close() — the ms task's close leg sends 'Bot terminated.' and
        // closes the client asynchronously, exactly like the JS onclose.
        let _ = active.ms_tx.send(MsCommand::Close);
        stopped = true;
    }
    let mut was_queued = false;
    lock_mutex(&state.blooket_queue).retain(|q| {
        if q.email == email {
            was_queued = true;
            false
        } else {
            true
        }
    });
    if was_queued {
        stopped = true;
    }
    if stopped {
        process_queue(state);
        return crate::routes::me::json_response(
            200,
            json!({ "success": true, "message": "Bot stopped successfully." }),
        );
    }
    crate::routes::me::json_response(
        200,
        json!({ "success": true, "message": "No active bot found." }),
    )
}

async fn lock(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(email) = email_of(state, headers) else {
        return crate::routes::me::json_response(
            401,
            json!({ "success": false, "error": "Auth required" }),
        );
    };
    if !mitch_lib::auth::is_admin_email(&state.store, &email) {
        return crate::routes::me::json_response(
            403,
            json!({ "success": false, "error": "Admin access required" }),
        );
    }
    let Some(body) = crate::routes::me::parse_body_strict(body_bytes) else {
        return crate::routes::me::json_response(
            400,
            json!({ "success": false, "error": "Invalid PIN" }),
        );
    };
    let pin = jsval::string(&jsval::or(body.get("pin"), json!("")))
        .trim()
        .to_string();
    let lock_op = jsval::truthy(&jsval::or(body.get("lock"), json!(false)));
    if pin.is_empty() {
        return crate::routes::me::json_response(
            400,
            json!({ "success": false, "error": "Invalid PIN" }),
        );
    }

    if lock_op {
        lock_mutex(&state.blooket_pin_locks).insert(pin.clone(), email.clone());
        tracing::info!("[blooket-bot] Admin {email} locked PIN {pin} exclusively");

        // Kick other users' active bots for this PIN.
        let kicks: Vec<(String, mpsc::UnboundedSender<MsCommand>)> = {
            let actives = lock_mutex(&state.blooket_active);
            actives
                .iter()
                .filter(|(active_email, session)| {
                    session.params.pin == pin
                        && mitch_lib::auth::normalize_email(active_email)
                            != mitch_lib::auth::normalize_email(&email)
                })
                .map(|(k, v)| (k.clone(), v.ms_tx.clone()))
                .collect()
        };
        for (kick_email, ms_tx) in kicks {
            tracing::info!(
                "[blooket-bot] Admin locked PIN {pin}, kicking active bot for {kick_email}"
            );
            let ctl = {
                let actives = lock_mutex(&state.blooket_active);
                actives.get(&kick_email).map(|r| r.ctl.clone())
            };
            if let Some(ctl) = ctl {
                let _ = ctl.send(BlooketCtl::Send(
                    json!({
                        "type": "error",
                        "message": "Kicked: This game PIN has been exclusively locked by an admin."
                    })
                    .to_string(),
                ));
            }
            let _ = ms_tx.send(MsCommand::Close);
            lock_mutex(&state.blooket_active).shift_remove(&kick_email);
        }

        // Kick other users' queued bots for this PIN.
        let queue_kicks: Vec<(u64, String, mpsc::UnboundedSender<BlooketCtl>)> = {
            let queue = lock_mutex(&state.blooket_queue);
            queue
                .iter()
                .filter(|q| {
                    q.params.pin == pin
                        && mitch_lib::auth::normalize_email(&q.email)
                            != mitch_lib::auth::normalize_email(&email)
                })
                .map(|q| (q.session_id, q.email.clone(), q.ctl.clone()))
                .collect()
        };
        for (session_id, kick_email, ctl) in queue_kicks {
            tracing::info!(
                "[blooket-bot] Admin locked PIN {pin}, removing queued request for {kick_email}"
            );
            let _ = ctl.send(BlooketCtl::Send(
                json!({
                    "type": "error",
                    "message": "This game PIN has been exclusively locked by an admin."
                })
                .to_string(),
            ));
            lock_mutex(&state.blooket_queue).retain(|q| q.session_id != session_id);
        }
        process_queue(state);
    } else {
        lock_mutex(&state.blooket_pin_locks).shift_remove(&pin);
        tracing::info!("[blooket-bot] Admin {email} unlocked PIN {pin}");
    }

    crate::routes::me::json_response(
        200,
        json!({
            "success": true,
            "message": if lock_op {
                format!("PIN {pin} locked successfully.")
            } else {
                format!("PIN {pin} unlocked successfully.")
            }
        }),
    )
}

fn report_failure(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(email) = email_of(state, headers) else {
        return crate::routes::me::json_response(
            401,
            json!({ "success": false, "error": "Auth required" }),
        );
    };
    let Some(body) = crate::routes::me::parse_body_strict(body_bytes) else {
        return crate::routes::me::json_response(
            400,
            json!({ "success": false, "error": "Console log is required" }),
        );
    };
    let pin = jsval::string(&jsval::or(body.get("pin"), json!("")))
        .trim()
        .to_string();
    let nickname = jsval::string(&jsval::or(body.get("nickname"), json!("")))
        .trim()
        .to_string();
    let console_log = jsval::string(&jsval::or(body.get("consoleLog"), json!("")))
        .trim()
        .to_string();
    if console_log.is_empty() {
        return crate::routes::me::json_response(
            400,
            json!({ "success": false, "error": "Console log is required" }),
        );
    }
    let file = state.data_dir().join("blooket_bot_failures.json");
    let mut reports = state
        .store
        .read_document(&file, json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    reports.push(json!({
        "email": email,
        "pin": pin,
        "nickname": nickname,
        "consoleLog": console_log,
        "ts": now_millis() as f64 / 1000.0
    }));
    if reports.len() > 200 {
        // Array.prototype.shift — drop the oldest.
        reports.remove(0);
    }
    let _ = state.store.write_document(&file, &Value::Array(reports));
    crate::routes::me::json_response(
        200,
        json!({ "success": true, "message": "Failure report submitted successfully." }),
    )
}

fn failure_reports(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cookies.get("id").unwrap_or(""))
        .to_string();
    if !mitch_lib::auth::is_any_admin_id(&state.store, &state.id_secret, &sid, false) {
        return crate::routes::me::json_response(
            403,
            json!({ "success": false, "error": "Forbidden" }),
        );
    }
    let reports = state.store.read_document(
        &state.data_dir().join("blooket_bot_failures.json"),
        json!([]),
    );
    crate::routes::me::json_response(200, json!({ "success": true, "reports": reports }))
}

fn parse_query(search: &str) -> Vec<(String, String)> {
    form_urlencoded::parse(search.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

/// The upgrade query params with the JS `||` defaults (server.js:11804-11808)
/// — an empty value falls through to the default, and `headless !== "false"`.
fn ws_params_from_search(search: &str) -> BlooketParams {
    let query = parse_query(search);
    let get = |k: &str| {
        let found = query
            .iter()
            .find(|(k2, _)| k2 == k)
            .map(|(_, v)| json!(v.clone()));
        jsval::string(&jsval::or(found.as_ref(), json!("")))
    };
    let pin = {
        let raw = get("pin");
        if raw.is_empty() {
            "7174055".to_string()
        } else {
            raw
        }
    };
    let name = {
        let raw = get("name");
        if raw.is_empty() {
            "AutomatedBot".to_string()
        } else {
            raw
        }
    };
    let auto = {
        let raw = get("auto");
        if raw.is_empty() {
            "none".to_string()
        } else {
            raw
        }
    };
    BlooketParams {
        pin,
        name,
        auto,
        headless: get("headless") != "false",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_param_defaults() {
        let p = ws_params_from_search("");
        assert_eq!(p.pin, "7174055");
        assert_eq!(p.name, "AutomatedBot");
        assert_eq!(p.auto, "none");
        assert!(p.headless);
        let p = ws_params_from_search("pin=1234&name=&auto=aggressive&headless=false");
        assert_eq!(p.pin, "1234");
        assert_eq!(p.name, "AutomatedBot");
        assert_eq!(p.auto, "aggressive");
        assert!(!p.headless);
        let p = ws_params_from_search("headless=true");
        assert!(p.headless);
    }

    #[tokio::test]
    async fn message_router_forwards_input() {
        let (ms_tx, mut ms_rx) = mpsc::unbounded_channel();
        let ms_tx = Some(ms_tx);
        route_client_message("a@b", false, &ms_tx, r#"{"type":"input","text":"hi"}"#);
        let cmd = ms_rx.recv().await;
        match cmd {
            Some(MsCommand::Send(s)) => assert_eq!(s, r#"{"type":"input","text":"hi"}"#),
            _ => unreachable!("expected Send"),
        }
        // eval-js is admin-gated and silently skipped for non-admins.
        route_client_message("a@b", false, &ms_tx, r#"{"type":"eval-js","code":"1+1"}"#);
        // Unknown types route nowhere.
        route_client_message("a@b", true, &ms_tx, r#"{"type":"zzz"}"#);
        // An undefined field is dropped, like JSON.stringify({text: undefined}).
        route_client_message("a@b", true, &ms_tx, r#"{"type":"input"}"#);
        let cmd = ms_rx.recv().await;
        match cmd {
            Some(MsCommand::Send(s)) => assert_eq!(s, r#"{"type":"input"}"#),
            _ => unreachable!("expected eval-js send"),
        }
        assert!(ms_rx.try_recv().is_err());
    }
}
