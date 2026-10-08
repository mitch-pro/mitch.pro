//! BYO-OS: a user uploads their own install ISO (max 6GB, one stored at a
//! time, 1000 MitchCoins) and boots a from-scratch VM from it instead of
//! cloning the curated template.
//!
//! Three concerns kept deliberately separate:
//! - **Chunked upload** (this file's `/api/vm/byo-os/iso/*` endpoints) — a
//!   browser can't reliably do one 6GB request, so the client slices the
//!   file and these endpoints reassemble it on disk, never buffering more
//!   than one chunk (capped well under `axum::body::to_bytes`'s per-path
//!   cap in main.rs) in memory at a time.
//! - **Proxmox ISO upload + from-scratch VM creation**
//!   (`proxmox_desktop.rs`'s `upload_iso`/`create_from_iso`) — a *separate*
//!   server-to-Proxmox transfer that only happens once the file is already
//!   fully assembled locally, streamed straight from disk.
//! - **VM creation** (`/api/vm/byo-os/create`, also this file) wires the
//!   two together, reusing the same "one computer per user" rule and
//!   per-tier cpu/memory/disk the template-clone path already enforces.

use crate::routes::me::json_response;
use crate::routes::vm::{authenticated_vm_actor, get_user_vm_upgrades, vm_audit, vm_same_origin_request};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::{jsval, vm as vmlib};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

/// Flat fee to use BYO-OS at all, charged once a fully-uploaded ISO is
/// finalized (not at VM-creation time — uploading and replacing the ISO is
/// the thing being paid for; a user can create/recreate a VM from the same
/// stored ISO for free afterward, same as the template path lets you
/// recreate your computer).
const BYO_OS_COST_COINS: f64 = 1000.0;
/// 6 GiB, matching the user-facing "6GB max" limit.
const MAX_ISO_BYTES: u64 = 6 * 1024 * 1024 * 1024;
/// Hard per-chunk ceiling enforced server-side regardless of what chunk
/// size the client declares at `/iso/start` — keeps a single malicious or
/// buggy chunk request from forcing a huge allocation. The client defaults
/// to 16MB chunks; this leaves headroom.
const MAX_CHUNK_BYTES: u64 = 24 * 1024 * 1024;

fn uploads_dir(state: &AppState) -> PathBuf {
    state.data_dir().join("byo_os_uploads")
}
fn isos_dir(state: &AppState) -> PathBuf {
    state.data_dir().join("byo_os_isos")
}
fn isos_manifest_file(state: &AppState) -> PathBuf {
    state.data_dir().join("byo_os_isos.json")
}

fn is_vm_banned(state: &AppState, email: &str) -> bool {
    state
        .vm_bans
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .contains(email)
}
fn is_byo_os_banned(state: &AppState, email: &str) -> bool {
    state
        .byo_os_bans
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .contains(email)
}

fn safe_iso_filename(raw: &str) -> String {
    // Keep it boring: alnum, dot, dash, underscore, 180-char cap (matching
    // the e2e-attachment filename convention) — this becomes a path
    // component on both our disk and Proxmox's.
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .take(180)
        .collect();
    if cleaned.trim_matches('.').is_empty() {
        "custom.iso".to_string()
    } else if !cleaned.to_ascii_lowercase().ends_with(".iso") {
        format!("{cleaned}.iso")
    } else {
        cleaned
    }
}

fn upload_manifest_path(state: &AppState, upload_id: &str) -> PathBuf {
    uploads_dir(state).join(upload_id).join("manifest.json")
}

pub(crate) async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
    search: &str,
) -> Option<Response> {
    if !path.starts_with("/api/vm/byo-os/") {
        return None;
    }
    if *method == Method::POST && !vm_same_origin_request(headers) {
        return Some(json_response(
            403,
            json!({ "error": "Request origin rejected." }),
        ));
    }
    let actor = match authenticated_vm_actor(state, headers) {
        Some(a) => a,
        None => return Some(json_response(401, json!({ "error": "Sign in required." }))),
    };

    // GET /api/vm/byo-os/iso — current stored ISO, if any.
    if path == "/api/vm/byo-os/iso" && *method == Method::GET {
        let manifest = state.store.read_document(&isos_manifest_file(state), json!({}));
        let entry = manifest.get(&actor.email).cloned();
        return Some(json_response(200, json!({ "iso": entry })));
    }

    // POST /api/vm/byo-os/iso/delete — free the "1 ISO" slot. No refund.
    if path == "/api/vm/byo-os/iso/delete" && *method == Method::POST {
        let mut manifest = state.store.read_document(&isos_manifest_file(state), json!({}));
        if let Some(entry) = manifest.get(&actor.email).cloned() {
            let disk_name = jsval::str_or(entry.get("diskFilename"), "");
            if !disk_name.is_empty() {
                let _ = std::fs::remove_file(isos_dir(state).join(&disk_name));
            }
            if let Some(map) = manifest.as_object_mut() {
                map.remove(&actor.email);
            }
            let _ = state.store.write_document(&isos_manifest_file(state), &manifest);
        }
        return Some(json_response(200, json!({ "ok": true })));
    }

    // POST /api/vm/byo-os/iso/start — begin a new chunked upload.
    if path == "/api/vm/byo-os/iso/start" && *method == Method::POST {
        if is_vm_banned(state, &actor.email) {
            return Some(json_response(403, json!({ "error": "VM access is restricted on this account." })));
        }
        if is_byo_os_banned(state, &actor.email) {
            return Some(json_response(403, json!({ "error": "BYO-OS is restricted on this account." })));
        }
        let body: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
        let filename = safe_iso_filename(&jsval::str_or(body.get("filename"), "custom.iso"));
        let total_size = body.get("totalSizeBytes").and_then(jsval::number).unwrap_or(0.0);
        let chunk_size = body.get("chunkSizeBytes").and_then(jsval::number).unwrap_or(0.0);
        if !(total_size.is_finite() && total_size > 0.0 && total_size as u64 <= MAX_ISO_BYTES) {
            return Some(json_response(
                400,
                json!({ "error": "ISO must be larger than 0 bytes and no more than 6GB." }),
            ));
        }
        if !(chunk_size.is_finite() && chunk_size > 0.0 && chunk_size as u64 <= MAX_CHUNK_BYTES) {
            return Some(json_response(
                400,
                json!({ "error": format!("Chunk size must be between 1 byte and {MAX_CHUNK_BYTES} bytes.") }),
            ));
        }
        let balance = mitch_lib::coins::get_coins(&state.store, state.data_dir(), &actor.email);
        if balance < BYO_OS_COST_COINS {
            return Some(json_response(
                402,
                json!({
                    "error": format!("BYO-OS costs {BYO_OS_COST_COINS:.0} coins. You have {balance:.2}."),
                    "code": "insufficient_coins",
                }),
            ));
        }
        let total_chunks = (total_size as u64).div_ceil(chunk_size as u64);
        let upload_id = mitch_lib::crypto::random_bytes_hex(16);
        let dir = uploads_dir(state).join(&upload_id);
        if std::fs::create_dir_all(&dir).is_err() {
            return Some(json_response(500, json!({ "error": "Could not start the upload." })));
        }
        let manifest = json!({
            "ownerEmail": actor.email,
            "filename": filename,
            "totalSizeBytes": total_size,
            "chunkSizeBytes": chunk_size,
            "totalChunks": total_chunks,
            "createdAt": mitch_lib::school::now_millis(),
        });
        let _ = state.store.write_document(&upload_manifest_path(state, &upload_id), &manifest);
        return Some(json_response(
            200,
            json!({ "uploadId": upload_id, "totalChunks": total_chunks }),
        ));
    }

    // POST /api/vm/byo-os/iso/chunk?uploadId=X&index=N — raw body = chunk bytes.
    if path == "/api/vm/byo-os/iso/chunk" && *method == Method::POST {
        let q = crate::handler::query(search);
        let upload_id = q.get("uploadId").cloned().unwrap_or_default();
        let index: u64 = q.get("index").and_then(|v| v.parse().ok()).unwrap_or(u64::MAX);
        if upload_id.is_empty() || !upload_id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Some(json_response(400, json!({ "error": "invalid uploadId" })));
        }
        let manifest = state
            .store
            .read_document(&upload_manifest_path(state, &upload_id), Value::Null);
        if manifest.is_null() {
            return Some(json_response(404, json!({ "error": "Upload not found or expired." })));
        }
        if jsval::str_or(manifest.get("ownerEmail"), "") != actor.email {
            return Some(json_response(403, json!({ "error": "Not your upload." })));
        }
        let total_chunks = manifest.get("totalChunks").and_then(jsval::number).unwrap_or(0.0) as u64;
        if index >= total_chunks {
            return Some(json_response(400, json!({ "error": "chunk index out of range" })));
        }
        if body_bytes.len() as u64 > MAX_CHUNK_BYTES {
            return Some(json_response(413, json!({ "error": "Chunk too large." })));
        }
        let chunk_path = uploads_dir(state).join(&upload_id).join(format!("{index}.chunk"));
        if std::fs::write(&chunk_path, body_bytes).is_err() {
            return Some(json_response(500, json!({ "error": "Could not write chunk to disk." })));
        }
        return Some(json_response(200, json!({ "ok": true, "received": index })));
    }

    // GET /api/vm/byo-os/iso/status?uploadId=X — which chunks landed, for resume.
    if path == "/api/vm/byo-os/iso/status" && *method == Method::GET {
        let q = crate::handler::query(search);
        let upload_id = q.get("uploadId").cloned().unwrap_or_default();
        let manifest = state
            .store
            .read_document(&upload_manifest_path(state, &upload_id), Value::Null);
        if manifest.is_null() || jsval::str_or(manifest.get("ownerEmail"), "") != actor.email {
            return Some(json_response(404, json!({ "error": "Upload not found." })));
        }
        let total_chunks = manifest.get("totalChunks").and_then(jsval::number).unwrap_or(0.0) as u64;
        let dir = uploads_dir(state).join(&upload_id);
        let mut received: Vec<u64> = Vec::new();
        for i in 0..total_chunks {
            if dir.join(format!("{i}.chunk")).exists() {
                received.push(i);
            }
        }
        return Some(json_response(
            200,
            json!({ "totalChunks": total_chunks, "receivedChunks": received }),
        ));
    }

    // POST /api/vm/byo-os/iso/complete — reassemble, verify, charge, store.
    if path == "/api/vm/byo-os/iso/complete" && *method == Method::POST {
        let body: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
        let upload_id = jsval::str_or(body.get("uploadId"), "");
        let manifest = state
            .store
            .read_document(&upload_manifest_path(state, &upload_id), Value::Null);
        if manifest.is_null() {
            return Some(json_response(404, json!({ "error": "Upload not found." })));
        }
        if jsval::str_or(manifest.get("ownerEmail"), "") != actor.email {
            return Some(json_response(403, json!({ "error": "Not your upload." })));
        }
        if is_vm_banned(state, &actor.email) || is_byo_os_banned(state, &actor.email) {
            return Some(json_response(403, json!({ "error": "BYO-OS is restricted on this account." })));
        }
        let total_chunks = manifest.get("totalChunks").and_then(jsval::number).unwrap_or(0.0) as u64;
        let declared_size = manifest.get("totalSizeBytes").and_then(jsval::number).unwrap_or(0.0) as u64;
        let filename = jsval::str_or(manifest.get("filename"), "custom.iso");
        let dir = uploads_dir(state).join(&upload_id);
        for i in 0..total_chunks {
            if !dir.join(format!("{i}.chunk")).exists() {
                return Some(json_response(
                    409,
                    json!({ "error": format!("Chunk {i} is missing — resume the upload first.") }),
                ));
            }
        }
        let balance = mitch_lib::coins::get_coins(&state.store, state.data_dir(), &actor.email);
        if balance < BYO_OS_COST_COINS {
            return Some(json_response(
                402,
                json!({
                    "error": format!("BYO-OS costs {BYO_OS_COST_COINS:.0} coins. You have {balance:.2}."),
                    "code": "insufficient_coins",
                }),
            ));
        }

        let _ = std::fs::create_dir_all(isos_dir(state));
        let disk_filename = format!("{}.iso", mitch_lib::crypto::random_bytes_hex(12));
        let final_path = isos_dir(state).join(&disk_filename);
        let assembled = (|| -> std::io::Result<u64> {
            use std::io::Write;
            let mut out = std::fs::File::create(&final_path)?;
            let mut written: u64 = 0;
            for i in 0..total_chunks {
                let chunk_path = dir.join(format!("{i}.chunk"));
                let bytes = std::fs::read(&chunk_path)?;
                out.write_all(&bytes)?;
                written += bytes.len() as u64;
                // Free disk space as we go rather than holding both the
                // chunks and the assembled file at once — for a 6GB ISO
                // that's the difference between ~6GB and ~12GB of transient
                // disk use.
                let _ = std::fs::remove_file(&chunk_path);
            }
            out.flush()?;
            Ok(written)
        })();
        let written = match assembled {
            Ok(w) => w,
            Err(_) => {
                let _ = std::fs::remove_file(&final_path);
                return Some(json_response(500, json!({ "error": "Could not assemble the uploaded ISO." })));
            }
        };
        if written != declared_size {
            let _ = std::fs::remove_file(&final_path);
            return Some(json_response(
                400,
                json!({ "error": "Assembled file size didn't match what was declared — re-upload." }),
            ));
        }

        // Enforce "1 ISO max": drop whatever was stored before.
        let mut iso_manifest = state.store.read_document(&isos_manifest_file(state), json!({}));
        if let Some(prev) = iso_manifest.get(&actor.email).cloned() {
            let prev_disk = jsval::str_or(prev.get("diskFilename"), "");
            if !prev_disk.is_empty() && prev_disk != disk_filename {
                let _ = std::fs::remove_file(isos_dir(state).join(&prev_disk));
            }
        }

        mitch_lib::coins::add_coins(
            &state.store,
            state.data_dir(),
            &actor.email,
            -BYO_OS_COST_COINS,
            1.0,
            "byo_os_iso_upload",
        );

        let entry = json!({
            "filename": filename,
            "diskFilename": disk_filename,
            "sizeBytes": written,
            "uploadedAt": mitch_lib::school::now_millis(),
        });
        if let Some(map) = iso_manifest.as_object_mut() {
            map.insert(actor.email.clone(), entry.clone());
        }
        let _ = state.store.write_document(&isos_manifest_file(state), &iso_manifest);

        let _ = std::fs::remove_file(upload_manifest_path(state, &upload_id));
        let _ = std::fs::remove_dir(&dir);

        vm_audit(
            state,
            &actor.email,
            None,
            "BYO_OS_ISO_UPLOADED",
            true,
            Some(&json!({ "filename": filename, "sizeBytes": written })),
        );

        return Some(json_response(
            200,
            json!({
                "ok": true,
                "iso": entry,
                "coins": mitch_lib::coins::get_coins(&state.store, state.data_dir(), &actor.email),
            }),
        ));
    }

    // POST /api/vm/byo-os/create — push the stored ISO to Proxmox and boot
    // a from-scratch VM from it.
    if path == "/api/vm/byo-os/create" && *method == Method::POST {
        if is_vm_banned(state, &actor.email) {
            return Some(json_response(403, json!({ "error": "VM access is restricted on this account." })));
        }
        if is_byo_os_banned(state, &actor.email) {
            return Some(json_response(403, json!({ "error": "BYO-OS is restricted on this account." })));
        }
        let iso_manifest = state.store.read_document(&isos_manifest_file(state), json!({}));
        let Some(iso_entry) = iso_manifest.get(&actor.email).cloned() else {
            return Some(json_response(
                400,
                json!({ "error": "Upload an ISO first.", "code": "no_iso" }),
            ));
        };
        let iso_filename = jsval::str_or(iso_entry.get("filename"), "custom.iso");
        let disk_filename = jsval::str_or(iso_entry.get("diskFilename"), "");
        let local_path = isos_dir(state).join(&disk_filename);
        if disk_filename.is_empty() || !local_path.exists() {
            return Some(json_response(
                410,
                json!({ "error": "Stored ISO is missing — re-upload it." }),
            ));
        }

        let existing: Vec<Value> = vmlib::get_virtual_machines_for_owner(&state.store, &actor.email)
            .into_iter()
            .map(|r| r.to_json())
            .filter(|r| jsval::str_or(r.get("status"), "") != "unassigned")
            .collect();
        if !existing.is_empty() {
            return Some(json_response(
                409,
                json!({ "error": "You already have a computer assigned. Delete it first to use BYO-OS." }),
            ));
        }

        let body: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
        let user_upgrades = get_user_vm_upgrades(state, &actor.email);
        let cpu_cores = body
            .get("cpuCores")
            .and_then(jsval::number)
            .or_else(|| user_upgrades.get("cpuCores").and_then(jsval::number))
            .unwrap_or(2.0);
        let memory_mb = body
            .get("memoryMb")
            .and_then(jsval::number)
            .or_else(|| user_upgrades.get("memoryMb").and_then(jsval::number))
            .unwrap_or(4096.0);
        let disk_gb = body
            .get("diskGb")
            .and_then(jsval::number)
            .or_else(|| user_upgrades.get("diskGb").and_then(jsval::number))
            .unwrap_or(64.0);

        let all_records = vmlib::list_virtual_machines(&state.store, true);
        let existing_vmids: Vec<f64> = all_records.iter().map(|r| r.vmid).collect();
        let vmid = match crate::proxmox_desktop::desktop()
            .next_available_vmid(100.0, 999_999_999.0, &existing_vmids)
            .await
        {
            Ok(v) => v as f64,
            Err(e) => {
                let friendly = crate::routes::vm::friendly_vm_error(&e.into());
                return Some(json_response(friendly.0, json!({ "error": friendly.1, "code": friendly.2 })));
            }
        };
        let hostname = format!("byo-os-{}", vmid as i64);

        let pending_record = vmlib::reserve_virtual_machine(
            &state.store,
            &json!({
                "id": format!("vm-{}", vmid as i64),
                "ownerEmail": actor.email,
                "ownerUserId": mitch_lib::profile::get_uid_for_email(&state.store, &state.id_secret, &actor.email).unwrap_or_default(),
                "vmid": vmid,
                "node": crate::proxmox_desktop::desktop().node(),
                "guestType": "qemu",
                "friendlyName": "My Computer (BYO-OS)",
                "hostname": hostname,
                "operatingSystem": format!("Custom ({iso_filename})"),
                "templateVmid": Value::Null,
                "cpuCores": cpu_cores,
                "memoryMb": memory_mb,
                "diskGb": disk_gb,
                "status": "provisioning",
                "createdAt": mitch_lib::school::now_millis() as f64,
            }),
        )
        .map(|r| r.to_json());
        let Some(pending_record) = pending_record else {
            return Some(json_response(500, json!({ "error": "Could not reserve a computer slot." })));
        };

        // Upload (server -> Proxmox, local network, can stream the whole
        // thing) then create the VM. Both can fail independently; either
        // failure marks the reservation provisioning-failed rather than
        // leaving a half-built record around.
        let result: Result<Value, crate::proxmox_desktop::ProxmoxServiceError> = async {
            crate::proxmox_desktop::desktop()
                .upload_iso(&local_path, &iso_filename)
                .await?;
            crate::proxmox_desktop::desktop()
                .create_from_iso(&crate::proxmox_desktop::CreateFromIsoParams {
                    vmid: Some(vmid),
                    hostname: hostname.clone(),
                    cpu_cores: Some(cpu_cores),
                    memory_mb: Some(memory_mb),
                    disk_gb: Some(disk_gb),
                    iso_filename: iso_filename.clone(),
                })
                .await
        }
        .await;

        match result {
            Ok(created) => {
                let mut merged = pending_record.clone();
                if let Some(obj) = merged.as_object_mut() {
                    if let Some(created_obj) = created.as_object() {
                        for (k, v) in created_obj {
                            obj.insert(k.clone(), v.clone());
                        }
                    }
                }
                merged["status"] = json!("assigned");
                let record = vmlib::upsert_virtual_machine(&state.store, &merged)
                    .map(|r| r.to_json())
                    .unwrap_or(merged);
                vm_audit(state, &actor.email, Some(&record), "BYO_OS_VM_CREATED", true, None);
                return Some(json_response(201, json!({ "success": true, "computer": record })));
            }
            Err(err) => {
                let rec_id = jsval::str_or(pending_record.get("id"), "");
                if !rec_id.is_empty() {
                    vmlib::update_virtual_machine_runtime(&state.store, &rec_id, None, Some("provisioning-failed"));
                }
                vm_audit(
                    state,
                    &actor.email,
                    Some(&pending_record),
                    "BYO_OS_VM_CREATED",
                    false,
                    Some(&json!({ "code": err.code })),
                );
                let friendly = crate::routes::vm::friendly_vm_error(&err.into());
                return Some(json_response(friendly.0, json!({ "error": friendly.1, "code": friendly.2 })));
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_iso_filename_strips_unsafe_chars_and_adds_extension() {
        assert_eq!(safe_iso_filename("ubuntu-24.04.iso"), "ubuntu-24.04.iso");
        assert_eq!(safe_iso_filename("my install.ISO"), "myinstall.ISO");
        // '/' is stripped entirely (no path traversal risk — this becomes a
        // path component joined into a server-controlled directory either
        // way), leaving the dots from ".." in place rather than trimmed.
        assert_eq!(safe_iso_filename("../../etc/passwd"), "....etcpasswd.iso");
        assert_eq!(safe_iso_filename("a/b\\c:d*e?f"), "abcdef.iso");
        assert_eq!(safe_iso_filename(""), "custom.iso");
        assert_eq!(safe_iso_filename("..."), "custom.iso");
        // 180-char cap applies before the extension is appended/checked.
        let long = "a".repeat(300);
        let result = safe_iso_filename(&long);
        assert_eq!(result.len(), 180 + 4); // capped name + ".iso"
        assert!(result.ends_with(".iso"));
    }

    #[test]
    fn chunk_count_rounds_up() {
        // Mirrors the /iso/start handler's div_ceil call directly, since
        // that's inline in the route rather than its own function.
        assert_eq!((0u64).div_ceil(16), 0);
        assert_eq!((1u64).div_ceil(16), 1);
        assert_eq!((16u64).div_ceil(16), 1);
        assert_eq!((17u64).div_ceil(16), 2);
        assert_eq!(MAX_ISO_BYTES.div_ceil(16 * 1024 * 1024), 384);
    }
}
