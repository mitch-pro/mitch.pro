//! Admin VM management — vm-requests, deny-vm, terminate-vm, restore-vm,
//! approve-vm, lxc-attach-sshd-hook, plus the full Proxmox desktop VM fleet
//! administration endpoints (server.js:20618-20875):
//! - GET /api/admin/vms/overview
//! - GET /api/admin/vms/stats
//! - POST /api/admin/vms/assign
//! - POST /api/admin/vms/unassign
//! - POST /api/admin/vms/create
//! - POST /api/admin/vms/credentials
//! - POST /api/admin/vms/delete

use super::{AdminCtx, Resp};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::{auth, jsval, profile, vm as vmlib};
use serde_json::{json, Value};
use std::sync::Arc;

const PVE_VMID_MIN: i64 = 100;
const PVE_VMID_MAX: i64 = 999999999;

fn vm_credentials_key(id_secret: &[u8]) -> [u8; 32] {
    mitch_lib::crypto::hmac_sha256(id_secret, b"vm-desktop-credentials-v1")
}

pub fn encrypt_vm_password(id_secret: &[u8], password: &str) -> String {
    use aes_gcm::aead::generic_array::GenericArray;
    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes256Gcm, KeyInit};
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;

    let key = vm_credentials_key(id_secret);
    let cipher = match Aes256Gcm::new_from_slice(&key) {
        Ok(c) => c,
        Err(_) => return String::new(),
    };
    let iv = mitch_lib::crypto::random_bytes(12);
    let nonce = GenericArray::from_slice(&iv);
    let ct_and_tag = match cipher.encrypt(nonce, password.as_bytes()) {
        Ok(ct) => ct,
        Err(_) => return String::new(),
    };
    if ct_and_tag.len() < 16 {
        return String::new();
    }
    let ct = &ct_and_tag[..ct_and_tag.len() - 16];
    let tag = &ct_and_tag[ct_and_tag.len() - 16..];
    format!(
        "{}.{}.{}",
        URL_SAFE_NO_PAD.encode(&iv),
        URL_SAFE_NO_PAD.encode(tag),
        URL_SAFE_NO_PAD.encode(ct)
    )
}

pub fn decrypt_vm_password(id_secret: &[u8], value: &str) -> String {
    use aes_gcm::aead::generic_array::GenericArray;
    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes256Gcm, KeyInit};
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;

    let parts: Vec<&str> = value.split('.').collect();
    if parts.len() != 3 {
        return String::new();
    }
    let iv = match URL_SAFE_NO_PAD.decode(parts[0]) {
        Ok(b) if b.len() == 12 => b,
        _ => return String::new(),
    };
    let tag = match URL_SAFE_NO_PAD.decode(parts[1]) {
        Ok(b) if b.len() == 16 => b,
        _ => return String::new(),
    };
    let ct = match URL_SAFE_NO_PAD.decode(parts[2]) {
        Ok(b) => b,
        _ => return String::new(),
    };

    let key = vm_credentials_key(id_secret);
    let cipher = match Aes256Gcm::new_from_slice(&key) {
        Ok(c) => c,
        Err(_) => return String::new(),
    };
    let nonce = GenericArray::from_slice(&iv);
    let mut combined = ct;
    combined.extend_from_slice(&tag);
    match cipher.decrypt(nonce, combined.as_ref()) {
        Ok(plain) => String::from_utf8(plain).unwrap_or_default(),
        Err(_) => String::new(),
    }
}

pub fn store_vm_desktop_credentials(
    state: &AppState,
    record_id: &str,
    owner_email: &str,
    username: &str,
    password: &str,
) {
    if record_id.is_empty() || password.is_empty() {
        return;
    }
    let file = state.data_dir().join("vm_credentials.json");
    let mut creds = state.store.read_document(&file, json!({}));
    let enc_pw = encrypt_vm_password(&state.id_secret, password);
    let entry = json!({
        "ownerEmail": auth::normalize_email(owner_email),
        "username": username,
        "password": enc_pw,
        "updatedAt": now_millis(),
    });
    if let Some(map) = creds.as_object_mut() {
        map.insert(record_id.to_string(), entry);
    } else {
        creds = json!({ record_id: entry });
    }
    let _ = state.store.write_document(&file, &creds);
}

pub fn owner_vm_desktop_credentials(state: &AppState, record_id: &str) -> Value {
    let file = state.data_dir().join("vm_credentials.json");
    let creds = state.store.read_document(&file, json!({}));
    let entry = creds.get(record_id);
    let Some(entry) = entry else {
        return Value::Null;
    };
    let enc_pw = entry.get("password").and_then(Value::as_str).unwrap_or("");
    let plain = decrypt_vm_password(&state.id_secret, enc_pw);
    if plain.is_empty() {
        return Value::Null;
    }
    json!({
        "username": entry.get("username").and_then(Value::as_str).unwrap_or(""),
        "password": plain,
        "updatedAt": entry.get("updatedAt").and_then(jsval::number).unwrap_or(0.0),
    })
}

pub fn remove_vm_desktop_credentials(state: &AppState, record_id: &str) {
    if record_id.is_empty() {
        return;
    }
    let file = state.data_dir().join("vm_credentials.json");
    let mut creds = state.store.read_document(&file, json!({}));
    if let Some(map) = creds.as_object_mut() {
        if map.remove(record_id).is_some() {
            let _ = state.store.write_document(&file, &creds);
        }
    }
}

pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    search: &str,
    body: &Value,
    ctx: &AdminCtx,
) -> Resp {
    // Origin check for /api/admin/vms/* POST
    if path.starts_with("/api/admin/vms/") && *method == Method::POST && !crate::routes::vm::vm_same_origin_request(headers) {
        return Some(json_response(403, json!({ "error": "Request origin rejected." })));
    }

    // GET /api/admin/vms/overview (server.js:20620-20691)
    if path == "/api/admin/vms/overview" && *method == Method::GET {
        if let Some((code, msg)) = state.rate_limit_check(&ctx.ip, "anon", "/api/admin/vms") {
            return Some(json_response(code, json!({ "error": msg })));
        }
        let actor = match crate::routes::vm::authenticated_vm_actor(state, headers) {
            Some(a) => a,
            None => return Some(json_response(401, json!({ "error": "Sign in required." }))),
        };
        if !actor.is_admin {
            return Some(json_response(403, json!({ "error": "Admin access required." })));
        }
        let is_owner = auth::is_owner_email(&state.store, &actor.email);

        let records = vmlib::list_virtual_machines(&state.store, true);
        let pve = crate::proxmox_desktop::desktop();
        let (guests, capacity, service_error) = match tokio::join!(pve.list_guests(), pve.node_capacity()) {
            (Ok(g), Ok(c)) => (g, json!(c), String::new()),
            (Err(e), _) => (Vec::new(), Value::Null, e.message),
            (_, Err(e)) => (Vec::new(), Value::Null, e.message),
        };

        let now = now_millis() as f64;
        let mut active_sessions: Vec<Value> = Vec::new();
        {
            let sockets = state
                .vm_desktop_sockets
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            for client in sockets.values() {
                let d = &client.data;
                let record_id = jsval::str_or(d.get("recordId"), "");
                if record_id.is_empty() {
                    continue;
                }
                let connected_at = d.get("connectedAt").and_then(jsval::number).unwrap_or(now);
                active_sessions.push(json!({
                    "recordId": record_id,
                    "vmid": d.get("vmid"),
                    "actorEmail": d.get("actorEmail"),
                    "ownerEmail": d.get("ownerEmail"),
                    "connectedAt": connected_at,
                    "durationSeconds": ((now - connected_at) / 1000.0).max(0.0).floor() as i64,
                }));
            }
        }

        let mut record_views: Vec<Value> = Vec::with_capacity(records.len());
        for rec in &records {
            let record = rec.to_json();
            let record_id = jsval::str_or(record.get("id"), "");
            let runtime = match pve.get_status(&record).await {
                Ok(rt) => rt,
                Err(_) => json!({ "state": "unavailable" }),
            };
            let active_users: Vec<Value> = active_sessions
                .iter()
                .filter(|s| s.get("recordId").and_then(Value::as_str) == Some(&record_id))
                .cloned()
                .collect();
            let mut view = crate::routes::vm::public_vm_record(
                state,
                &record,
                Some(&runtime),
                Some(&actor.to_json()),
            );
            if let Some(map) = view.as_object_mut() {
                map.insert("ownerEmail".into(), record.get("ownerEmail").cloned().unwrap_or(Value::Null));
                map.insert("vmid".into(), record.get("vmid").cloned().unwrap_or(Value::Null));
                map.insert("node".into(), record.get("node").cloned().unwrap_or(Value::Null));
                map.insert("guestType".into(), record.get("guestType").cloned().unwrap_or(Value::Null));
                map.insert("assignmentStatus".into(), record.get("status").cloned().unwrap_or(Value::Null));
                map.insert("activeUsers".into(), json!(active_users));
                map.insert("isCurrentlyInUse".into(), json!(!active_users.is_empty()));
                map.insert("canAccess".into(), json!(crate::routes::vm::vm_record_allowed_for_actor(state, &record, &actor.to_json())));
                if is_owner && jsval::str_or(record.get("status"), "") != "unassigned" {
                    let creds = owner_vm_desktop_credentials(state, &record_id);
                    if !creds.is_null() {
                        map.insert("desktopCredentials".into(), creds);
                    }
                }
            }
            record_views.push(view);
        }

        let passwords = state.store.read_document(&state.data_dir().join("passwords.json"), json!({}));
        let profiles = state.store.read_document(&state.data_dir().join("profiles.json"), json!({}));
        let mut user_emails: Vec<String> = passwords
            .as_object()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        user_emails.sort();
        let users: Vec<Value> = user_emails
            .into_iter()
            .map(|email| {
                let norm = auth::normalize_email(&email);
                let prof = profiles.get(norm.as_str());
                let name = prof
                    .and_then(|p| p.get("displayName").or_else(|| p.get("nickname")))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| email.split('@').next().unwrap_or("student").to_string());
                json!({ "email": email, "name": name })
            })
            .collect();

        let running_resources = crate::routes::vm::get_running_non_admin_vm_resources(state).await;
        let running_non_admin_count = running_resources.get("count").and_then(jsval::number).unwrap_or(0.0) as i64;
        let running_non_admin_cores = running_resources.get("cores").and_then(jsval::number).unwrap_or(0.0) as i64;
        let running_non_admin_memory_mb = running_resources.get("memoryMb").and_then(jsval::number).unwrap_or(0.0) as i64;

        let query_map = crate::handler::query(search);
        let day_key = query_map.get("day").cloned().unwrap_or_default();
        let usage_stats = crate::routes::vm::get_vm_fleet_usage_stats(state, &day_key);

        let assigned_vmids: std::collections::HashSet<i64> = records
            .iter()
            .filter(|r| r.status != "unassigned")
            .map(|r| r.vmid as i64)
            .collect();
        let template_vmids = pve.template_vmids();
        let available_guests: Vec<Value> = guests
            .iter()
            .filter(|guest| {
                !guest.get("template").and_then(Value::as_bool).unwrap_or(false)
                    && guest.get("type").and_then(Value::as_str) == Some("qemu")
                    && guest.get("vmid").and_then(Value::as_i64).map_or(false, |id| !assigned_vmids.contains(&id))
            })
            .cloned()
            .collect();
        let templates: Vec<Value> = guests
            .iter()
            .filter(|guest| {
                guest.get("template").and_then(Value::as_bool).unwrap_or(false)
                    && guest.get("type").and_then(Value::as_str) == Some("qemu")
                    && guest.get("vmid").and_then(Value::as_i64).map_or(false, |id| template_vmids.contains(&id))
            })
            .cloned()
            .collect();

        let audit_rows = vmlib::list_vm_audit_logs(&state.store, 100.0);
        let audit: Vec<Value> = audit_rows.into_iter().map(|r| r.to_json()).collect();

        return Some(json_response(200, json!({
            "computers": record_views,
            "availableGuests": available_guests,
            "templates": templates,
            "users": users,
            "capacity": capacity,
            "serviceAvailable": pve.configured() && service_error.is_empty(),
            "serviceError": service_error,
            "audit": audit,
            "activeSessions": active_sessions,
            "runningNonAdminCount": running_non_admin_count,
            "runningNonAdminCores": running_non_admin_cores,
            "runningNonAdminMemoryMb": running_non_admin_memory_mb,
            "maxFleetCores": mitch_lib::vm_security::VM_FLEET_MAX_CORES,
            "maxFleetMemoryMb": mitch_lib::vm_security::VM_FLEET_MAX_MEMORY_MB,
            "maxRunningNonAdminLimit": mitch_lib::vm_security::VM_MAX_CONCURRENT_RUNNING,
            "usageStats": usage_stats,
            "viewerIsOwner": is_owner,
        })));
    }

    // GET /api/admin/vms/stats (server.js:20693-20701)
    if path == "/api/admin/vms/stats" && *method == Method::GET {
        if let Some((code, msg)) = state.rate_limit_check(&ctx.ip, "anon", "/api/admin/vms") {
            return Some(json_response(code, json!({ "error": msg })));
        }
        let actor = match crate::routes::vm::authenticated_vm_actor(state, headers) {
            Some(a) => a,
            None => return Some(json_response(401, json!({ "error": "Sign in required." }))),
        };
        if !actor.is_admin {
            return Some(json_response(403, json!({ "error": "Admin access required." })));
        }
        let query_map = crate::handler::query(search);
        let day_key = query_map.get("day").cloned().unwrap_or_default();
        let usage_stats = crate::routes::vm::get_vm_fleet_usage_stats(state, &day_key);
        return Some(json_response(200, json!({ "ok": true, "usageStats": usage_stats })));
    }

    // POST /api/admin/vms/assign (server.js:20703-20735)
    if path == "/api/admin/vms/assign" && *method == Method::POST {
        if let Some((code, msg)) = state.rate_limit_check(&ctx.ip, "anon", "/api/admin/vms") {
            return Some(json_response(code, json!({ "error": msg })));
        }
        let actor = match crate::routes::vm::authenticated_vm_actor(state, headers) {
            Some(a) => a,
            None => return Some(json_response(401, json!({ "error": "Sign in required." }))),
        };
        if !actor.is_admin {
            return Some(json_response(403, json!({ "error": "Admin access required." })));
        }

        let raw_email = jsval::str_or(body.get("ownerEmail"), "").trim().to_string();
        let resolved = profile::resolve_target_email(&state.store, state.data_dir(), &state.id_secret, &raw_email)
            .unwrap_or(raw_email);
        let owner_email = auth::normalize_email(&resolved);
        let passwords = state.store.read_document(&state.data_dir().join("passwords.json"), json!({}));
        if owner_email.is_empty() || !passwords.get(&owner_email).is_some() {
            return Some(json_response(400, json!({ "error": "Choose a valid website user." })));
        }

        let vmid = body.get("vmid").and_then(jsval::number).unwrap_or(0.0);
        if !crate::routes::vm::is_vm_id_in_range(vmid) {
            return Some(json_response(400, json!({ "error": "Choose a valid computer." })));
        }

        let record = vmlib::get_virtual_machine_by_vmid(&state.store, vmid);
        if let Some(ref rec) = record {
            if rec.status != "unassigned" && auth::normalize_email(&rec.owner_email) != owner_email {
                return Some(json_response(409, json!({ "error": "That computer is already assigned." })));
            }
        }

        let guests = match crate::proxmox_desktop::desktop().list_guests().await {
            Ok(g) => g,
            Err(e) => {
                let friendly = crate::routes::vm::friendly_vm_error(&e.into());
                return Some(json_response(friendly.0, json!({ "error": friendly.1, "code": friendly.2 })));
            }
        };
        let guest = guests.iter().find(|item| {
            item.get("vmid").and_then(jsval::number) == Some(vmid)
                && item.get("type").and_then(Value::as_str) == Some("qemu")
                && !item.get("template").and_then(Value::as_bool).unwrap_or(false)
        });
        let Some(guest) = guest else {
            return Some(json_response(404, json!({ "error": "Computer not found or cannot be assigned." })));
        };

        let node = jsval::str_or(guest.get("node"), crate::proxmox_desktop::desktop().node());
        let guest_qemu = json!({
            "vmid": vmid,
            "node": node,
            "guestType": "qemu",
        });
        let config = crate::proxmox_desktop::desktop().get_config(&guest_qemu).await.unwrap_or(json!({}));

        let cores = config.get("cores").and_then(jsval::number)
            .or_else(|| guest.get("cpuCores").and_then(jsval::number))
            .unwrap_or(4.0);
        let memory = config.get("memory").and_then(jsval::number)
            .or_else(|| guest.get("memoryMb").and_then(jsval::number))
            .unwrap_or(16384.0);
        let disk = guest.get("diskGb").and_then(jsval::number).unwrap_or(64.0);

        let friendly_name = jsval::str_or(body.get("friendlyName"), "My Computer").trim().to_string();
        let friendly_name = if friendly_name.is_empty() { "My Computer" } else { &friendly_name[..friendly_name.len().min(60)] };
        let os_name = jsval::str_or(body.get("operatingSystem"), "Linux Mint Cinnamon").trim().to_string();
        let os_name = if os_name.is_empty() { "Linux Mint Cinnamon" } else { &os_name[..os_name.len().min(80)] };

        let rec_id = record.as_ref().map(|r| r.id.clone()).unwrap_or_else(|| format!("vm-{}", vmid as i64));
        let created_at = record.as_ref().map(|r| r.created_at).unwrap_or_else(|| now_millis() as f64);
        let hostname = jsval::str_or(guest.get("name"), &rec_id);

        let new_rec = vmlib::upsert_virtual_machine(
            &state.store,
            &json!({
                "id": rec_id,
                "ownerEmail": owner_email,
                "ownerUserId": profile::get_uid_for_email(&state.store, &state.id_secret, &owner_email).unwrap_or_default(),
                "vmid": vmid,
                "node": node,
                "guestType": "qemu",
                "friendlyName": friendly_name,
                "hostname": hostname,
                "operatingSystem": os_name,
                "templateVmid": Value::Null,
                "cpuCores": cores,
                "memoryMb": memory,
                "diskGb": disk,
                "status": "assigned",
                "createdAt": created_at,
            }),
        );
        let Some(new_rec) = new_rec else {
            return Some(json_response(500, json!({ "error": "Failed to assign computer." })));
        };

        crate::routes::vm::vm_audit(
            state,
            &actor.email,
            Some(&new_rec.to_json()),
            "VM_ASSIGNED",
            true,
            None,
        );

        let mut comp = crate::routes::vm::public_vm_record(
            state,
            &new_rec.to_json(),
            None,
            Some(&actor.to_json()),
        );
        if let Some(map) = comp.as_object_mut() {
            map.insert("ownerEmail".into(), json!(new_rec.owner_email));
            map.insert("vmid".into(), json!(new_rec.vmid));
        }
        return Some(json_response(201, json!({ "success": true, "computer": comp })));
    }

    // POST /api/admin/vms/unassign (server.js:20737-20752)
    if path == "/api/admin/vms/unassign" && *method == Method::POST {
        if let Some((code, msg)) = state.rate_limit_check(&ctx.ip, "anon", "/api/admin/vms") {
            return Some(json_response(code, json!({ "error": msg })));
        }
        let actor = match crate::routes::vm::authenticated_vm_actor(state, headers) {
            Some(a) => a,
            None => return Some(json_response(401, json!({ "error": "Sign in required." }))),
        };
        if !actor.is_admin {
            return Some(json_response(403, json!({ "error": "Admin access required." })));
        }

        let id = jsval::str_or(body.get("id"), "");
        let record = vmlib::get_virtual_machine_by_id(&state.store, &id);
        let Some(record) = record else {
            return Some(json_response(404, json!({ "error": "Computer not found." })));
        };

        let success = vmlib::unassign_virtual_machine(&state.store, &record.id);
        if success {
            crate::routes::vm::revoke_vm_desktop_connections(state, &record.id);
            remove_vm_desktop_credentials(state, &record.id);
        }
        crate::routes::vm::vm_audit(
            state,
            &actor.email,
            Some(&record.to_json()),
            "VM_UNASSIGNED",
            success,
            None,
        );
        return Some(json_response(200, json!({ "success": success })));
    }

    // POST /api/admin/vms/create (server.js:20754-20800)
    if path == "/api/admin/vms/create" && *method == Method::POST {
        if let Some((code, msg)) = state.rate_limit_check(&ctx.ip, "anon", "/api/admin/vms") {
            return Some(json_response(code, json!({ "error": msg })));
        }
        let actor = match crate::routes::vm::authenticated_vm_actor(state, headers) {
            Some(a) => a,
            None => return Some(json_response(401, json!({ "error": "Sign in required." }))),
        };
        if !actor.is_admin {
            return Some(json_response(403, json!({ "error": "Admin access required." })));
        }

        let raw_email = jsval::str_or(body.get("ownerEmail"), "").trim().to_string();
        let resolved = profile::resolve_target_email(&state.store, state.data_dir(), &state.id_secret, &raw_email)
            .unwrap_or(raw_email);
        let owner_email = auth::normalize_email(&resolved);
        let passwords = state.store.read_document(&state.data_dir().join("passwords.json"), json!({}));
        if owner_email.is_empty() || !passwords.get(&owner_email).is_some() {
            return Some(json_response(400, json!({ "error": "Choose a valid website user." })));
        }

        {
            let mut requests = state
                .vm_power_requests
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if requests.contains_key("admin-create") {
                return Some(json_response(409, json!({ "error": "Another computer is currently being created." })));
            }
            requests.insert("admin-create".to_string(), json!({ "startedAt": now_millis() }));
        }

        let default_tpl = crate::proxmox_desktop::desktop()
            .template_vmids()
            .first()
            .copied()
            .unwrap_or(9010);
        let template_vmid = body.get("templateVmid").and_then(jsval::number).unwrap_or(default_tpl as f64) as i64;
        if !crate::proxmox_desktop::desktop().template_vmids().contains(&template_vmid) {
            state.vm_power_requests.lock().unwrap_or_else(|e| e.into_inner()).remove("admin-create");
            return Some(json_response(400, json!({ "error": "Choose an available desktop template." })));
        }

        let desktop_username = jsval::str_or(body.get("desktopUsername"), "");
        let desktop_password = jsval::str_or(body.get("desktopPassword"), "");
        let (login_username, login_password) = match crate::proxmox_desktop::desktop().validate_desktop_login(&desktop_username, &desktop_password) {
            Ok(l) => l,
            Err(_) => {
                state.vm_power_requests.lock().unwrap_or_else(|e| e.into_inner()).remove("admin-create");
                return Some(json_response(400, json!({
                    "error": "Choose a desktop username and a non-empty password without line breaks.",
                    "code": "invalid_desktop_login"
                })));
            }
        };

        let cpu_cores = body.get("cpuCores").and_then(jsval::number).unwrap_or(6.0).clamp(2.0, 16.0).round();
        let memory_mb = body.get("memoryMb").and_then(jsval::number).unwrap_or(16384.0).clamp(2048.0, 65536.0).round();
        let disk_gb = body.get("diskGb").and_then(jsval::number).unwrap_or(64.0).clamp(40.0, 256.0).round();

        let default_host = format!("computer-{}", owner_email.split('@').next().unwrap_or("student"));
        let hostname = jsval::str_or(body.get("hostname"), &default_host).trim().to_string();
        let friendly_name = jsval::str_or(body.get("friendlyName"), "My Computer").trim().to_string();
        let friendly_name = if friendly_name.is_empty() { "My Computer" } else { &friendly_name[..friendly_name.len().min(60)] };
        let os_name = jsval::str_or(body.get("operatingSystem"), "Ubuntu Desktop LTS").trim().to_string();
        let os_name = if os_name.is_empty() { "Ubuntu Desktop LTS" } else { &os_name[..os_name.len().min(80)] };

        let all_records = vmlib::list_virtual_machines(&state.store, true);
        let existing_vmids: Vec<f64> = all_records.iter().map(|r| r.vmid).collect();
        let vmid = match crate::proxmox_desktop::desktop().next_available_vmid(PVE_VMID_MIN as f64, PVE_VMID_MAX as f64, &existing_vmids).await {
            Ok(v) => v as f64,
            Err(e) => {
                state.vm_power_requests.lock().unwrap_or_else(|e| e.into_inner()).remove("admin-create");
                let friendly = crate::routes::vm::friendly_vm_error(&e.into());
                return Some(json_response(friendly.0, json!({ "error": friendly.1, "code": friendly.2 })));
            }
        };

        let pending_record = vmlib::reserve_virtual_machine(
            &state.store,
            &json!({
                "id": format!("vm-{}", vmid as i64),
                "ownerEmail": owner_email,
                "ownerUserId": profile::get_uid_for_email(&state.store, &state.id_secret, &owner_email).unwrap_or_default(),
                "vmid": vmid,
                "node": crate::proxmox_desktop::desktop().node(),
                "guestType": "qemu",
                "friendlyName": friendly_name,
                "hostname": hostname,
                "operatingSystem": os_name,
                "templateVmid": template_vmid,
                "cpuCores": cpu_cores,
                "memoryMb": memory_mb,
                "diskGb": disk_gb,
                "status": "provisioning",
                "createdAt": now_millis(),
            }),
        );
        let Some(pending_record) = pending_record else {
            state.vm_power_requests.lock().unwrap_or_else(|e| e.into_inner()).remove("admin-create");
            return Some(json_response(409, json!({ "error": "Another computer is being created. Please try again." })));
        };

        let clone_params = crate::proxmox_desktop::CloneDesktopParams {
            template_vmid: Some(template_vmid as f64),
            vmid: Some(vmid),
            hostname: hostname.clone(),
            cpu_cores: Some(cpu_cores),
            memory_mb: Some(memory_mb),
            disk_gb: Some(disk_gb),
            desktop_username: login_username.clone(),
            desktop_password: login_password.clone(),
        };
        let clone_res = crate::proxmox_desktop::desktop().clone_desktop(&clone_params).await;
        state.vm_power_requests.lock().unwrap_or_else(|e| e.into_inner()).remove("admin-create");

        match clone_res {
            Ok(created) => {
                let mut merged = pending_record.to_json();
                if let Some(map) = merged.as_object_mut() {
                    if let Some(c_map) = created.as_object() {
                        for (k, v) in c_map {
                            map.insert(k.clone(), v.clone());
                        }
                    }
                    map.insert("status".into(), json!("assigned"));
                }
                let record = vmlib::upsert_virtual_machine(&state.store, &merged);
                let Some(record) = record else {
                    return Some(json_response(500, json!({ "error": "Failed to save created computer record." })));
                };
                store_vm_desktop_credentials(state, &record.id, &record.owner_email, &login_username, &login_password);
                crate::routes::vm::vm_audit(
                    state,
                    &actor.email,
                    Some(&record.to_json()),
                    "VM_CREATED",
                    true,
                    Some(&json!({ "templateVmid": template_vmid })),
                );
                crate::routes::vm::vm_audit(
                    state,
                    &actor.email,
                    Some(&record.to_json()),
                    "VM_ASSIGNED",
                    true,
                    None,
                );
                let mut comp = crate::routes::vm::public_vm_record(
                    state,
                    &record.to_json(),
                    Some(&json!({ "state": "starting" })),
                    Some(&actor.to_json()),
                );
                if let Some(map) = comp.as_object_mut() {
                    map.insert("ownerEmail".into(), json!(record.owner_email));
                    map.insert("vmid".into(), json!(record.vmid));
                }
                return Some(json_response(201, json!({ "success": true, "computer": comp })));
            }
            Err(e) => {
                let _ = vmlib::update_virtual_machine_runtime(
                    &state.store,
                    &pending_record.id,
                    None,
                    Some("provisioning-failed"),
                );
                crate::routes::vm::vm_audit(
                    state,
                    &actor.email,
                    Some(&pending_record.to_json()),
                    "VM_CREATED",
                    false,
                    Some(&json!({ "code": e.code })),
                );
                let friendly = crate::routes::vm::friendly_vm_error(&e.into());
                return Some(json_response(friendly.0, json!({ "error": friendly.1, "code": friendly.2 })));
            }
        }
    }

    // POST /api/admin/vms/credentials (server.js:20802-20823)
    if path == "/api/admin/vms/credentials" && *method == Method::POST {
        if let Some((code, msg)) = state.rate_limit_check(&ctx.ip, "anon", "/api/admin/vms") {
            return Some(json_response(code, json!({ "error": msg })));
        }
        let actor = match crate::routes::vm::authenticated_vm_actor(state, headers) {
            Some(a) => a,
            None => return Some(json_response(401, json!({ "error": "Sign in required." }))),
        };
        let is_owner = auth::is_owner_email(&state.store, &actor.email);
        if !is_owner {
            return Some(json_response(403, json!({ "error": "Owner access required." })));
        }

        let id = jsval::str_or(body.get("id"), "");
        let record = vmlib::get_virtual_machine_by_id(&state.store, &id);
        let Some(record) = record else {
            return Some(json_response(404, json!({ "error": "Assigned computer not found." })));
        };
        if record.status == "unassigned" {
            return Some(json_response(404, json!({ "error": "Assigned computer not found." })));
        }

        let username = jsval::str_or(body.get("username"), "");
        let password = jsval::str_or(body.get("password"), "");
        let (login_username, login_password) = match crate::proxmox_desktop::desktop().validate_desktop_login(&username, &password) {
            Ok(l) => l,
            Err(_) => {
                return Some(json_response(400, json!({
                    "error": "Choose a desktop username and a non-empty password without line breaks.",
                    "code": "invalid_desktop_login"
                })));
            }
        };

        match crate::proxmox_desktop::desktop().enable_friendly_desktop_login(record.vmid as i64, &login_username, &login_password).await {
            Ok(_) => {
                store_vm_desktop_credentials(state, &record.id, &record.owner_email, &login_username, &login_password);
                crate::routes::vm::vm_audit(
                    state,
                    &actor.email,
                    Some(&record.to_json()),
                    "OWNER_VM_CREDENTIALS_RESET",
                    true,
                    Some(&json!({ "username": login_username })),
                );
                let creds = owner_vm_desktop_credentials(state, &record.id);
                return Some(json_response(200, json!({ "success": true, "credentials": creds })));
            }
            Err(err) => {
                crate::routes::vm::vm_audit(
                    state,
                    &actor.email,
                    Some(&record.to_json()),
                    "OWNER_VM_CREDENTIALS_RESET",
                    false,
                    Some(&json!({ "code": err.code })),
                );
                let friendly = crate::routes::vm::friendly_vm_error(&err.into());
                return Some(json_response(
                    friendly.0,
                    json!({
                        "error": format!("{} Start the computer first, then try again.", friendly.1),
                        "code": friendly.2
                    }),
                ));
            }
        }
    }

    // POST /api/admin/vms/delete (server.js:20825-20875)
    if path == "/api/admin/vms/delete" && *method == Method::POST {
        if let Some((code, msg)) = state.rate_limit_check(&ctx.ip, "anon", "/api/admin/vms") {
            return Some(json_response(code, json!({ "error": msg })));
        }
        let actor = match crate::routes::vm::authenticated_vm_actor(state, headers) {
            Some(a) => a,
            None => return Some(json_response(401, json!({ "error": "Sign in required." }))),
        };
        if !actor.is_admin {
            return Some(json_response(403, json!({ "error": "Admin access required." })));
        }

        let vmid_fallback = jsval::str_or(body.get("vmid"), "");
        let id_str = jsval::str_or(body.get("id"), &vmid_fallback);
        let record = vmlib::get_virtual_machine_by_id(&state.store, &id_str)
            .or_else(|| {
                id_str.parse::<f64>().ok().and_then(|v| vmlib::get_virtual_machine_by_vmid(&state.store, v))
            });
        let Some(record) = record else {
            return Some(json_response(404, json!({ "error": "Computer not found." })));
        };

        let is_force = body.get("force").and_then(Value::as_bool).unwrap_or(false);
        let rec_json = record.to_json();
        let del_res = crate::proxmox_desktop::desktop().delete_guest(&rec_json, is_force).await;
        let proxmox_error = match del_res {
            Ok(_) => None,
            Err(e) => Some(crate::routes::vm::friendly_vm_error(&e.into()).1),
        };

        if let Some(ref err_msg) = proxmox_error {
            if !is_force {
                crate::routes::vm::vm_audit(
                    state,
                    &actor.email,
                    Some(&rec_json),
                    "VM_DELETED",
                    false,
                    Some(&json!({ "error": err_msg })),
                );
                return Some(json_response(500, json!({
                    "error": err_msg,
                    "canForce": true,
                    "message": format!("{err_msg} You can click Force Delete to remove it anyway and ignore Proxmox errors.")
                })));
            }
        }

        vmlib::delete_virtual_machine(&state.store, &record.id);
        remove_vm_desktop_credentials(state, &record.id);
        crate::routes::vm::revoke_vm_desktop_connections(state, &record.id);

        // Clean up from vm_apps.json if present
        {
            let file = state.data_dir().join("vm_apps.json");
            let mut vm_apps = state.store.read_document(&file, json!({}));
            if let Some(map) = vm_apps.as_object_mut() {
                let mut to_remove = None;
                for (k, v) in map.iter() {
                    if v.get("vmid").and_then(jsval::number) == Some(record.vmid) {
                        to_remove = Some(k.clone());
                        break;
                    }
                }
                if let Some(k) = to_remove {
                    map.remove(&k);
                    let _ = state.store.write_document(&file, &vm_apps);
                }
            }
        }

        crate::routes::vm::vm_audit(
            state,
            &actor.email,
            Some(&rec_json),
            "VM_DELETED",
            true,
            Some(&json!({ "force": is_force, "proxmoxError": proxmox_error })),
        );

        let msg = if is_force && proxmox_error.is_some() {
            format!("Computer deleted from system (ignored Proxmox error: {})", proxmox_error.as_deref().unwrap_or(""))
        } else {
            "Computer deleted.".to_string()
        };
        return Some(json_response(200, json!({ "success": true, "message": msg })));
    }

    // POST /api/admin/vms/downgrade-specs — admin override of a computer's
    // cpu/memory/disk, independent of the user's own paid-upgrade path.
    // Reuses update_hardware, the same Proxmox resize call the paid upgrade
    // flow uses (routes/vm.rs's /api/vm/my-computer/upgrade handler) — cores
    // and memory move freely in either direction; disk only ever grows (a
    // live virtual disk can't be safely shrunk without guest cooperation,
    // so update_hardware already no-ops a smaller disk request rather than
    // risk data loss).
    if path == "/api/admin/vms/downgrade-specs" && *method == Method::POST {
        if let Some((code, msg)) = state.rate_limit_check(&ctx.ip, "anon", "/api/admin/vms") {
            return Some(json_response(code, json!({ "error": msg })));
        }
        let actor = match crate::routes::vm::authenticated_vm_actor(state, headers) {
            Some(a) => a,
            None => return Some(json_response(401, json!({ "error": "Sign in required." }))),
        };
        if !actor.is_admin {
            return Some(json_response(403, json!({ "error": "Admin access required." })));
        }

        let vmid_fallback = jsval::str_or(body.get("vmid"), "");
        let id_str = jsval::str_or(body.get("id"), &vmid_fallback);
        let record = vmlib::get_virtual_machine_by_id(&state.store, &id_str).or_else(|| {
            id_str
                .parse::<f64>()
                .ok()
                .and_then(|v| vmlib::get_virtual_machine_by_vmid(&state.store, v))
        });
        let Some(record) = record else {
            return Some(json_response(404, json!({ "error": "Computer not found." })));
        };

        let cpu_opt = body.get("cpuCores").and_then(jsval::number);
        let mem_opt = body.get("memoryMb").and_then(jsval::number);
        let disk_opt = body.get("diskGb").and_then(jsval::number);
        if cpu_opt.is_none() && mem_opt.is_none() && disk_opt.is_none() {
            return Some(json_response(
                400,
                json!({ "error": "Provide at least one of cpuCores, memoryMb, diskGb." }),
            ));
        }

        let mut rec_json = record.to_json();
        vmlib::update_virtual_machine_specs(&state.store, &record.id, cpu_opt, mem_opt, disk_opt);
        if let Some(c) = cpu_opt {
            rec_json["cpuCores"] = json!(c);
        }
        if let Some(m) = mem_opt {
            rec_json["memoryMb"] = json!(m);
        }
        if let Some(d) = disk_opt {
            rec_json["diskGb"] = json!(d);
        }

        let mut proxmox_error = None;
        if crate::proxmox_desktop::desktop().configured() {
            if let Err(e) = crate::proxmox_desktop::desktop()
                .update_hardware(&rec_json, cpu_opt, mem_opt, disk_opt)
                .await
            {
                proxmox_error = Some(crate::routes::vm::friendly_vm_error(&e.into()).1);
            }
        }

        crate::routes::vm::vm_audit(
            state,
            &actor.email,
            Some(&rec_json),
            "VM_ADMIN_SPEC_OVERRIDE",
            proxmox_error.is_none(),
            Some(&json!({
                "cpuCores": cpu_opt,
                "memoryMb": mem_opt,
                "diskGb": disk_opt,
                "proxmoxError": proxmox_error,
            })),
        );

        return Some(json_response(
            200,
            json!({
                "success": true,
                "message": match &proxmox_error {
                    Some(e) => format!("Specs updated in the system; the running VM may need a restart to pick up the change ({e})."),
                    None => "Specs updated.".to_string(),
                },
            }),
        ));
    }

    // GET /api/admin/vm-requests (admins only).
    if path == "/api/admin/vm-requests" && *method == Method::GET {
        if state.rate_limit_check(&ctx.ip, "anon", path).is_some() {
            return Some(json_response(
                429,
                json!({ "error": "Too many requests, slow down" }),
            ));
        }
        if !valid_id(&ctx.sid, state) || !ctx.is_admin(state) {
            return Some(json_response(401, json!({ "error": "unauthorized" })));
        }
        let data = state
            .store
            .read_document(&state.cfg.data_dir.join("vm_apps.json"), json!({}));
        let mut requests = data;
        if let Some(map) = requests.as_object_mut() {
            for (_key, app) in map.iter_mut() {
                if mitch_lib::auth::is_admin_email(
                    &state.store,
                    app.get("email").and_then(|v| v.as_str()).unwrap_or(""),
                ) {
                    app["billing"] = json!("admin_comped");
                    app["priceUsd"] = json!(0);
                }
            }
        }
        return Some(json_response(
            200,
            json!({ "success": true, "requests": requests }),
        ));
    }

    // POST /api/admin/deny-vm.
    if path == "/api/admin/deny-vm" && *method == Method::POST {
        if state.rate_limit_check(&ctx.ip, "anon", path).is_some() {
            return Some(json_response(
                429,
                json!({ "error": "Too many requests, slow down" }),
            ));
        }
        if !valid_id(&ctx.sid, state) || !ctx.is_admin(state) {
            return Some(json_response(401, json!({ "error": "unauthorized" })));
        }
        let target_email = body
            .get("email")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase()
            .trim()
            .to_string();
        if target_email.is_empty() {
            return Some(json_response(
                400,
                json!({ "error": "Valid email required." }),
            ));
        }
        let file = state.cfg.data_dir.join("vm_apps.json");
        let mut data = state.store.read_document(&file, json!({}));
        let norm = mitch_lib::auth::normalize_email(&target_email);
        if !data.get(norm.as_str()).is_some() {
            return Some(json_response(
                400,
                json!({ "error": "No VM application found." }),
            ));
        }
        if let Some(obj) = data.get_mut(norm.as_str()).and_then(|v| v.as_object_mut()) {
            obj.insert("status".into(), json!("denied"));
            obj.insert("deniedAt".into(), json!(now_millis()));
        }
        let _ = state.store.write_document(&file, &data);
        return Some(json_response(
            200,
            json!({ "success": true, "message": format!("VM request for {target_email} denied.") }),
        ));
    }

    // POST /api/admin/terminate-vm (soft delete; Proxmox stop stubbed).
    if path == "/api/admin/terminate-vm" && *method == Method::POST {
        if state.rate_limit_check(&ctx.ip, "anon", path).is_some() {
            return Some(json_response(
                429,
                json!({ "error": "Too many requests, slow down" }),
            ));
        }
        if !valid_id(&ctx.sid, state) || !ctx.is_admin(state) {
            return Some(json_response(401, json!({ "error": "unauthorized" })));
        }
        let target_email = body
            .get("email")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase()
            .trim()
            .to_string();
        if target_email.is_empty() {
            return Some(json_response(
                400,
                json!({ "error": "Valid email required." }),
            ));
        }
        let file = state.cfg.data_dir.join("vm_apps.json");
        let data = state.store.read_document(&file, json!({}));
        let norm = mitch_lib::auth::normalize_email(&target_email);
        let has_vmid = data
            .get(norm.as_str())
            .and_then(|app| app.get("vmid"))
            .is_some();
        if !has_vmid {
            return Some(json_response(
                400,
                json!({ "error": "No active/approved VM found for this user." }),
            ));
        }
        return Some(json_response(
            500,
            json!({ "error": "Proxmox API is not reachable from this environment." }),
        ));
    }

    // POST /api/admin/restore-vm.
    if path == "/api/admin/restore-vm" && *method == Method::POST {
        if state.rate_limit_check(&ctx.ip, "anon", path).is_some() {
            return Some(json_response(
                429,
                json!({ "error": "Too many requests, slow down" }),
            ));
        }
        if !valid_id(&ctx.sid, state) || !ctx.is_admin(state) {
            return Some(json_response(401, json!({ "error": "unauthorized" })));
        }
        let target_email = body
            .get("email")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase()
            .trim()
            .to_string();
        if target_email.is_empty() {
            return Some(json_response(
                400,
                json!({ "error": "Valid email required." }),
            ));
        }
        let file = state.cfg.data_dir.join("vm_apps.json");
        let data = state.store.read_document(&file, json!({}));
        let norm = mitch_lib::auth::normalize_email(&target_email);
        let app = data.get(norm.as_str()).cloned();
        let app_ok = app
            .as_ref()
            .map(|app| {
                app.get("status").and_then(|v| v.as_str()) == Some("deleted")
                    && app.get("vmid").is_some()
            })
            .unwrap_or(false);
        if !app_ok {
            return Some(json_response(
                400,
                json!({ "error": "No restorable VM found for this user." }),
            ));
        }
        let app = app.unwrap_or(json!({}));
        let deleted_at = app.get("deletedAt").and_then(|v| v.as_i64()).unwrap_or(0);
        if now_millis() - deleted_at >= 7 * 24 * 3600 * 1000 {
            return Some(json_response(
                400,
                json!({ "error": "Restore period of 7 days has expired. VM has been purged." }),
            ));
        }
        return Some(json_response(
            500,
            json!({ "error": "Proxmox API is not reachable from this environment." }),
        ));
    }

    // POST /api/admin/approve-vm.
    if path == "/api/admin/approve-vm" && *method == Method::POST {
        if state.rate_limit_check(&ctx.ip, "anon", path).is_some() {
            return Some(json_response(
                429,
                json!({ "error": "Too many requests, slow down" }),
            ));
        }
        if !valid_id(&ctx.sid, state) || !ctx.is_admin(state) {
            return Some(json_response(401, json!({ "error": "unauthorized" })));
        }
        let target_email = body
            .get("email")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase()
            .trim()
            .to_string();
        let vmid = body
            .get("vmid")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);
        if target_email.is_empty() || !vmid_in_range(vmid) {
            return Some(json_response(
                400,
                json!({ "error": format!("Valid email and VMID ({PVE_VMID_MIN}-{PVE_VMID_MAX}) are required.") }),
            ));
        }
        let file = state.cfg.data_dir.join("vm_apps.json");
        let data = state.store.read_document(&file, json!({}));
        let norm = mitch_lib::auth::normalize_email(&target_email);
        let Some(app) = data.get(norm.as_str()).cloned() else {
            return Some(json_response(
                400,
                json!({ "error": "No VM application found for this user." }),
            ));
        };
        if app.get("status").and_then(|v| v.as_str()) != Some("pending") {
            return Some(json_response(
                409,
                json!({ "error": "Only pending VM requests can be approved." }),
            ));
        }
        return Some(json_response(
            500,
            json!({ "error": "Proxmox API is not reachable from this environment." }),
        ));
    }

    // POST /api/admin/lxc-attach-sshd-hook (Step 13 with russh).
    if path == "/api/admin/lxc-attach-sshd-hook" && *method == Method::POST {
        if state.rate_limit_check(&ctx.ip, "anon", path).is_some() {
            return Some(json_response(
                429,
                json!({ "error": "Too many requests, slow down" }),
            ));
        }
        if !valid_id(&ctx.sid, state) {
            return Some(json_response(401, json!({ "error": "unauthorized" })));
        }
        if !ctx.is_admin(state) {
            return Some(json_response(403, json!({ "error": "forbidden" })));
        }
        let vmid = body
            .get("vmid")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);
        if !vmid_in_range(vmid) {
            return Some(json_response(
                400,
                json!({ "error": format!("vmid must be in [{PVE_VMID_MIN}, {PVE_VMID_MAX}]") }),
            ));
        }
        return Some(json_response(
            502,
            json!({ "success": false, "error": "LXC sshd hook attach is not yet available in the Rust build (Step 13 port)." }),
        ));
    }

    None
}

fn json_response(code: u16, obj: Value) -> Response {
    crate::errors::json_resp(code, obj)
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn valid_id(sid: &str, state: &Arc<AppState>) -> bool {
    !sid.is_empty() && mitch_lib::auth::valid_id(sid, &state.id_secret)
}

fn vmid_in_range(vmid: i64) -> bool {
    (PVE_VMID_MIN..=PVE_VMID_MAX).contains(&vmid)
}
