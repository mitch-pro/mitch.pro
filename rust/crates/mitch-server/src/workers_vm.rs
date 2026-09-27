//! VM background workers and boot tasks (Step 13 batch 4).
//!
//! Replaces the VM `setInterval` timers and startup block in `server.js`:
//! - `sampleVmUsageWorker` (server.js:27432-27507) — every 60s + on boot
//! - `purgeExpiredVmsWorker` (server.js:28465-28492) — every 3600s + on boot
//! - `pruneInactiveFreeVmsWorker` (server.js:28494-28511) — every 300s + on boot
//! - `enforceVmMaxUptimeWorker` (server.js:27874-27986) — every 15s
//! - `cleanupVmDesktopSessions` (server.js:12169, 27863) — every 5s + on boot
//! - Boot sequence:
//!   1. `prune_old_vm_usage_samples(&state.store, 30)`
//!   2. `migrate_legacy_vm_ownership(&state)`
//!   3. `init_portal_ssh_key(data_dir)`
//!   4. `cleanup_all_ephemeral_vms(&state)`

use crate::state::AppState;
use mitch_lib::jsval;
use mitch_lib::vm as vmlib;
use mitch_lib::vm_security::{get_vm_day_key, is_vm_inactive};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `sampleVmUsageWorker()` (server.js:27432-27507) — records usage snapshots
/// for all running VMs into SQLite `vm_usage_samples`.
pub(crate) async fn sample_vm_usage_worker(state: &AppState) {
    let now = now_ms() as f64;
    let day_key = get_vm_day_key(now as i64);

    let mut active_sockets_by_record: HashMap<String, Vec<String>> = HashMap::new();
    {
        let sockets = state
            .vm_desktop_sockets
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for client in sockets.values() {
            let d = &client.data;
            let rec_id = jsval::str_or(d.get("recordId"), "");
            if !rec_id.is_empty() {
                let user =
                    jsval::str_or(d.get("actorEmail"), &jsval::str_or(d.get("ownerEmail"), ""));
                if !user.is_empty() {
                    active_sockets_by_record
                        .entry(rec_id)
                        .or_default()
                        .push(user);
                }
            }
        }
    }

    let records = vmlib::list_virtual_machines(&state.store, true);
    let guests = if crate::proxmox_desktop::desktop().configured() {
        crate::proxmox_desktop::desktop()
            .list_guests()
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let guests_by_vmid: HashMap<i64, &Value> = guests
        .iter()
        .filter_map(|g| g.get("vmid").and_then(jsval::number).map(|v| (v as i64, g)))
        .collect();

    for record in records {
        if record.owner_email.is_empty() {
            continue;
        }
        let guest = guests_by_vmid.get(&(record.vmid as i64));
        let mut is_running = false;
        let mut uptime = 0.0;

        if let Some(g) = guest {
            let st = jsval::str_or(g.get("status"), "");
            if st == "running" || st == "paused" {
                is_running = true;
                uptime = g.get("uptime").and_then(jsval::number).unwrap_or(0.0);
            }
        }

        if !is_running {
            let presence = state
                .vm_page_presence
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&record.id)
                .copied();
            if let Some(last_seen) = presence {
                if now - (last_seen as f64) < 15.0 * 60.0 * 1000.0 {
                    is_running = true;
                }
            }
        }

        let active_users = active_sockets_by_record
            .get(&record.id)
            .cloned()
            .unwrap_or_default();
        if !active_users.is_empty() {
            is_running = true;
        }

        if is_running {
            let vm_name = if !record.friendly_name.is_empty() {
                &record.friendly_name
            } else if !record.hostname.is_empty() {
                &record.hostname
            } else {
                &record.id
            };
            vmlib::record_vm_usage_sample(
                &state.store,
                Some(now),
                &day_key,
                None,
                None,
                &record.owner_email,
                &record.id,
                Some(record.vmid),
                vm_name,
                uptime,
                &active_users.join(","),
                true,
            );
        }
    }
}

/// `purgeExpiredVmsWorker()` (server.js:28465-28492) — deletes VMs whose
/// applications entry has been in status='deleted' for >= 7 days.
pub(crate) async fn purge_expired_vms_worker(state: &AppState) {
    let mut data = crate::routes::vm::vm_applications(state);
    let mut changed = false;
    let now = now_ms() as f64;
    let one_week_ms = 7.0 * 24.0 * 3600.0 * 1000.0;

    let mut to_purge: Vec<(String, f64)> = Vec::new();
    if let Some(obj) = data.as_object() {
        for (email, app) in obj {
            if jsval::str_or(app.get("status"), "") == "deleted" {
                let deleted_at = app.get("deletedAt").and_then(jsval::number).unwrap_or(0.0);
                if deleted_at > 0.0 && (now - deleted_at) >= one_week_ms {
                    let vmid = app.get("vmid").and_then(jsval::number).unwrap_or(0.0);
                    to_purge.push((email.clone(), vmid));
                }
            }
        }
    }

    for (email, vmid) in to_purge {
        if vmid > 0.0 {
            println!(
                "[purge-vm] Restoring period expired for VM {} ({}). Purging from Proxmox...",
                vmid as i64, email
            );
            let _ = crate::routes::vm::destroy_user_vm(state, vmid).await;
        }
        if let Some(obj) = data.as_object_mut() {
            obj.remove(&email);
            changed = true;
        }
    }

    if changed {
        crate::routes::vm::save_vm_json(state, "vm_applications.json", &data);
    }
}

/// `pruneInactiveFreeVmsWorker()` (server.js:28494-28511) — tears down ephemeral
/// free VMs inactive for > 15m or alive for > 1h.
pub(crate) async fn prune_inactive_free_vms_worker(state: &AppState) {
    let now = now_ms() as f64;
    let max_inactive_ms = 15.0 * 60.0 * 1000.0;
    let max_lifespan_ms = 60.0 * 60.0 * 1000.0;

    let to_prune: Vec<(String, f64, bool, bool)> = {
        let vms = state
            .active_free_vms
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        vms.iter()
            .filter_map(|(email, entry)| {
                let last_active = entry
                    .get("lastActive")
                    .and_then(jsval::number)
                    .unwrap_or(0.0);
                let started_at = entry
                    .get("startedAt")
                    .and_then(jsval::number)
                    .unwrap_or(0.0);
                let vmid = entry.get("vmid").and_then(jsval::number).unwrap_or(0.0);
                let is_inactive = (now - last_active) > max_inactive_ms;
                let is_expired = (now - started_at) > max_lifespan_ms;
                if is_inactive || is_expired {
                    Some((email.clone(), vmid, is_inactive, is_expired))
                } else {
                    None
                }
            })
            .collect()
    };

    for (email, vmid, is_inactive, is_expired) in to_prune {
        println!(
            "[free-vm] Pruning free VM {} for {} (inactive: {}, expired: {})",
            vmid as i64, email, is_inactive, is_expired
        );
        let _ = crate::routes::vm::terminate_user_vm(state, vmid).await;
        state
            .active_free_vms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&email);
    }
}

/// `enforceVmMaxUptimeWorker()` (server.js:27874-27986) — power down running
/// non-admin VMs exceeding daily / session limit or 10-minute off-page inactivity.
pub(crate) async fn enforce_vm_max_uptime_worker(state: &AppState) {
    if !crate::proxmox_desktop::desktop().configured() {
        return;
    }
    let guests = match crate::proxmox_desktop::desktop().list_guests().await {
        Ok(g) => g,
        Err(_) => return,
    };

    let apps_data = crate::routes::vm::vm_applications(state);

    for guest in guests {
        let is_template = guest.get("template").map(jsval::truthy).unwrap_or(false);
        let status = jsval::str_or(guest.get("status"), "");
        if is_template {
            continue;
        }

        let vmid = match guest.get("vmid").and_then(jsval::number) {
            Some(v) => v,
            None => continue,
        };

        let record_opt = vmlib::get_virtual_machine_by_vmid(&state.store, vmid);
        let record = record_opt.as_ref().map(|r| r.to_json());
        let record_key = record_opt
            .as_ref()
            .map(|r| r.id.clone())
            .unwrap_or_else(|| format!("vmid-{}", vmid as i64));

        if status != "running" {
            state
                .vm_shutdown_initiated
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&record_key);
            continue;
        }

        let mut owner_email = record_opt
            .as_ref()
            .map(|r| r.owner_email.clone())
            .unwrap_or_default();
        if owner_email.is_empty() {
            let vms = state
                .active_free_vms
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            for (email, entry) in vms.iter() {
                if entry.get("vmid").and_then(jsval::number) == Some(vmid) {
                    owner_email = email.clone();
                    break;
                }
            }
        }
        if owner_email.is_empty() {
            if let Some(obj) = apps_data.as_object() {
                for (email, app) in obj {
                    if app.get("vmid").and_then(jsval::number) == Some(vmid) {
                        owner_email = jsval::str_or(app.get("email"), email.as_str());
                        break;
                    }
                }
            }
        }

        // Admins are exempt from VM time limits
        if !owner_email.is_empty() && mitch_lib::auth::is_admin_email(&state.store, &owner_email) {
            continue;
        }

        let now = now_ms() as i64;
        let uptime = guest.get("uptime").and_then(jsval::number).unwrap_or(0.0);
        let lease =
            crate::routes::vm::get_vm_lease(state, &record_key, uptime, false, &owner_email);

        let mut has_open_socket = false;
        {
            let sockets = state
                .vm_desktop_sockets
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            for client in sockets.values() {
                let d = &client.data;
                let rec_id = jsval::str_or(d.get("recordId"), "");
                let sock_vmid = d.get("vmid").and_then(jsval::number);
                if rec_id == record_key || sock_vmid == Some(vmid) {
                    has_open_socket = true;
                    break;
                }
            }
        }

        if has_open_socket {
            state
                .vm_page_presence
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(record_key.clone(), now);
            state
                .vm_shutdown_initiated
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&record_key);
        }

        let is_lease_exempt = lease.get("isExempt").map(jsval::truthy).unwrap_or(false);
        let remaining_seconds = lease.get("remainingSeconds").and_then(jsval::number);
        let daily_remaining = lease
            .get("dailyRemainingSeconds")
            .and_then(jsval::number)
            .unwrap_or(f64::NAN);
        let is_daily_limit = daily_remaining.is_finite() && daily_remaining <= 0.0;
        let max_uptime = lease
            .get("maxUptimeSeconds")
            .and_then(jsval::number)
            .unwrap_or(0.0);

        // Daily / session uptime expiration (only for non-exempt users)
        let is_uptime_expired = !is_lease_exempt && remaining_seconds.map(|s| s <= 0.0).unwrap_or(false);

        // Inactivity expiration (applies to ALL users, even if they bought unlimited time)
        let mut is_inactive = false;
        let mut inactive_ms: u64 = 0;
        if !has_open_socket {
            let presence_last = state
                .vm_page_presence
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&record_key)
                .copied();
            let last_seen = presence_last.unwrap_or_else(|| {
                let past = (now as f64) - (uptime * 1000.0);
                if past >= 0.0 {
                    past as i64
                } else {
                    now
                }
            });
            if presence_last.is_none() {
                state
                    .vm_page_presence
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(record_key.clone(), last_seen);
            }

            inactive_ms = (now - last_seen).max(0) as u64;
            if is_vm_inactive(last_seen as f64, now as f64) {
                is_inactive = true;
            }
        }

        let shutdown_initiated = state
            .vm_shutdown_initiated
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&record_key)
            .copied();

        if is_inactive || is_uptime_expired || shutdown_initiated.is_some() {
            let shutdown_reason = if shutdown_initiated.is_some() {
                if is_inactive {
                    "inactivity_10m"
                } else if is_daily_limit {
                    "daily_limit_reached"
                } else if is_uptime_expired {
                    "max_uptime_reached"
                } else {
                    "shutdown_timeout_10m"
                }
            } else if is_inactive {
                "inactivity_10m"
            } else if is_daily_limit {
                "daily_limit_reached"
            } else {
                "max_uptime_reached"
            };

            let force_timeout_ms: i64 = 10 * 60 * 1000; // 10 minutes
            let must_force_stop = match shutdown_initiated {
                Some(initiated_at) => (now - initiated_at) >= force_timeout_ms,
                None => false,
            };

            if must_force_stop {
                let elapsed_ms = shutdown_initiated.map(|t| now - t).unwrap_or(0);
                println!(
                    "[vm-watchdog] VM {} (VMID {}) did not shut off in 10m after shutdown signal (elapsed: {}m). Force stopping...",
                    record_key,
                    vmid as i64,
                    (elapsed_ms as f64 / 60000.0).round() as i64
                );

                if let Some(rec) = record.as_ref() {
                    crate::routes::vm::vm_audit(
                        state,
                        "system",
                        Some(rec),
                        "VM_SHUTDOWN_FORCE_STOP",
                        true,
                        Some(&json!({
                            "elapsedSeconds": elapsed_ms / 1000,
                            "reason": shutdown_reason,
                            "vmid": vmid,
                        })),
                    );
                    crate::routes::vm::revoke_vm_desktop_connections(state, &record_key);
                    let _ = crate::proxmox_desktop::desktop()
                        .power(rec, "force-stop")
                        .await;
                } else {
                    let synthetic = json!({
                        "vmid": vmid,
                        "node": guest.get("node"),
                        "guestType": guest.get("type"),
                    });
                    let _ = crate::proxmox_desktop::desktop()
                        .power(&synthetic, "force-stop")
                        .await;
                }

                state
                    .vm_shutdown_initiated
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&record_key);
                state
                    .vm_leases
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&record_key);
                state
                    .vm_page_presence
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&record_key);
            } else if shutdown_initiated.is_none() {
                // First detection: initiate graceful shutdown and record timestamp
                state
                    .vm_shutdown_initiated
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(record_key.clone(), now);

                println!(
                    "[vm-watchdog] VM {} (VMID {}) reached {} (inactive: {}m). Requesting graceful shutdown...",
                    record_key,
                    vmid as i64,
                    shutdown_reason,
                    (inactive_ms as f64 / 60000.0).round() as i64
                );

                if let Some(rec) = record.as_ref() {
                    crate::routes::vm::vm_audit(
                        state,
                        "system",
                        Some(rec),
                        if is_inactive {
                            "VM_SHUTDOWN_INACTIVITY"
                        } else if is_daily_limit {
                            "VM_SHUTDOWN_DAILY_LIMIT"
                        } else {
                            "VM_SHUTDOWN_TIMEOUT"
                        },
                        true,
                        Some(&json!({
                            "uptime": uptime,
                            "maxUptimeSeconds": max_uptime,
                            "inactiveSeconds": inactive_ms / 1000,
                            "reason": shutdown_reason,
                        })),
                    );
                    crate::routes::vm::revoke_vm_desktop_connections(state, &record_key);
                    if let Err(err) = crate::proxmox_desktop::desktop()
                        .power(rec, "shutdown")
                        .await
                    {
                        tracing::warn!(
                            "[vm-watchdog] Graceful shutdown failed immediately for {}, force stopping: {:?}",
                            record_key,
                            err
                        );
                        let _ = crate::proxmox_desktop::desktop()
                            .power(rec, "force-stop")
                            .await;
                    }
                    if !owner_email.is_empty() {
                        crate::routes::vm::trigger_vm_cooldown(
                            state,
                            &owner_email,
                            shutdown_reason,
                        );
                    }
                } else {
                    let synthetic = json!({
                        "vmid": vmid,
                        "node": guest.get("node"),
                        "guestType": guest.get("type"),
                    });
                    if crate::proxmox_desktop::desktop()
                        .power(&synthetic, "shutdown")
                        .await
                        .is_err()
                    {
                        let _ = crate::proxmox_desktop::desktop()
                            .power(&synthetic, "force-stop")
                            .await;
                    }
                }
            } else {
                // Graceful shutdown has already been sent, waiting for VM to shut off (up to 10m).
            }
        } else {
            // VM is active / not expired: ensure no stale shutdown_initiated timestamp
            state
                .vm_shutdown_initiated
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&record_key);
        }
    }
}

/// Generates a dedicated portal SSH key pair if not already present on disk (server.js:28513-28539).
fn init_portal_ssh_key(data_dir: &Path) {
    let priv_key = data_dir.join("portal_id_rsa");
    let pub_key = data_dir.join("portal_id_rsa.pub");
    if !priv_key.exists() {
        println!("[ssh-init] Generating dedicated portal SSH key pair...");
        let status = std::process::Command::new("ssh-keygen")
            .args(["-t", "rsa", "-b", "2048", "-N", "", "-f"])
            .arg(&priv_key)
            .status();
        match status {
            Ok(s) if s.success() => {
                println!("[ssh-init] Portal SSH key pair generated successfully.");
            }
            Ok(s) => {
                eprintln!(
                    "[ssh-init] Failed to generate portal SSH key pair: exit status {}",
                    s
                );
                return;
            }
            Err(e) => {
                eprintln!("[ssh-init] Failed to generate portal SSH key pair: {}", e);
                return;
            }
        }
    }
    if let Ok(pub_content) = std::fs::read_to_string(&pub_key) {
        let trimmed = pub_content.trim();
        println!("\n========================================================================");
        println!("[SSH GATEWAY SECURITY KEY]");
        println!("To allow the web application to automatically configure new containers,");
        println!("please add the following public key to your Proxmox host /root/.ssh/authorized_keys file:");
        println!("\n{}\n", trimmed);
        println!("========================================================================\n");
    } else {
        eprintln!("[ssh-init] Failed to read portal public SSH key");
    }
}

/// Checks for leftover ephemeral containers (200..210) on Proxmox on startup (server.js:28541-28558).
async fn cleanup_all_ephemeral_vms(state: &Arc<AppState>) {
    println!("[startup] Checking for leftover ephemeral containers on Proxmox...");
    let existing_ids = crate::routes::vm::get_existing_vmids().await;
    for id in 200..210 {
        if existing_ids.contains_key(&id) {
            println!("[startup] Leftover ephemeral VMID {id} detected. Wiping...");
            let state_c = Arc::clone(state);
            tokio::spawn(async move {
                let _ = crate::routes::vm::terminate_user_vm(&state_c, id as f64).await;
            });
        }
    }
    println!("[startup] Leftover ephemeral containers cleanup sequence completed.");
}

/// Spawns all VM background worker loops and runs the boot sequence.
pub fn spawn(state: Arc<AppState>) {
    // 1. Boot-time database & legacy migration
    vmlib::prune_old_vm_usage_samples(&state.store, 30.0);
    crate::routes::vm::migrate_legacy_vm_ownership(&state);
    init_portal_ssh_key(state.data_dir());

    {
        let state_c = Arc::clone(&state);
        tokio::spawn(async move {
            cleanup_all_ephemeral_vms(&state_c).await;
        });
    }

    // 2. sampleVmUsageWorker — every 60s + immediate run
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            sample_vm_usage_worker(&state).await;
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                sample_vm_usage_worker(&state).await;
            }
        });
    }

    // 3. purgeExpiredVmsWorker — every 3600s + immediate run
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            purge_expired_vms_worker(&state).await;
            let mut interval = tokio::time::interval(Duration::from_secs(3600));
            loop {
                interval.tick().await;
                purge_expired_vms_worker(&state).await;
            }
        });
    }

    // 4. pruneInactiveFreeVmsWorker — every 300s + immediate run
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            prune_inactive_free_vms_worker(&state).await;
            let mut interval = tokio::time::interval(Duration::from_secs(300));
            loop {
                interval.tick().await;
                prune_inactive_free_vms_worker(&state).await;
            }
        });
    }

    // 5. enforceVmMaxUptimeWorker — every 15s
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(15));
            loop {
                interval.tick().await;
                enforce_vm_max_uptime_worker(&state).await;
            }
        });
    }

    // 6. cleanupVmDesktopSessions — every 5s + immediate run
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            crate::routes::vm::cleanup_vm_desktop_sessions(&state);
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            loop {
                interval.tick().await;
                crate::routes::vm::cleanup_vm_desktop_sessions(&state);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vm_inactivity_threshold() {
        let now = 1_000_000_000_000.0;
        // 9 minutes ago -> not inactive
        let active_9m = now - (9.0 * 60.0 * 1000.0);
        assert!(!is_vm_inactive(active_9m, now));

        // 10 minutes ago -> inactive
        let inactive_10m = now - (10.0 * 60.0 * 1000.0);
        assert!(is_vm_inactive(inactive_10m, now));

        // 15 minutes ago -> inactive
        let inactive_15m = now - (15.0 * 60.0 * 1000.0);
        assert!(is_vm_inactive(inactive_15m, now));
    }

    #[test]
    fn test_force_stop_timeout() {
        let now: i64 = 1_000_000_000_000;
        let force_timeout_ms: i64 = 10 * 60 * 1000; // 10 minutes

        // 5 minutes since shutdown initiated -> must NOT force stop
        let initiated_5m = now - (5 * 60 * 1000);
        assert!((now - initiated_5m) < force_timeout_ms);

        // 9m59s since shutdown initiated -> must NOT force stop
        let initiated_almost_10m = now - (9 * 60 * 1000 + 59 * 1000);
        assert!((now - initiated_almost_10m) < force_timeout_ms);

        // Exactly 10 minutes -> MUST force stop
        let initiated_10m = now - (10 * 60 * 1000);
        assert!((now - initiated_10m) >= force_timeout_ms);

        // 12 minutes -> MUST force stop
        let initiated_12m = now - (12 * 60 * 1000);
        assert!((now - initiated_12m) >= force_timeout_ms);
    }
}
