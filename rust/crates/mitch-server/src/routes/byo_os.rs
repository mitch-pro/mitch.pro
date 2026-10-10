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
use crate::routes::vm::{
    authenticated_vm_actor, get_user_vm_upgrades, vm_audit, vm_same_origin_request, PVE_VMID_MAX,
    PVE_VMID_MIN,
};
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

/// Link-based ISOs (`/iso/from-url`) have no size cap — the download is
/// relayed straight into Proxmox without this server ever holding the
/// whole file, so there's no local-disk reason to cap it. Size is billed
/// instead: BYO_OS_COST_COINS covers up to this many GB...
const LINK_ISO_INCLUDED_GB: f64 = 6.0;
/// ...a per-GB surcharge applies above that...
const LINK_ISO_SURCHARGE_PER_GB: f64 = 100.0;
/// ...which itself stops scaling at this many GB, so there's a ceiling on
/// total cost even for an arbitrarily large ISO.
const LINK_ISO_SURCHARGE_CAP_GB: f64 = 12.0;

/// Below this, a "successfully uploaded" ISO is almost certainly the wrong
/// thing entirely (a download-page HTML redirect, a login wall, a broken
/// mirror) rather than a real install image — small enough that a user can
/// self-refund with no review, instead of needing a ticket.
const SELF_REFUND_MAX_BYTES: u64 = 128 * 1024 * 1024;

/// `BYO_OS_COST_COINS` plus a per-GB surcharge for anything over
/// `LINK_ISO_INCLUDED_GB`, capped at `LINK_ISO_SURCHARGE_CAP_GB`.
fn link_iso_cost_coins(size_bytes: u64) -> f64 {
    let size_gb = size_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    let billable_overage_gb =
        (size_gb - LINK_ISO_INCLUDED_GB).clamp(0.0, LINK_ISO_SURCHARGE_CAP_GB - LINK_ISO_INCLUDED_GB);
    BYO_OS_COST_COINS + billable_overage_gb * LINK_ISO_SURCHARGE_PER_GB
}

fn fmt_gb(size_bytes: u64) -> String {
    format!("{:.1}GB", size_bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

// Only byo_os_uploads/ exists on this box now — a scratch area used for the
// brief reassemble-then-hand-off-to-Proxmox window in /iso/complete. There
// is deliberately no persistent byo_os_isos/ directory: once upload
// completes the ISO lives on Proxmox's own storage, not here, so it's never
// part of this app's own backups.
fn uploads_dir(state: &AppState) -> PathBuf {
    state.data_dir().join("byo_os_uploads")
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
    // The ISO lives on Proxmox's own storage (never this box's disk once
    // upload completes — see /iso/complete), so this deletes it there.
    if path == "/api/vm/byo-os/iso/delete" && *method == Method::POST {
        let mut manifest = state.store.read_document(&isos_manifest_file(state), json!({}));
        if let Some(entry) = manifest.get(&actor.email).cloned() {
            let proxmox_filename = jsval::str_or(entry.get("proxmoxFilename"), "");
            if !proxmox_filename.is_empty() {
                let _ = crate::proxmox_desktop::desktop()
                    .delete_iso(&proxmox_filename)
                    .await;
            }
            if let Some(map) = manifest.as_object_mut() {
                map.remove(&actor.email);
            }
            let _ = state.store.write_document(&isos_manifest_file(state), &manifest);
        }
        return Some(json_response(200, json!({ "ok": true })));
    }

    // POST /api/vm/byo-os/iso/refund-undersized — self-service refund for
    // an ISO that came back too small to possibly be a real install image
    // (the classic mistake: a link to a download *page* instead of the
    // file itself). No admin review needed — the size threshold is the
    // whole safety check, so it's tight and automatic.
    if path == "/api/vm/byo-os/iso/refund-undersized" && *method == Method::POST {
        let mut manifest = state.store.read_document(&isos_manifest_file(state), json!({}));
        let Some(entry) = manifest.get(&actor.email).cloned() else {
            return Some(json_response(404, json!({ "error": "No stored ISO found." })));
        };
        let size_bytes = entry.get("sizeBytes").and_then(jsval::number).unwrap_or(0.0) as u64;
        if size_bytes >= SELF_REFUND_MAX_BYTES {
            return Some(json_response(
                400,
                json!({
                    "error": format!(
                        "That ISO is {} — too large to self-refund. Submit a ticket instead and it'll be reviewed.",
                        fmt_gb(size_bytes)
                    ),
                    "code": "too_large_for_self_refund",
                }),
            ));
        }
        let proxmox_filename = jsval::str_or(entry.get("proxmoxFilename"), "");
        if !proxmox_filename.is_empty() {
            let _ = crate::proxmox_desktop::desktop().delete_iso(&proxmox_filename).await;
        }
        let refund = link_iso_cost_coins(size_bytes);
        mitch_lib::coins::add_coins(
            &state.store,
            state.data_dir(),
            &actor.email,
            refund,
            1.0,
            "byo_os_iso_self_refund",
        );
        if let Some(map) = manifest.as_object_mut() {
            map.remove(&actor.email);
        }
        let _ = state.store.write_document(&isos_manifest_file(state), &manifest);
        vm_audit(
            state,
            &actor.email,
            None,
            "BYO_OS_ISO_SELF_REFUNDED",
            true,
            Some(&json!({ "sizeBytes": size_bytes, "refundCoins": refund })),
        );
        return Some(json_response(
            200,
            json!({
                "ok": true,
                "refunded": refund,
                "coins": mitch_lib::coins::get_coins(&state.store, state.data_dir(), &actor.email),
            }),
        ));
    }

    // POST /api/vm/byo-os/iso/report-issue — one-click ticket for anything
    // the self-refund threshold doesn't cover (a large ISO that's still
    // wrong somehow, a VM that won't create from it, etc.) — pings the
    // admin to review and manually restore coins if warranted. Doesn't
    // touch the stored ISO or balance itself; at most one open report per
    // stored ISO (re-clicking after the same entry was already reported is
    // a no-op) so a confused user mashing the button doesn't spam alerts.
    if path == "/api/vm/byo-os/iso/report-issue" && *method == Method::POST {
        let mut manifest = state.store.read_document(&isos_manifest_file(state), json!({}));
        let Some(entry) = manifest.get(&actor.email).cloned() else {
            return Some(json_response(404, json!({ "error": "No stored ISO found." })));
        };
        if jsval::truthy(entry.get("reported").unwrap_or(&Value::Null)) {
            return Some(json_response(
                200,
                json!({ "ok": true, "message": "Already reported — it's in the queue." }),
            ));
        }

        let filename = jsval::str_or(entry.get("filename"), "custom.iso");
        let size_bytes = entry.get("sizeBytes").and_then(jsval::number).unwrap_or(0.0) as u64;
        let proxmox_filename = jsval::str_or(entry.get("proxmoxFilename"), "");
        let body: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
        let note = jsval::str_or(body.get("note"), "");

        let report = json!({
            "email": actor.email,
            "filename": filename,
            "sizeBytes": size_bytes,
            "proxmoxFilename": proxmox_filename,
            "uploadedAt": entry.get("uploadedAt").cloned().unwrap_or(Value::Null),
            "note": note,
            "reportedAt": mitch_lib::school::now_millis(),
        });
        let reports_file = state.data_dir().join("byo_os_reports.json");
        let mut reports = state.store.read_document(&reports_file, json!([]));
        if let Some(arr) = reports.as_array_mut() {
            arr.push(report);
        }
        let _ = state.store.write_document(&reports_file, &reports);

        crate::routes::push::ntfy_notify(
            &format!(
                "BYO-OS ISO issue from {}: \"{filename}\" ({}). {}",
                actor.email,
                fmt_gb(size_bytes),
                if note.is_empty() { "No note.".to_string() } else { format!("Note: {note}") }
            ),
            "BYO-OS Ticket",
            "high",
        );

        if let Some(map) = manifest.as_object_mut() {
            if let Some(obj) = map.get_mut(&actor.email).and_then(|v| v.as_object_mut()) {
                obj.insert("reported".to_string(), json!(true));
            }
        }
        let _ = state.store.write_document(&isos_manifest_file(state), &manifest);

        vm_audit(
            state,
            &actor.email,
            None,
            "BYO_OS_ISO_ISSUE_REPORTED",
            true,
            Some(&json!({ "filename": filename, "sizeBytes": size_bytes })),
        );

        return Some(json_response(
            200,
            json!({ "ok": true, "message": "Reported — it'll be reviewed and your coins restored if it checks out." }),
        ));
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
        if !crate::proxmox_desktop::desktop().configured() {
            return Some(json_response(503, json!({ "error": "Computer service is not configured." })));
        }

        // Reassembled only in a scratch spot and only for as long as it
        // takes to hand off to Proxmox — this box's own disk (and its
        // backups) never holds a finished ISO. local_path is removed on
        // every exit path below, success or failure.
        let _ = std::fs::create_dir_all(uploads_dir(state));
        let local_path = dir.join("assembled.iso");
        let assembled = (|| -> std::io::Result<u64> {
            use std::io::Write;
            let mut out = std::fs::File::create(&local_path)?;
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
                let _ = std::fs::remove_file(&local_path);
                return Some(json_response(500, json!({ "error": "Could not assemble the uploaded ISO." })));
            }
        };
        if written != declared_size {
            let _ = std::fs::remove_file(&local_path);
            return Some(json_response(
                400,
                json!({ "error": "Assembled file size didn't match what was declared — re-upload." }),
            ));
        }

        // Push to Proxmox's own storage immediately, then drop the local
        // copy — from here on the ISO lives on the hypervisor, not on this
        // box. Coins are only charged once this actually succeeds. The
        // destination filename is generated by upload_iso itself (and may
        // change across internal retries), so take whatever it reports back.
        let upload_result = crate::proxmox_desktop::desktop().upload_iso(&local_path).await;
        let _ = std::fs::remove_file(&local_path);
        let _ = std::fs::remove_dir(&dir);
        let proxmox_filename = match upload_result {
            Ok(name) => name,
            Err(e) => {
                let friendly = crate::routes::vm::friendly_vm_error(&e.into());
                return Some(json_response(
                    friendly.0,
                    json!({ "error": format!("Uploaded, but handing off to the computer service failed: {} Re-upload to try again.", friendly.1) }),
                ));
            }
        };

        // Enforce "1 ISO max": drop whatever Proxmox-side ISO was stored
        // before (best-effort — Proxmox's own ISO storage, not this box).
        let mut iso_manifest = state.store.read_document(&isos_manifest_file(state), json!({}));
        if let Some(prev) = iso_manifest.get(&actor.email).cloned() {
            let prev_filename = jsval::str_or(prev.get("proxmoxFilename"), "");
            if !prev_filename.is_empty() && prev_filename != proxmox_filename {
                let _ = crate::proxmox_desktop::desktop()
                    .delete_iso(&prev_filename)
                    .await;
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
            "proxmoxFilename": proxmox_filename,
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

    // POST /api/vm/byo-os/iso/from-url — fetch an ISO from a link instead of
    // uploading a file. Proxmox's own download-url API needs a Sys.Modify
    // grant on "/" (a much bigger privilege than this feature should need),
    // so instead this server relays the download itself: the file streams
    // source → us → Proxmox in one pass, never touching disk here and never
    // fully buffered in memory, so there's no size cap — see
    // link_iso_cost_coins for how a bigger file costs more instead.
    if path == "/api/vm/byo-os/iso/from-url" && *method == Method::POST {
        if is_vm_banned(state, &actor.email) {
            return Some(json_response(403, json!({ "error": "VM access is restricted on this account." })));
        }
        if is_byo_os_banned(state, &actor.email) {
            return Some(json_response(403, json!({ "error": "BYO-OS is restricted on this account." })));
        }
        let body: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
        let raw_url = jsval::str_or(body.get("url"), "").trim().to_string();
        if raw_url.len() > 2000 {
            return Some(json_response(400, json!({ "error": "That link is too long." })));
        }
        let parsed = url::Url::parse(&raw_url)
            .ok()
            .filter(|u| matches!(u.scheme(), "http" | "https"));
        let Some(parsed) = parsed else {
            return Some(json_response(
                400,
                json!({ "error": "Enter a valid http:// or https:// link to an ISO file." }),
            ));
        };

        // The real, size-adjusted cost isn't known until the download
        // finishes — this just filters out accounts that can't even cover
        // the minimum before any work starts.
        let balance = mitch_lib::coins::get_coins(&state.store, state.data_dir(), &actor.email);
        if balance < BYO_OS_COST_COINS {
            return Some(json_response(
                402,
                json!({
                    "error": format!("BYO-OS costs at least {BYO_OS_COST_COINS:.0} coins. You have {balance:.2}."),
                    "code": "insufficient_coins",
                }),
            ));
        }
        if !crate::proxmox_desktop::desktop().configured() {
            return Some(json_response(503, json!({ "error": "Computer service is not configured." })));
        }

        let display_filename = parsed
            .path_segments()
            .and_then(|mut segs| segs.next_back())
            .map(safe_iso_filename)
            .unwrap_or_else(|| "custom.iso".to_string());

        let (proxmox_filename, size_bytes) = match crate::proxmox_desktop::desktop()
            .upload_iso_from_url(&raw_url, &uploads_dir(state))
            .await
        {
            Ok(v) => v,
            Err(e) => {
                let friendly = crate::routes::vm::friendly_vm_error(&e.into());
                return Some(json_response(
                    friendly.0,
                    json!({ "error": format!("Fetching that ISO failed: {}", friendly.1) }),
                ));
            }
        };

        let total_cost = link_iso_cost_coins(size_bytes);
        let current_balance = mitch_lib::coins::get_coins(&state.store, state.data_dir(), &actor.email);
        if current_balance < total_cost {
            let _ = crate::proxmox_desktop::desktop().delete_iso(&proxmox_filename).await;
            return Some(json_response(
                402,
                json!({
                    "error": format!(
                        "That ISO is {} and costs {total_cost:.0} coins total. You have {current_balance:.2} — nothing was charged.",
                        fmt_gb(size_bytes)
                    ),
                    "code": "insufficient_coins",
                }),
            ));
        }

        let mut iso_manifest = state.store.read_document(&isos_manifest_file(state), json!({}));
        if let Some(prev) = iso_manifest.get(&actor.email).cloned() {
            let prev_filename = jsval::str_or(prev.get("proxmoxFilename"), "");
            if !prev_filename.is_empty() && prev_filename != proxmox_filename {
                let _ = crate::proxmox_desktop::desktop().delete_iso(&prev_filename).await;
            }
        }

        mitch_lib::coins::add_coins(
            &state.store,
            state.data_dir(),
            &actor.email,
            -total_cost,
            1.0,
            "byo_os_iso_upload",
        );

        let entry = json!({
            "filename": display_filename,
            "proxmoxFilename": proxmox_filename,
            "sizeBytes": size_bytes,
            "uploadedAt": mitch_lib::school::now_millis(),
        });
        if let Some(map) = iso_manifest.as_object_mut() {
            map.insert(actor.email.clone(), entry.clone());
        }
        let _ = state.store.write_document(&isos_manifest_file(state), &iso_manifest);

        vm_audit(
            state,
            &actor.email,
            None,
            "BYO_OS_ISO_UPLOADED",
            true,
            Some(&json!({ "filename": display_filename, "sizeBytes": size_bytes, "sourceUrl": raw_url, "costCoins": total_cost })),
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

    // POST /api/vm/byo-os/create — boot a from-scratch VM from the ISO
    // already sitting on Proxmox's storage (uploaded at /iso/complete time).
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
        let proxmox_filename = jsval::str_or(iso_entry.get("proxmoxFilename"), "");
        if proxmox_filename.is_empty() {
            return Some(json_response(
                410,
                json!({ "error": "Stored ISO is missing — re-upload it." }),
            ));
        }

        // Same lock namespace the template path's create/recreate use — a
        // user can only have one computer-provisioning operation in flight
        // at a time, regardless of which path (template or BYO-OS) started it.
        let lock_key = format!("byo-os-create-{}", actor.email);
        {
            let mut requests = state.vm_power_requests.lock().unwrap_or_else(|e| e.into_inner());
            if requests.contains_key(&lock_key)
                || requests.contains_key(&format!("create-{}", actor.email))
                || requests.contains_key(&format!("recreate-{}", actor.email))
            {
                return Some(json_response(
                    409,
                    json!({ "error": "A computer operation is already in progress for your account." }),
                ));
            }
            requests.insert(lock_key.clone(), json!({ "startedAt": mitch_lib::school::now_millis() as f64 }));
        }

        // Delete any existing computer(s) first — same as the template
        // path's "Delete & Recreate" does. This lets BYO-OS create double as
        // "replace my computer with a fresh one from a newly uploaded ISO"
        // instead of requiring a separate delete step through the template UI.
        let old_records = vmlib::get_virtual_machines_for_owner(&state.store, &actor.email);
        tracing::warn!(
            "[byo-os create] delete-loop found {} existing record(s) for {}",
            old_records.len(),
            actor.email
        );
        for old_rec in old_records {
            let old = old_rec.to_json();
            let old_id = jsval::str_or(old.get("id"), "");
            if let Err(err) = crate::proxmox_desktop::desktop()
                .delete_guest(&old, true)
                .await
            {
                tracing::warn!("[byo-os create] Note: Proxmox delete for {old_id} returned: {err:?}");
            }
            let deleted = vmlib::delete_virtual_machine(&state.store, &old_id);
            tracing::warn!("[byo-os create] local delete_virtual_machine({old_id}) -> {deleted}");
            crate::routes::vm::revoke_vm_desktop_connections(state, &old_id);
            crate::routes::vm::clear_vm_lease(state, &old_id);
            state
                .vm_page_presence
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&old_id);
            vm_audit(state, &actor.email, Some(&old), "VM_DELETED_FOR_RECREATE", true, None);
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

        // next_available_vmid only checks Proxmox's own live guest list plus
        // whatever vmids this request already knows about — a vmid it hands
        // back can still collide with another row already in our local
        // table (e.g. a row next_available_vmid can't see at all, such as
        // one belonging to a guest type it doesn't poll) by the time
        // reserve_virtual_machine's own existence check runs. Re-asking it
        // the exact same question would just hand back the exact same vmid
        // again — explicitly exclude every vmid that already collided this
        // request so each retry is forced to a genuinely new one.
        const MAX_RESERVE_ATTEMPTS: u32 = 5;
        let mut pending_record: Option<Value> = None;
        let mut vmid: f64 = 0.0;
        let mut hostname = String::new();
        let mut excluded_vmids: Vec<f64> = Vec::new();
        for attempt in 1..=MAX_RESERVE_ATTEMPTS {
            let all_records = vmlib::list_virtual_machines(&state.store, true);
            let mut existing_vmids: Vec<f64> = all_records.iter().map(|r| r.vmid).collect();
            existing_vmids.extend_from_slice(&excluded_vmids);
            let candidate_vmid = match crate::proxmox_desktop::desktop()
                .next_available_vmid(PVE_VMID_MIN, PVE_VMID_MAX, &existing_vmids)
                .await
            {
                Ok(v) => v as f64,
                Err(e) => {
                    state.vm_power_requests.lock().unwrap_or_else(|e| e.into_inner()).remove(&lock_key);
                    let friendly = crate::routes::vm::friendly_vm_error(&e.into());
                    return Some(json_response(friendly.0, json!({ "error": friendly.1, "code": friendly.2 })));
                }
            };
            let candidate_hostname = format!("byo-os-{}", candidate_vmid as i64);
            let reserved = vmlib::reserve_virtual_machine(
                &state.store,
                &json!({
                    "id": format!("vm-{}", candidate_vmid as i64),
                    "ownerEmail": actor.email,
                    "ownerUserId": mitch_lib::profile::get_uid_for_email(&state.store, &state.id_secret, &actor.email).unwrap_or_default(),
                    "vmid": candidate_vmid,
                    "node": crate::proxmox_desktop::desktop().node(),
                    "guestType": "qemu",
                    "friendlyName": "My Computer (BYO-OS)",
                    "hostname": candidate_hostname,
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
            if let Some(rec) = reserved {
                pending_record = Some(rec);
                vmid = candidate_vmid;
                hostname = candidate_hostname;
                break;
            }
            tracing::warn!(
                "[byo-os create] reserve_virtual_machine attempt {attempt}/{MAX_RESERVE_ATTEMPTS} collided on vmid {candidate_vmid}"
            );
            excluded_vmids.push(candidate_vmid);
        }
        let Some(pending_record) = pending_record else {
            state.vm_power_requests.lock().unwrap_or_else(|e| e.into_inner()).remove(&lock_key);
            return Some(json_response(
                500,
                json!({ "error": "Could not reserve a computer slot. Please try again." }),
            ));
        };

        // The ISO is already on Proxmox's storage (uploaded at /iso/complete
        // time) — this just builds the VM against it.
        let result = crate::proxmox_desktop::desktop()
            .create_from_iso(&crate::proxmox_desktop::CreateFromIsoParams {
                vmid: Some(vmid),
                hostname: hostname.clone(),
                cpu_cores: Some(cpu_cores),
                memory_mb: Some(memory_mb),
                disk_gb: Some(disk_gb),
                iso_filename: proxmox_filename.clone(),
            })
            .await;
        state.vm_power_requests.lock().unwrap_or_else(|e| e.into_inner()).remove(&lock_key);

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

    // POST /api/vm/byo-os/detach-iso — once the OS is installed, eject the
    // install ISO and drop it from the boot order so the next boot goes
    // straight to disk instead of back into the installer. Only meaningful
    // for a BYO-OS computer (templateVmid is null — a template-cloned one
    // never had an ISO attached in the first place).
    if path == "/api/vm/byo-os/detach-iso" && *method == Method::POST {
        if is_vm_banned(state, &actor.email) {
            return Some(json_response(403, json!({ "error": "VM access is restricted on this account." })));
        }
        let records = vmlib::get_virtual_machines_for_owner(&state.store, &actor.email);
        let Some(record) = records.into_iter().find(|r| r.template_vmid.is_none()) else {
            return Some(json_response(
                404,
                json!({ "error": "No BYO-OS computer found on your account." }),
            ));
        };
        let record_json = record.to_json();
        let node = jsval::str_or(record_json.get("node"), crate::proxmox_desktop::desktop().node());
        if let Err(e) = crate::proxmox_desktop::desktop()
            .detach_iso(&node, record.vmid as i64)
            .await
        {
            let friendly = crate::routes::vm::friendly_vm_error(&e.into());
            return Some(json_response(friendly.0, json!({ "error": friendly.1 })));
        }
        vm_audit(state, &actor.email, Some(&record_json), "BYO_OS_ISO_DETACHED", true, None);
        return Some(json_response(
            200,
            json!({ "ok": true, "message": "ISO detached — restart your computer to boot from disk." }),
        ));
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

    #[test]
    fn link_iso_cost_scales_then_caps() {
        const GB: u64 = 1024 * 1024 * 1024;
        // At or under the included 6GB: flat base fee, no surcharge.
        assert_eq!(link_iso_cost_coins(0), BYO_OS_COST_COINS);
        assert_eq!(link_iso_cost_coins(6 * GB), BYO_OS_COST_COINS);
        // Between 6 and 12GB: base + 100/GB for the overage.
        assert_eq!(link_iso_cost_coins(9 * GB), BYO_OS_COST_COINS + 300.0);
        assert_eq!(link_iso_cost_coins(12 * GB), BYO_OS_COST_COINS + 600.0);
        // Past 12GB the surcharge stops scaling — same price as exactly 12GB.
        assert_eq!(link_iso_cost_coins(50 * GB), BYO_OS_COST_COINS + 600.0);
        assert_eq!(link_iso_cost_coins(500 * GB), link_iso_cost_coins(12 * GB));
    }
}
