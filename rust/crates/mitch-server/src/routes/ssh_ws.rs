//! `/ssh/ws` — the terminal bridge to the Rust ssh-gateway (plan Step 13).
//!
//! Ports (server.js):
//! - the upgrade gate (11738-11752): sid ladder, signed-in members reuse their
//!   site session, the sessionless break-glass console needs an admin
//!   passphrase. An upgrade failure falls through (no 400) — the JS `if
//!   (success) return;` just skips the block.
//! - the message machine (25067-25212): per-message passphrase auth, the lazy
//!   gateway connect with its 8s timeout, the VM-ownership authorization for
//!   non-admins (`getVmConnectionIpForEmail` from routes/vm.rs), and the
//!   connect/data/resize forwarding. Unknown message types are ignored.
//!
//! Divergences (documented, both unreachable in practice): gateway binary
//! frames are forwarded lossy-UTF-8 (the gateway's protocol is JSON text —
//! the JS `String(ev.data)` on a binary would stringify the object anyway);
//! the JS secret-nulling after the connect frame is memory hygiene with no
//! wire effect and is not replicated.

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use mitch_lib::jsval;
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use crate::state::AppState;

// ── upgrade gate (server.js:11738-11752) ─────────────────────────────────────

pub fn handle_ws_upgrade(
    state: &Arc<AppState>,
    path: &str,
    headers: &HeaderMap,
    upgrade: Option<WebSocketUpgrade>,
) -> Option<Response> {
    if path != "/ssh/ws" {
        return None;
    }
    let Some(on_upgrade) = upgrade else {
        // JS `if (success) return;` — a failed upgrade falls through to the
        // blocks below (unlike /livekit/rtc, which 400s here).
        return None;
    };

    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cookies.get("id").unwrap_or(""))
        .to_string();
    let is_authenticated = !sid.is_empty()
        && mitch_lib::auth::valid_id(&sid, &state.id_secret)
        && !is_revoked_id(state, &sid);
    // JS lowercases here; normalize_email inside the message machine is a
    // second, equivalent lowercase.
    let email = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid)
        .unwrap_or_default()
        .to_lowercase();
    let is_admin = mitch_lib::auth::is_any_admin_id(&state.store, &state.id_secret, &sid, false);
    let require_passphrase = !is_authenticated && !is_admin;

    let st = Arc::clone(state);
    Some(on_upgrade.on_upgrade(move |socket| async move {
        run_session(
            st,
            socket,
            SshSess {
                sid,
                email,
                require_passphrase,
            },
        )
        .await;
    }))
}

/// Per-socket session data (server.js `ws.data`).
struct SshSess {
    sid: String,
    /// Lowercased at upgrade; replaced with 'admin@mitch.pro' after a
    /// successful passphrase auth.
    email: String,
    require_passphrase: bool,
}

/// Gateway-bound command for the pump task.
enum GwCmd {
    Send(String),
    Close,
}

/// Gateway → client-loop events.
enum GwEvent {
    Frame(String),
    Closed,
}

/// What the message machine wants after one client frame. `Send` is used when
/// the gateway pipe already exists (JS `ensureGateway` returns true
/// immediately); `Connect` carries the frame to forward after the lazy
/// connect succeeds. `Stop` closes the client after sending the optional
/// error frame (JS sends the error then `ws.close()`).
#[derive(Debug)]
enum SshAction {
    Continue,
    Send(Value),
    Connect(Value),
    Stop(Option<String>),
}

fn ssh_error_json(message: &str) -> String {
    json!({ "type": "error", "message": message }).to_string()
}

fn is_revoked_id(state: &AppState, sid: &str) -> bool {
    mitch_lib::revoked::revoked_id_exists(&state.store, sid)
}

// ── the session loop ─────────────────────────────────────────────────────────

async fn run_session(state: Arc<AppState>, socket: WebSocket, mut sess: SshSess) {
    let (mut client_sink, mut client_stream) = socket.split();
    // JS `ws.data.sshGateway` — None until the first message that needs it.
    let mut gw: Option<(
        mpsc::UnboundedSender<GwCmd>,
        mpsc::UnboundedReceiver<GwEvent>,
    )> = None;

    let mut ban_check = tokio::time::interval(Duration::from_secs(1));
    loop {
        if mitch_lib::auth::banned_info_for_email(&state.store, &sess.email).is_some() {
            let _ = client_sink
                .send(Message::Close(Some(CloseFrame {
                    code: 1008,
                    reason: "Account banned".into(),
                })))
                .await;
            break;
        }
        // One select step; processing happens after the borrows are released.
        enum Step {
            Tick,
            Gw(Option<GwEvent>),
            Client(Option<Result<Message, axum::Error>>),
        }
        let step = if let Some((_, gw_rx)) = gw.as_mut() {
            tokio::select! {
                _ = ban_check.tick() => Step::Tick,
                ev = gw_rx.recv() => Step::Gw(ev),
                frame = client_stream.next() => Step::Client(frame),
            }
        } else {
            tokio::select! {
                _ = ban_check.tick() => Step::Tick,
                frame = client_stream.next() => Step::Client(frame),
            }
        };

        match step {
            Step::Tick => continue,
            Step::Gw(Some(GwEvent::Frame(text))) => {
                // JS upstream.onmessage → `ws.send(...)`.
                if client_sink.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            Step::Gw(_) => {
                // JS upstream.onclose → ws.close() (no code), and a dropped
                // event channel means the pump is gone too. Bun's `ws.close()`
                // with no args defaults to close code 1000 on the wire, so the
                // frame must carry 1000 explicitly (an empty frame would show
                // up as 1005 in the client).
                let _ = client_sink
                    .send(Message::Close(Some(CloseFrame {
                        code: 1000,
                        reason: "".into(),
                    })))
                    .await;
                break;
            }
            Step::Client(Some(Ok(Message::Text(t)))) => {
                let cmd = gw.as_ref().map(|(c, _)| c.clone());
                match handle_client_frame(&state, &mut sess, &t.to_string(), cmd.as_ref()).await {
                    SshAction::Continue => {}
                    SshAction::Send(v) => {
                        if let Some(c) = cmd {
                            let _ = c.send(GwCmd::Send(v.to_string()));
                        }
                    }
                    SshAction::Connect(v) => {
                        if !ensure_gateway(&mut gw, &mut client_sink).await {
                            break;
                        }
                        if let Some((c, _)) = gw.as_ref() {
                            let _ = c.send(GwCmd::Send(v.to_string()));
                        }
                    }
                    SshAction::Stop(msg) => {
                        // JS sends the error frame inside the handler, then
                        // `ws.close()` here (Bun's no-arg close = code 1000).
                        if let Some(m) = msg {
                            let _ = client_sink
                                .send(Message::Text(ssh_error_json(&m).into()))
                                .await;
                        }
                        let _ = client_sink
                            .send(Message::Close(Some(CloseFrame {
                                code: 1000,
                                reason: "".into(),
                            })))
                            .await;
                        break;
                    }
                }
            }
            Step::Client(Some(Ok(Message::Close(_))))
            | Step::Client(Some(Err(_)))
            | Step::Client(None) => break,
            Step::Client(Some(Ok(_))) => {
                // Binary/ping/pong — the JS terminal app speaks JSON text.
            }
        }
    }

    // JS close handler (25205-25208): sshGateway.close().
    if let Some((c, _)) = gw.take() {
        let _ = c.send(GwCmd::Close);
    }
}

// ── the message machine (server.js:25070-25204) ──────────────────────────────

async fn handle_client_frame(
    state: &Arc<AppState>,
    sess: &mut SshSess,
    text: &str,
    cmd: Option<&mpsc::UnboundedSender<GwCmd>>,
) -> SshAction {
    let Ok(payload) = serde_json::from_str::<Value>(text) else {
        // Silent catch — avoid logging credential-bearing payloads.
        return SshAction::Continue;
    };

    // The break-glass console authenticates on its first message (the JS
    // checks the flag on EVERY message until it clears).
    if sess.require_passphrase {
        let pass = jsval::string(&jsval::or(payload.get("adminPassphrase"), json!("")));
        let verified = mitch_lib::admin::verify_admin_passphrase_raw(
            &state.store,
            &state.id_secret,
            state.data_dir(),
            &sess.sid,
            &pass,
        );
        if !verified {
            tracing::warn!("[ssh-security] Admin passphrase verification failed");
            return SshAction::Stop(Some("Admin passphrase verification failed.".to_string()));
        }
        sess.require_passphrase = false;
        sess.email = "admin@mitch.pro".to_string();
    }

    let typ = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match typ {
        "init" | "connect" => {
            let host_ip = jsval::string(&jsval::or(payload.get("host"), json!("")))
                .trim()
                .to_string();
            let email_norm = mitch_lib::auth::normalize_email(&sess.email);
            let is_user_admin =
                mitch_lib::auth::is_any_admin_id(&state.store, &state.id_secret, &sess.sid, false)
                    || email_norm == "admin@mitch.pro";

            if !is_user_admin {
                let allowed_ip =
                    crate::routes::vm::get_vm_connection_ip_for_email(state, &email_norm).await;
                if host_ip != allowed_ip {
                    tracing::warn!(
                        "[ssh-security] Blocked SSH connection attempt by {} to unauthorized host {host_ip}",
                        sess.email
                    );
                    return SshAction::Stop(Some(
                        "Access Denied: You can only connect to your own VM.".to_string(),
                    ));
                }
            }

            tracing::info!(
                "[ssh] {} -> {host_ip}:{}",
                sess.email,
                jsval::string(&jsval::or(payload.get("port"), json!(22)))
            );

            // The connect frame: JSON.stringify drops undefined keys, so a
            // missing payload key is omitted (preserve_order keeps the JS
            // literal's key order).
            let mut fwd = Map::new();
            fwd.insert("type".to_string(), json!("connect"));
            if let Some(h) = payload.get("host") {
                fwd.insert("host".to_string(), h.clone());
            }
            fwd.insert(
                "port".to_string(),
                jsval::or(payload.get("port"), json!(22)),
            );
            if let Some(u) = payload.get("username") {
                fwd.insert("username".to_string(), u.clone());
            }
            fwd.insert(
                "cols".to_string(),
                jsval::or(payload.get("cols"), json!(80)),
            );
            fwd.insert(
                "rows".to_string(),
                jsval::or(payload.get("rows"), json!(24)),
            );

            if payload
                .get("useSavedKey")
                .map(jsval::truthy)
                .unwrap_or(false)
            {
                let saved = state.store.read_document(
                    &crate::routes::me::data_file(state, "admin_ssh_keys.json"),
                    json!({}),
                );
                let user_key = saved.get(mitch_lib::auth::normalize_email(&sess.email).as_str());
                let key = user_key
                    .and_then(|k| k.get("privateKey"))
                    .filter(|v| jsval::truthy(v));
                let Some(key) = key else {
                    return SshAction::Stop(Some("No saved SSH key found.".to_string()));
                };
                fwd.insert("privateKey".to_string(), key.clone());
                if payload
                    .get("passphrase")
                    .map(jsval::truthy)
                    .unwrap_or(false)
                {
                    if let Some(p) = payload.get("passphrase") {
                        fwd.insert("passphrase".to_string(), p.clone());
                    }
                }
            } else if payload
                .get("privateKey")
                .map(jsval::truthy)
                .unwrap_or(false)
            {
                fwd.insert(
                    "privateKey".to_string(),
                    payload.get("privateKey").cloned().unwrap_or(Value::Null),
                );
                if payload
                    .get("passphrase")
                    .map(jsval::truthy)
                    .unwrap_or(false)
                {
                    if let Some(p) = payload.get("passphrase") {
                        fwd.insert("passphrase".to_string(), p.clone());
                    }
                }
            } else if let Some(p) = payload.get("password") {
                fwd.insert("password".to_string(), p.clone());
            }

            let fwd = Value::Object(fwd);
            match cmd {
                Some(_) => SshAction::Send(fwd),
                None => SshAction::Connect(fwd),
            }
        }
        "data" | "resize" => match cmd {
            Some(_) => SshAction::Send(payload),
            None => SshAction::Connect(payload),
        },
        // Other types: ignored (the JS falls through the if-chain silently).
        _ => SshAction::Continue,
    }
}

// ── the lazy gateway pipe (server.js:25086-25117) ────────────────────────────

async fn ensure_gateway(
    gw: &mut Option<(
        mpsc::UnboundedSender<GwCmd>,
        mpsc::UnboundedReceiver<GwEvent>,
    )>,
    client_sink: &mut futures_util::stream::SplitSink<WebSocket, Message>,
) -> bool {
    if gw.is_some() {
        return true;
    }
    let gateway_url = std::env::var("SSH_GATEWAY_URL")
        .unwrap_or_else(|_| "ws://ssh-gateway:6820".to_string())
        .trim()
        .to_string();
    tracing::info!("[ssh] Connecting to gateway at {gateway_url}");
    let res = tokio::time::timeout(
        Duration::from_secs(8),
        tokio_tungstenite::connect_async(&gateway_url),
    )
    .await;
    let Ok(Ok((upstream, _))) = res else {
        // JS ensureGateway catch: error frame + ws.close().
        let _ = client_sink
            .send(Message::Text(
                ssh_error_json("SSH gateway unavailable").into(),
            ))
            .await;
        // Bun's no-arg `ws.close()` = close code 1000 on the wire.
        let _ = client_sink
            .send(Message::Close(Some(CloseFrame {
                code: 1000,
                reason: "".into(),
            })))
            .await;
        return false;
    };
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    tokio::spawn(gateway_pump(upstream, cmd_rx, ev_tx));
    *gw = Some((cmd_tx, ev_rx));
    true
}

/// Owns the gateway stream: forwards its frames to the session loop and its
/// close as `GwEvent::Closed` (JS upstream.onclose → ws.close()).
async fn gateway_pump(
    ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    mut cmd_rx: mpsc::UnboundedReceiver<GwCmd>,
    ev_tx: mpsc::UnboundedSender<GwEvent>,
) {
    let (mut sink, mut stream) = ws.split();
    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => match cmd {
                Some(GwCmd::Send(text)) => {
                    if sink
                        .send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Some(GwCmd::Close) | None => {
                    let _ = sink.close().await;
                    break;
                }
            },
            msg = stream.next() => match msg {
                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t))) => {
                    if ev_tx.send(GwEvent::Frame(t.to_string())).is_err() {
                        break;
                    }
                }
                Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(b))) => {
                    // JS `String(ev.data)` on binary is a stringification of
                    // the object; the gateway speaks JSON text, so a lossy
                    // UTF-8 view is the closest sane behavior.
                    if ev_tx
                        .send(GwEvent::Frame(String::from_utf8_lossy(&b).into_owned()))
                        .is_err()
                    {
                        break;
                    }
                }
                _ => break,
            },
        }
    }
    let _ = sink.close().await;
    let _ = ev_tx.send(GwEvent::Closed);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> (Arc<AppState>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "mitch-server-ssh-ws-test-{}",
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

    fn admin_sess() -> SshSess {
        SshSess {
            sid: String::new(),
            email: "admin@mitch.pro".to_string(),
            require_passphrase: false,
        }
    }

    fn user_sess(email: &str) -> SshSess {
        SshSess {
            sid: String::new(),
            email: email.to_string(),
            require_passphrase: false,
        }
    }

    #[tokio::test]
    async fn connect_frame_defaults_and_dropped_keys() {
        // JS JSON.stringify drops undefined keys: a missing `username` is
        // omitted; port/cols/rows fall back through the `||` ladder.
        let (state, _dir) = test_state();
        let mut sess = admin_sess();
        let action = handle_client_frame(
            &state,
            &mut sess,
            r#"{"type":"connect","password":"secret"}"#,
            None,
        )
        .await;
        match action {
            SshAction::Connect(v) => {
                let m = v.as_object().unwrap_or_else(|| panic!("map: {v}"));
                assert_eq!(m.get("type"), Some(&json!("connect")));
                assert_eq!(m.get("port"), Some(&json!(22)));
                assert_eq!(m.get("cols"), Some(&json!(80)));
                assert_eq!(m.get("rows"), Some(&json!(24)));
                assert_eq!(m.get("password"), Some(&json!("secret")));
                assert!(!m.contains_key("username"));
                assert!(!m.contains_key("privateKey"));
            }
            other => panic!("expected Connect, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn connect_frame_raw_values_preserved() {
        // JS `payload.cols || 80` keeps a truthy string ("120") untouched.
        let (state, _dir) = test_state();
        let mut sess = admin_sess();
        let action = handle_client_frame(
            &state,
            &mut sess,
            r#"{"type":"init","host":"10.0.0.5","username":"mitch","cols":"120","rows":40,"port":2222,"privateKey":"KEY"}"#,
            None,
        )
        .await;
        let SshAction::Connect(v) = action else {
            panic!("expected Connect");
        };
        let m = v.as_object().unwrap_or_else(|| panic!("map: {v}"));
        assert_eq!(m.get("host"), Some(&json!("10.0.0.5")));
        assert_eq!(m.get("username"), Some(&json!("mitch")));
        assert_eq!(m.get("cols"), Some(&json!("120")));
        assert_eq!(m.get("rows"), Some(&json!(40)));
        assert_eq!(m.get("port"), Some(&json!(2222)));
        assert_eq!(m.get("privateKey"), Some(&json!("KEY")));
        assert!(!m.contains_key("password"));
        // JS nulls the payload's secrets after building the frame (memory
        // hygiene); the forwarded frame itself carries them.
    }

    #[tokio::test]
    async fn data_and_resize_forward_raw_payload() {
        let (state, _dir) = test_state();
        let mut sess = admin_sess();
        let action =
            handle_client_frame(&state, &mut sess, r#"{"type":"data","data":"ls\n"}"#, None).await;
        match action {
            SshAction::Connect(v) => {
                assert_eq!(v.get("type"), Some(&json!("data")));
                assert_eq!(v.get("data"), Some(&json!("ls\n")));
            }
            other => panic!("expected Connect, got {other:?}"),
        }
        let action = handle_client_frame(
            &state,
            &mut sess,
            r#"{"type":"resize","cols":120,"rows":40}"#,
            None,
        )
        .await;
        match action {
            SshAction::Connect(v) => {
                assert_eq!(v.get("type"), Some(&json!("resize")));
                assert_eq!(v.get("cols"), Some(&json!(120)));
            }
            other => panic!("expected Connect, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn non_admin_blocked_from_foreign_host() {
        // No VM registrations → allowed IP resolves '' → any host is denied.
        let (state, _dir) = test_state();
        let mut sess = user_sess("student@mitch.pro");
        let action = handle_client_frame(
            &state,
            &mut sess,
            r#"{"type":"connect","host":"10.0.0.250"}"#,
            None,
        )
        .await;
        match action {
            SshAction::Stop(Some(m)) => {
                assert_eq!(m, "Access Denied: You can only connect to your own VM.");
            }
            other => panic!("expected Stop, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn non_admin_allowed_for_registered_vm() {
        // activeFreeVms registers vmid 250 → the PVE token is unset here, so
        // the LXC fallback IP 10.0.0.250 is what the user may connect to.
        let (state, dir) = test_state();
        let norm = mitch_lib::auth::normalize_email("student@mitch.pro");
        state
            .active_free_vms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(norm, json!({ "vmid": 250.0 }));
        let mut sess = user_sess("student@mitch.pro");
        let action = handle_client_frame(
            &state,
            &mut sess,
            r#"{"type":"connect","host":"10.0.0.250","username":"student","password":"pw"}"#,
            None,
        )
        .await;
        let SshAction::Connect(v) = action else {
            let allowed =
                crate::routes::vm::get_vm_connection_ip_for_email(&state, "student@mitch.pro")
                    .await;
            panic!("expected Connect, allowed ip = {allowed:?}");
        };
        assert_eq!(v.get("host"), Some(&json!("10.0.0.250")));
        let _ = dir;
    }

    #[tokio::test]
    async fn saved_key_lookup_and_passphrase_rule() {
        // The saved-key branch reads admin_ssh_keys.json by normalized email
        // and only forwards a passphrase when the client sent one.
        let (state, dir) = test_state();
        std::fs::write(
            dir.join("admin_ssh_keys.json"),
            r#"{"admin@mitch.pro":{"privateKey":"SAVEDKEY"}}"#,
        )
        .unwrap_or_default();
        let mut sess = admin_sess();
        let action = handle_client_frame(
            &state,
            &mut sess,
            r#"{"type":"connect","useSavedKey":true,"passphrase":"pp"}"#,
            None,
        )
        .await;
        let SshAction::Connect(v) = action else {
            panic!("expected Connect");
        };
        assert_eq!(v.get("privateKey"), Some(&json!("SAVEDKEY")));
        assert_eq!(v.get("passphrase"), Some(&json!("pp")));
        assert!(!v
            .as_object()
            .unwrap_or_else(|| panic!("map"))
            .contains_key("password"));

        // Without a passphrase in the payload the key stays in and no
        // passphrase is forwarded.
        let mut sess = admin_sess();
        let action = handle_client_frame(
            &state,
            &mut sess,
            r#"{"type":"connect","useSavedKey":true}"#,
            None,
        )
        .await;
        let SshAction::Connect(v) = action else {
            panic!("expected Connect");
        };
        assert_eq!(v.get("privateKey"), Some(&json!("SAVEDKEY")));
        assert!(!v
            .as_object()
            .unwrap_or_else(|| panic!("map"))
            .contains_key("passphrase"));
    }

    #[tokio::test]
    async fn missing_saved_key_fails_closed() {
        let (state, _dir) = test_state();
        let mut sess = admin_sess();
        let action = handle_client_frame(
            &state,
            &mut sess,
            r#"{"type":"connect","useSavedKey":true}"#,
            None,
        )
        .await;
        match action {
            SshAction::Stop(Some(m)) => assert_eq!(m, "No saved SSH key found."),
            other => panic!("expected Stop, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn passphrase_gate() {
        // No passphrase file at all → verification fails closed.
        let (state, dir) = test_state();
        let mut sess = SshSess {
            sid: String::new(),
            email: String::new(),
            require_passphrase: true,
        };
        let action = handle_client_frame(&state, &mut sess, r#"{"type":"init"}"#, None).await;
        match action {
            SshAction::Stop(Some(m)) => {
                assert_eq!(m, "Admin passphrase verification failed.");
                // The flag stays set — the JS only clears it on success.
                assert!(sess.require_passphrase);
            }
            other => panic!("expected Stop, got {other:?}"),
        }

        // A stored argon2 hash for admin@mitch.pro verifies and clears the
        // flag + rewrites the session email.
        let hash = mitch_lib::crypto::argon2_hash("hunter2");
        std::fs::write(
            dir.join("admin_passphrase.json"),
            json!({ "admin@mitch.pro": { "hash": hash } }).to_string(),
        )
        .unwrap_or_default();
        let mut sess = SshSess {
            sid: String::new(),
            email: String::new(),
            require_passphrase: true,
        };
        let action = handle_client_frame(
            &state,
            &mut sess,
            r#"{"type":"init","adminPassphrase":"hunter2"}"#,
            None,
        )
        .await;
        assert!(matches!(
            action,
            SshAction::Continue | SshAction::Connect(_)
        ));
        assert!(!sess.require_passphrase);
        assert_eq!(sess.email, "admin@mitch.pro");
    }

    #[tokio::test]
    async fn parse_failures_and_unknown_types_are_silent() {
        let (state, _dir) = test_state();
        let mut sess = admin_sess();
        assert!(matches!(
            handle_client_frame(&state, &mut sess, "not json", None).await,
            SshAction::Continue
        ));
        assert!(matches!(
            handle_client_frame(&state, &mut sess, r#"{"type":"eval-js"}"#, None).await,
            SshAction::Continue
        ));
    }
}
