//! Owner-only Proxmox container (VMID [`OWNER_CONTAINER_VMID`]).
//!
//! The container is created by hand in Proxmox; this module never creates or
//! deletes it. It only lets the owner see its state, power it, and open its
//! console through the existing noVNC bridge.
//!
//! Nobody else may learn it exists, so every non-owner request (anonymous,
//! moderator, admin, developer) gets the same 404 as an unknown route, and
//! nothing here writes to the VM audit log or the system log, both of which
//! admins/developers can read. The other places that could leak it are closed
//! elsewhere: `ProxmoxDesktopService::list_guests` drops it and
//! `next_available_vmid` reserves its id. Details in `docs/owner-container.md`.
//!
//! Two console modes: the Proxmox text console (`vncproxy`), and, when
//! `OWNER_CONTAINER_VNC_HOST` is set, the container's own VNC server (a GNOME
//! session) reached over TCP by [`bridge_vnc_tcp`].

use std::sync::Arc;

use axum::http::{HeaderMap, Method};
use serde_json::{json, Value};

use super::{json_response, Resp};
use crate::proxmox_desktop::desktop;
use crate::state::AppState;
use mitch_lib::jsval;
use mitch_lib::vm::OWNER_CONTAINER_VMID;

const BASE: &str = "/api/admin/owner-container";

/// The container is shut down after this long without owner input.
const IDLE_LIMIT_MS: i64 = 10 * 60 * 1000;
/// How often the idle check runs.
const IDLE_CHECK_SECS: u64 = 60;

/// When the owner last did something with the container (millis; 0 = not
/// known yet). In memory only: nothing about this is written anywhere
/// admins or developers can read.
static LAST_ACTIVITY: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

fn note_activity() {
    LAST_ACTIVITY.store(
        mitch_lib::school::now_millis(),
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// What the idle check does for one look at the container. Returns the new
/// activity time and whether to shut the container down.
///
/// A container that is not running clears the clock, so one started some other
/// way (the Proxmox UI) gets a fresh ten minutes the first time it is seen
/// running instead of being judged by an old timestamp.
fn idle_decision(running: bool, last: i64, now: i64) -> (i64, bool) {
    if !running {
        return (0, false);
    }
    if last == 0 {
        return (now, false);
    }
    if now - last >= IDLE_LIMIT_MS {
        // Restart the clock so a slow shutdown isn't requested again each tick.
        return (now, true);
    }
    (last, false)
}

/// Starts the background check that shuts the container down after ten
/// minutes without owner input. Failures are ignored: this module never logs.
pub fn spawn_idle_shutdown() {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(IDLE_CHECK_SECS));
        loop {
            tick.tick().await;
            let Ok(Some(guest)) = find_container().await else {
                continue;
            };
            let running = jsval::str_or(guest.get("status"), "") == "running";
            let last = LAST_ACTIVITY.load(std::sync::atomic::Ordering::Relaxed);
            let (next, shutdown) = idle_decision(running, last, mitch_lib::school::now_millis());
            LAST_ACTIVITY.store(next, std::sync::atomic::Ordering::Relaxed);
            if shutdown {
                let _ = desktop().power(&record_for(&guest), "shutdown", true).await;
            }
        }
    });
}

/// The container's own VNC server, from the server environment only (never
/// from a request, so this can't be pointed at an arbitrary host).
/// `OWNER_CONTAINER_VNC_HOST` enables it; `_PORT` defaults to 5901;
/// `_PASSWORD` is handed to the owner's noVNC client when the server asks.
struct VncTarget {
    host: String,
    port: u16,
    password: String,
}

fn vnc_target() -> Option<VncTarget> {
    let host = std::env::var("OWNER_CONTAINER_VNC_HOST")
        .ok()?
        .trim()
        .to_string();
    if host.is_empty() {
        return None;
    }
    let port = std::env::var("OWNER_CONTAINER_VNC_PORT")
        .ok()
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or(5901);
    let password = std::env::var("OWNER_CONTAINER_VNC_PASSWORD").unwrap_or_default();
    Some(VncTarget {
        host,
        port,
        password,
    })
}

fn not_found() -> axum::response::Response {
    json_response(404, json!({ "error": "Not found" }))
}

/// The container's row from Proxmox, or `None` when it has not been created.
async fn find_container() -> Result<Option<Value>, crate::proxmox_desktop::ProxmoxServiceError> {
    let guests = desktop().list_guests_all().await?;
    Ok(guests
        .into_iter()
        .find(|g| jsval::number(&g["vmid"]).map(|n| n as i64) == Some(OWNER_CONTAINER_VMID)))
}

/// The record shape `power`/`create_console` expect, built from live Proxmox
/// state rather than a database row (there is deliberately no
/// `virtual_machines` row, so the inactivity worker and every admin VM list
/// can never touch it).
fn record_for(guest: &Value) -> Value {
    json!({
        "vmid": OWNER_CONTAINER_VMID,
        "node": guest["node"],
        "guestType": guest["type"],
    })
}

fn proxmox_error(err: crate::proxmox_desktop::ProxmoxServiceError) -> axum::response::Response {
    json_response(
        err.status,
        json!({ "error": err.message, "code": err.code }),
    )
}

pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body: &Value,
) -> Resp {
    if path != BASE && !path.starts_with(&format!("{BASE}/")) {
        return None;
    }
    let owner = crate::routes::vm::authenticated_vm_actor(state, headers)
        .filter(|a| mitch_lib::auth::is_owner_email(&state.store, &a.email));
    let Some(actor) = owner else {
        return Some(not_found());
    };
    if *method == Method::POST && !crate::routes::vm::vm_same_origin_request(headers) {
        return Some(json_response(
            403,
            json!({ "error": "Request origin rejected." }),
        ));
    }

    // The desktop page pings this while the owner is using the container.
    if path == format!("{BASE}/activity") && *method == Method::POST {
        note_activity();
        return Some(json_response(200, json!({ "ok": true })));
    }

    let guest = match find_container().await {
        Ok(g) => g,
        Err(e) => return Some(proxmox_error(e)),
    };

    if path == BASE && *method == Method::GET {
        return Some(json_response(
            200,
            match guest {
                None => json!({ "exists": false, "vmid": OWNER_CONTAINER_VMID }),
                Some(g) => json!({
                    "exists": true,
                    "vmid": OWNER_CONTAINER_VMID,
                    "name": g["name"],
                    "type": g["type"],
                    "status": g["status"],
                    "cpuCores": g["cpuCores"],
                    "memoryMb": g["memoryMb"],
                    "diskGb": g["diskGb"],
                    "uptime": g["uptime"],
                    "desktopConfigured": vnc_target().is_some(),
                }),
            },
        ));
    }

    let Some(guest) = guest else {
        return Some(json_response(
            404,
            json!({ "error": "Container does not exist in Proxmox." }),
        ));
    };
    let record = record_for(&guest);

    if path == format!("{BASE}/power") && *method == Method::POST {
        return Some(power(&record, body).await);
    }
    if path == format!("{BASE}/console") && *method == Method::POST {
        return Some(
            console(
                state,
                &actor.email,
                &actor.sid,
                &actor.auth_session_key,
                &record,
                &guest,
                jsval::str_or(body.get("mode"), "desktop") == "text",
            )
            .await,
        );
    }
    Some(not_found())
}

async fn power(record: &Value, body: &Value) -> axum::response::Response {
    let action = jsval::str_or(body.get("action"), "");
    match desktop().power(record, &action, true).await {
        Ok(_) => {
            // A start or restart begins a fresh ten minutes.
            note_activity();
            json_response(202, json!({ "ok": true }))
        }
        Err(e) => proxmox_error(e),
    }
}

#[allow(clippy::too_many_arguments)]
async fn console(
    state: &Arc<AppState>,
    email: &str,
    sid: &str,
    auth_session_key: &str,
    record: &Value,
    guest: &Value,
    force_text: bool,
) -> axum::response::Response {
    if jsval::str_or(guest.get("status"), "") != "running" {
        return json_response(409, json!({ "error": "The container is not running." }));
    }
    note_activity();
    crate::routes::vm::cleanup_vm_desktop_sessions(state);
    // Desktop mode: no Proxmox console is minted; the bridge dials the
    // container's VNC server itself.
    let target = if force_text { None } else { vnc_target() };
    let session = match &target {
        Some(_) => json!({}),
        None => match desktop().create_console_for(record, true).await {
            Ok(s) => s,
            Err(e) => return proxmox_error(e),
        },
    };
    let session_id = {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mitch_lib::crypto::random_bytes(24))
    };
    let expires =
        (crate::routes::vm::now_ms() as f64) + crate::routes::vm::VM_DESKTOP_SESSION_TTL_MS;
    state
        .vm_desktop_sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            session_id.clone(),
            json!({
                // `ownerContainer` switches the WebSocket bridge to the
                // owner-only authorization path (no virtual_machines row).
                "ownerContainer": true,
                "vncTcp": target.is_some(),
                "actorEmail": email,
                "sid": sid,
                "authSessionKey": auth_session_key,
                "recordId": "",
                "ownerEmail": email,
                "vmid": OWNER_CONTAINER_VMID,
                "node": record["node"],
                "wsUrl": session.get("wsUrl"),
                "authorization": session.get("authorization"),
                "tlsOptions": session.get("tlsOptions"),
                "expiresAt": expires,
                "used": false,
            }),
        );
    json_response(
        201,
        json!({
            "socketPath": format!(
                "/api/vm/desktop/ws?session={}",
                mitch_lib::auth::encode_uri_component(&session_id)
            ),
            "credentials": {
                "password": match &target {
                    Some(t) => t.password.clone(),
                    None => jsval::str_or(session.get("ticket"), ""),
                }
            },
            "expiresIn": (crate::routes::vm::VM_DESKTOP_SESSION_TTL_MS / 1000.0).floor() as i64,
        }),
    )
}

/// Relays an owner-container desktop session between the browser's WebSocket
/// and the container's VNC server (raw RFB over TCP, which is what noVNC
/// speaks inside WebSocket binary frames). Re-checks authorization every
/// second like the Proxmox bridge, and closes when either side ends.
pub(crate) async fn bridge_vnc_tcp(
    state: Arc<AppState>,
    client_ws: axum::extract::ws::WebSocket,
    data: Value,
    mut close_rx: tokio::sync::mpsc::UnboundedReceiver<(u16, String)>,
) {
    use axum::extract::ws::{CloseFrame, Message};
    use futures_util::{SinkExt, StreamExt};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (mut client_sink, mut client_stream) = client_ws.split();
    let fail = |reason: &'static str| CloseFrame {
        code: 1011,
        reason: reason.into(),
    };
    let Some(target) = vnc_target() else {
        let _ = client_sink
            .send(Message::Close(Some(fail("Desktop is not configured"))))
            .await;
        return;
    };
    let connect = tokio::net::TcpStream::connect((target.host.as_str(), target.port));
    let tcp = match tokio::time::timeout(std::time::Duration::from_secs(8), connect).await {
        Ok(Ok(t)) => t,
        _ => {
            let _ = client_sink
                .send(Message::Close(Some(fail("Desktop connection failed"))))
                .await;
            return;
        }
    };
    let _ = tcp.set_nodelay(true);
    // Probe an idle connection every 15s so a firewall/NAT between the
    // webserver and the container can't silently expire it after a few minutes.
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(std::time::Duration::from_secs(30))
        .with_interval(std::time::Duration::from_secs(15));
    let _ = socket2::SockRef::from(&tcp).set_tcp_keepalive(&keepalive);
    let (mut tcp_read, mut tcp_write) = tcp.into_split();
    let mut buf = vec![0u8; 16 * 1024];
    let mut auth_check = tokio::time::interval(std::time::Duration::from_secs(1));

    loop {
        tokio::select! {
            _ = auth_check.tick() => {
                if !crate::routes::vm::vm_desktop_socket_authorized(&state, &data) {
                    let _ = client_sink.send(Message::Close(Some(CloseFrame {
                        code: 1008,
                        reason: "Desktop access expired".into(),
                    }))).await;
                    break;
                }
            }
            Some((code, reason)) = close_rx.recv() => {
                let _ = client_sink.send(Message::Close(Some(CloseFrame {
                    code,
                    reason: reason.into(),
                }))).await;
                break;
            }
            read = tcp_read.read(&mut buf) => {
                match read {
                    Ok(0) | Err(_) => {
                        // The reason goes only to the owner's browser (it is
                        // not logged): it says which side ended the session.
                        let reason = match &read {
                            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => "VNC server connection timed out",
                            Err(_) => "VNC server connection was reset",
                            _ => "VNC server closed the connection",
                        };
                        let _ = client_sink.send(Message::Close(Some(CloseFrame {
                            code: 1012,
                            reason: reason.into(),
                        }))).await;
                        break;
                    }
                    Ok(n) => {
                        if client_sink
                            .send(Message::Binary(buf[..n].to_vec().into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            msg = client_stream.next() => {
                match msg {
                    Some(Ok(Message::Binary(bin))) => {
                        if tcp_write.write_all(&bin).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(p))) => {
                        if client_sink.send(Message::Pong(p)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Text(_) | Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                }
            }
        }
    }
}

#[cfg(test)]
mod idle_tests {
    use super::*;

    #[test]
    fn shuts_down_only_a_running_container_idle_for_ten_minutes() {
        let now = 100 * 60 * 1000;
        // Newly seen running: the clock starts now, no shutdown.
        assert_eq!(idle_decision(true, 0, now), (now, false));
        // Active recently.
        let recent = now - IDLE_LIMIT_MS + 1;
        assert_eq!(idle_decision(true, recent, now), (recent, false));
        // Idle long enough: shut down, and the clock restarts.
        assert_eq!(idle_decision(true, now - IDLE_LIMIT_MS, now), (now, true));
        // A stopped container is never shut down and clears the clock, so a
        // later start doesn't inherit an old timestamp.
        assert_eq!(
            idle_decision(false, now - 2 * IDLE_LIMIT_MS, now),
            (0, false)
        );
    }
}
