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

use std::sync::Arc;

use axum::http::{HeaderMap, Method};
use serde_json::{json, Value};

use super::{json_response, Resp};
use crate::proxmox_desktop::desktop;
use crate::state::AppState;
use mitch_lib::jsval;
use mitch_lib::vm::OWNER_CONTAINER_VMID;

const BASE: &str = "/api/admin/owner-container";

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
            )
            .await,
        );
    }
    Some(not_found())
}

async fn power(record: &Value, body: &Value) -> axum::response::Response {
    let action = jsval::str_or(body.get("action"), "");
    match desktop().power(record, &action, true).await {
        Ok(_) => json_response(202, json!({ "ok": true })),
        Err(e) => proxmox_error(e),
    }
}

async fn console(
    state: &Arc<AppState>,
    email: &str,
    sid: &str,
    auth_session_key: &str,
    record: &Value,
    guest: &Value,
) -> axum::response::Response {
    if jsval::str_or(guest.get("status"), "") != "running" {
        return json_response(409, json!({ "error": "The container is not running." }));
    }
    crate::routes::vm::cleanup_vm_desktop_sessions(state);
    let session = match desktop().create_console_for(record, true).await {
        Ok(s) => s,
        Err(e) => return proxmox_error(e),
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
            "credentials": { "password": jsval::str_or(session.get("ticket"), "") },
            "expiresIn": (crate::routes::vm::VM_DESKTOP_SESSION_TTL_MS / 1000.0).floor() as i64,
        }),
    )
}
