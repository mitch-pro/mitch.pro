//! `/api/vm/*` + Proxmox (plan Step 13).
//!
//! Batch 2 carries the slice the `/ssh/ws` bridge needs for non-admin
//! authorization (server.js:25786-25927): the PVE configuration, the VMID
//! range gate, the VM type resolution, the per-user status/IP resolution and
//! the `activeFreeVms` registry read. The provisioning endpoints
//! (`initializeWebVm`/`terminateUserVm`/`getExistingVmids`/the free-VM
//! pruner) land with batch 3's `/api/vm/*` group over this same slice.

// The slice is intentionally unwired until batch 3 — the /ssh/ws module (this
// batch) calls into it once written; the rest stays dead until then.
#![allow(dead_code)]

use crate::state::AppState;
use axum::http::Method;
use axum::response::Response;
use mitch_lib::jsval;
use serde_json::{json, Value};
use std::sync::Arc;

// ── Proxmox configuration (server.js:25518-25522, 25787-25788) ───────────────

/// `PVE_URL` — env value first, then the tartarus default.
pub(crate) fn pve_url() -> String {
    std::env::var("PVE_URL").unwrap_or_else(|_| "https://192.168.100.1:8006/api2/json".to_string())
}

/// `PVE_TOKEN` — "PVEAPIToken=api-helper@pve!token-id=…"; empty means the
/// Proxmox surface is unconfigured (the JS gates status fetches on it).
pub(crate) fn pve_token() -> String {
    std::env::var("PVE_TOKEN").unwrap_or_default()
}

/// `PVE_NODE` — the production host.
pub(crate) fn pve_node() -> String {
    std::env::var("PVE_NODE").unwrap_or_else(|_| "tartarus".to_string())
}

/// `PVE_VMID_MIN` / `PVE_VMID_MAX` (server.js:25787-25788).
pub(crate) const PVE_VMID_MIN: f64 = 200.0;
pub(crate) const PVE_VMID_MAX: f64 = 999.0;

/// `isVmIdInRange` (server.js:25797-25799).
pub(crate) fn is_vm_id_in_range(vmid: f64) -> bool {
    vmid.is_finite() && (PVE_VMID_MIN..=PVE_VMID_MAX).contains(&vmid)
}

/// `loadJson(VM_APPS_FILE, {})` (server.js:319).
pub(crate) fn vm_applications(state: &AppState) -> Value {
    state
        .store
        .read_document(&state.data_dir().join("vm_applications.json"), json!({}))
}

/// A PVE API client: self-signed certs on the tartarus interface, so the JS
/// `tls: { rejectUnauthorized: false }` maps to `danger_accept_invalid_certs`.
fn pve_client(deadline: u64) -> reqwest::Client {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_millis(deadline))
        .build()
        .unwrap_or_default()
}

async fn pve_get_json(url: &str, deadline: u64) -> Result<Value, String> {
    pve_client(deadline)
        .get(url)
        .header("Authorization", pve_token())
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json::<Value>()
        .await
        .map_err(|e| e.to_string())
}

/// `getVmTypeByVmid` (server.js:25831-25849) — activeFreeVms first, then
/// vm_applications.json (premium → lxc, else qemu), then the range heuristic.
pub(crate) fn get_vm_type_by_vmid(state: &AppState, vmid: f64) -> &'static str {
    for entry in state
        .active_free_vms
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
    {
        if entry.get("vmid").and_then(jsval::number) == Some(vmid) {
            return "lxc";
        }
    }
    let apps = vm_applications(state);
    if let Some(map) = apps.as_object() {
        for app in map.values() {
            if app.get("vmid").and_then(jsval::number) == Some(vmid) {
                return if jsval::string(&jsval::or(app.get("tier"), json!(""))) == "premium" {
                    "lxc"
                } else {
                    "qemu"
                };
            }
        }
    }
    if (200.0..400.0).contains(&vmid) {
        "lxc"
    } else {
        "qemu"
    }
}

/// `getUserVmStatus` (server.js:25851-25907) — returns the JS shape:
/// `{success:true, status, ip}` or `{success:false, error}`. `None` would be
/// the JS null-from-deadline case; the JS folds timeouts into the error
/// objects shown below, which is what we return too.
pub(crate) async fn get_user_vm_status(state: &AppState, vmid: f64) -> Value {
    let token = pve_token();
    if token.is_empty() {
        return json!({ "success": false, "error": "Proxmox token not configured." });
    }
    let vmid_int = if vmid >= 0.0 && vmid.fract() == 0.0 {
        vmid as i64
    } else {
        return json!({ "success": false, "error": "invalid vmid" });
    };
    let type_ = get_vm_type_by_vmid(state, vmid);
    let url = format!(
        "{}/nodes/{}/{type_}/{vmid_int}/status/current",
        pve_url(),
        pve_node()
    );
    let res = match pve_client(5000)
        .get(&url)
        .header("Authorization", &token)
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => {
            // fetchWithDeadline null → 'Proxmox status fetch timed out'.
            return json!({ "success": false, "error": "Proxmox status fetch timed out" });
        }
    };
    if !res.status().is_success() {
        return json!({
            "success": false,
            "error": format!("Failed to fetch status: {}", res.status().as_u16())
        });
    }
    let data = match res.json::<Value>().await {
        Ok(v) => v,
        Err(e) => {
            return json!({ "success": false, "error": e.to_string() });
        }
    };

    let mut ip = String::new();
    if type_ == "lxc" {
        let suffix = if vmid >= 300.0 { vmid - 200.0 } else { vmid } as i64;
        ip = format!("10.0.0.{suffix}");
    } else if data
        .get("data")
        .and_then(|d| d.get("status"))
        .and_then(|s| s.as_str())
        == Some("running")
    {
        // QEMU guest agent IP probe (3s deadline).
        let agent_url = format!(
            "{}/nodes/{}/qemu/{vmid_int}/agent/network-get-interfaces",
            pve_url(),
            pve_node()
        );
        if let Ok(agent_res) = pve_client(3000)
            .get(&agent_url)
            .header("Authorization", &token)
            .send()
            .await
        {
            if agent_res.status().is_success() {
                if let Ok(agent_data) = agent_res.json::<Value>().await {
                    let result = agent_data
                        .get("data")
                        .and_then(|d| d.get("result"))
                        .and_then(|r| r.as_array())
                        .cloned()
                        .unwrap_or_default();
                    'outer: for iface in result {
                        let addrs = iface
                            .get("ip-addresses")
                            .and_then(|a| a.as_array())
                            .cloned()
                            .unwrap_or_default();
                        for addr in addrs {
                            let is_v4 = addr.get("ip-address-type").and_then(|t| t.as_str())
                                == Some("ipv4");
                            let ip_str = addr
                                .get("ip-address")
                                .and_then(|i| i.as_str())
                                .unwrap_or("");
                            if is_v4 && ip_str.starts_with("10.0.0.") {
                                ip = ip_str.to_string();
                                break 'outer;
                            }
                        }
                    }
                }
            }
        }
    }

    let suffix = if vmid >= 300.0 { vmid - 200.0 } else { vmid } as i64;
    let fallback_ip = format!("10.0.0.{suffix}");
    let status = data
        .get("data")
        .and_then(|d| d.get("status"))
        .and_then(|s| s.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| "unknown".to_string());
    json!({
        "success": true,
        "status": status,
        "ip": if ip.is_empty() { fallback_ip } else { ip }
    })
}

/// `getVmConnectionIpForEmail` (server.js:25908-25927) — resolves the SSH/VNC
/// authorization IP for a user's VM: activeFreeVms, then an approved
/// vm_applications entry; out-of-range or unassigned → '' (denied).
pub(crate) async fn get_vm_connection_ip_for_email(state: &AppState, email: &str) -> String {
    let norm = mitch_lib::auth::normalize_email(email);
    let mut vmid: Option<f64> = None;
    {
        let free = state
            .active_free_vms
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = free.get(&norm) {
            vmid = entry.get("vmid").and_then(jsval::number);
        }
    }
    if vmid.is_none() {
        let apps = vm_applications(state);
        let app = apps.get(&norm);
        if let Some(app) = app {
            if jsval::string(&jsval::or(app.get("status"), json!(""))) == "approved" {
                let candidate = app.get("vmid").and_then(jsval::number);
                if candidate.is_some() {
                    vmid = candidate;
                }
            }
        }
    }
    let Some(vmid) = vmid.filter(|v| is_vm_id_in_range(*v)) else {
        return String::new();
    };

    // KVM guests use DHCP — resolve the real address through the guest agent
    // before authorizing the bridge; LXC keeps the deterministic fallback.
    let status = get_user_vm_status(state, vmid).await;
    let ip = status
        .get("ip")
        .and_then(|i| i.as_str())
        .unwrap_or("")
        .to_string();
    if status
        .get("success")
        .and_then(|s| s.as_bool())
        .unwrap_or(false)
        && js_is_10_ip(&ip)
    {
        return ip;
    }
    let suffix = if vmid >= 300.0 { vmid - 200.0 } else { vmid } as i64;
    format!("10.0.0.{suffix}")
}

/// `/^10\.0\.0\.\d{1,3}$/` (server.js:25922).
fn js_is_10_ip(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("10.0.0.") else {
        return false;
    };
    !rest.is_empty() && rest.len() <= 3 && rest.bytes().all(|b| b.is_ascii_digit())
}

// ── batch 4: the /api/vm route layer (server.js:18849-19809) + helpers ───────

use crate::routes::me::json_response;
use axum::extract::ws::{CloseFrame, WebSocketUpgrade};
use axum::http::HeaderMap;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use mitch_lib::vm as vmlib;
use mitch_lib::vm_security::{
    can_user_extend, compute_cooldown_remaining, format_uptime_duration,
    get_remaining_daily_vm_seconds, is_daily_vm_limit_reached, validate_desktop_session,
    VM_COOLDOWN_DURATION_MS, VM_DAILY_MAX_SECONDS, VM_DEFAULT_CPU_CORES, VM_DEFAULT_DISK_GB,
    VM_DEFAULT_MEMORY_MB, VM_FLEET_MAX_CORES, VM_FLEET_MAX_MEMORY_MB, VM_MAX_CONCURRENT_RUNNING,
};
use std::sync::atomic::Ordering;
use std::time::Duration;

// ── constants (server.js:27087-27090) ────────────────────────────────────────

/// `VM_DESKTOP_SESSION_TTL_MS` (server.js:27087).
pub(crate) const VM_DESKTOP_SESSION_TTL_MS: f64 = 75_000.0;
/// `VM_BASE_MAX_UPTIME_SECONDS` (server.js:27088) — 6 hours maximum daily
/// uptime.
pub(crate) const VM_BASE_MAX_UPTIME_SECONDS: f64 = 6.0 * 3600.0;
/// `VM_MAX_EXTENSION_SECONDS` (server.js:27089) — 30 minutes maximum
/// extension.
pub(crate) const VM_MAX_EXTENSION_SECONDS: f64 = 1800.0;

/// The JS `now` everywhere in this module is `Date.now()` — server-local
/// epoch milliseconds.
pub(crate) fn now_ms() -> i64 {
    mitch_lib::school::now_millis()
}

// ── shared JS micro-semantics ────────────────────────────────────────────────

/// `Number(v) || fallback` — 0/NaN fall to the fallback.
fn js_num_or(v: Option<&Value>, fallback: f64) -> f64 {
    match v.and_then(jsval::number) {
        Some(n) if n != 0.0 && n.is_finite() => n,
        _ => fallback,
    }
}

/// JS spread `{...base, ...over}` — shallow object merge, `over` wins.
fn js_object_merge(base: &Value, over: &Value) -> Value {
    let mut merged = base.clone();
    if let (Some(m), Some(o)) = (merged.as_object_mut(), over.as_object()) {
        for (k, v) in o {
            m.insert(k.clone(), v.clone());
        }
    }
    merged
}

/// `Math.random().toString(36).slice(-10)` — 10 lowercase base-36 digits of
/// a uniform random fraction. (JS picks the last 10 digits of the shortest
/// round-trip base-36 expansion; both are nondeterministic [0-9a-z] draws,
/// which is all any consumer relies on.)
pub(crate) fn js_random_password() -> String {
    let r = mitch_lib::crypto::js_random();
    let mut n = (r * (1u64 << 52) as f64) as u128;
    let mut s = String::new();
    for _ in 0..10 {
        s.push(char::from_digit((n % 36) as u32, 36).unwrap_or('0'));
        n /= 36;
    }
    s
}

// ── the desktop-bridge socket registry entry ─────────────────────────────────

/// One live `/api/vm/desktop/ws` bridge socket. Carries the JS `ws.data`
/// subset the authorization check, fleet stats and workers read, plus a
/// close channel so other tasks can force `ws.close(1008, …)` the way the
/// JS does. The owning bridge task removes itself from
/// `state.vm_desktop_sockets` on close.
pub(crate) struct VmDesktopClient {
    pub data: Value,
    pub close_tx: tokio::sync::mpsc::UnboundedSender<(u16, String)>,
}

// ── friendlyVmError ──────────────────────────────────────────────────────────

/// The error shapes `friendlyVmError` (server.js:27775-27789) distinguishes:
/// a `ProxmoxServiceError` vs everything else (`{message, code}`).
pub(crate) enum VmError {
    Service(crate::proxmox_desktop::ProxmoxServiceError),
    Generic {
        message: String,
        code: Option<String>,
    },
}

impl From<crate::proxmox_desktop::ProxmoxServiceError> for VmError {
    fn from(e: crate::proxmox_desktop::ProxmoxServiceError) -> Self {
        VmError::Service(e)
    }
}

/// `friendlyVmError(error)` (server.js:27775-27789) → (status, error, code).
pub(crate) fn friendly_vm_error(error: &VmError) -> (u16, String, String) {
    fn or_default(msg: &str, fallback: &str) -> String {
        if msg.is_empty() {
            fallback.to_string()
        } else {
            msg.to_string()
        }
    }
    match error {
        VmError::Service(e) => match e.code {
            "STOPPED" => (
                409,
                "Your computer is currently offline. Start it and try again.".to_string(),
                "computer_offline".to_string(),
            ),
            "TIMEOUT" | "TASK_TIMEOUT" => (
                504,
                "Your computer is still starting. Try again in a moment.".to_string(),
                "computer_starting".to_string(),
            ),
            "INVALID_ACTION" | "INVALID_VM" | "INVALID_TEMPLATE" => (
                400,
                or_default(&e.message, "That computer request is not valid."),
                "invalid_request".to_string(),
            ),
            "INVALID_DESKTOP_LOGIN" => (
                400,
                or_default(
                    &e.message,
                    "Choose a desktop username and a password of 8 to 128 characters.",
                ),
                "invalid_desktop_login".to_string(),
            ),
            "NO_CAPACITY" => (
                409,
                "No computer slots are available right now.".to_string(),
                "no_capacity".to_string(),
            ),
            "NO_GRAPHICAL_DESKTOP" => (
                409,
                "This machine does not have a graphical desktop.".to_string(),
                "desktop_unavailable".to_string(),
            ),
            "GUEST_SETUP_FAILED" => (
                504,
                or_default(&e.message, "The graphical desktop did not finish starting."),
                "guest_setup_failed".to_string(),
            ),
            "TASK_FAILED" => (
                502,
                or_default(&e.message, "The computer task failed."),
                "task_failed".to_string(),
            ),
            "UPSTREAM_REJECTED" => {
                let status = if e.status == 0 { 502 } else { e.status };
                (
                    status,
                    "Your computer could not be reached.".to_string(),
                    "computer_unreachable".to_string(),
                )
            }
            _ => {
                let status = if e.status == 0 { 502 } else { e.status };
                (
                    status,
                    or_default(&e.message, "Your computer could not be reached."),
                    e.code.to_lowercase(),
                )
            }
        },
        VmError::Generic { message, code } => (
            502,
            or_default(message, "Your computer could not be reached."),
            code.clone()
                .unwrap_or_else(|| "computer_unreachable".to_string()),
        ),
    }
}

// ── the authenticated VM actor (server.js:27758-27764) ───────────────────────

pub(crate) struct VmActor {
    pub sid: String,
    pub email: String,
    pub is_admin: bool,
    pub is_owner: bool,
    pub auth_session_key: String,
}

impl VmActor {
    /// The actor as a `canAccessVmRecord`/`validateDesktopSession` payload.
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "sid": self.sid,
            "email": self.email,
            "isAdmin": self.is_admin,
            "isOwner": self.is_owner,
            "authSessionKey": self.auth_session_key,
        })
    }
}

/// `isRevoked(id)` — key presence in `data/revoked.json`.
pub(crate) fn is_revoked_id(state: &AppState, sid: &str) -> bool {
    state
        .store
        .read_document(&state.data_dir().join("revoked.json"), json!({}))
        .get(sid)
        .is_some()
}

/// `authenticatedVmActor(req)` — the `studentId||id` cookie ladder with the
/// password-cookie check the legacy vm routes skip.
pub(crate) fn authenticated_vm_actor(state: &AppState, headers: &HeaderMap) -> Option<VmActor> {
    let node_env_test = std::env::var("NODE_ENV").unwrap_or_default() == "test";
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cookies.get("id").unwrap_or(""))
        .to_string();
    if sid.is_empty()
        || !mitch_lib::auth::valid_id(&sid, &state.id_secret)
        || is_revoked_id(state, &sid)
        || !mitch_lib::auth::check_password_cookie(
            &state.store,
            &state.id_secret,
            &cookies,
            Some(&sid),
            node_env_test,
            mitch_lib::auth::dev_test_access_enabled(),
        )
    {
        return None;
    }
    let email = mitch_lib::auth::normalize_email(
        mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid)
            .unwrap_or_default()
            .as_str(),
    );
    if email.is_empty() {
        return None;
    }
    let is_admin =
        mitch_lib::auth::is_admin_id(&state.store, &state.id_secret, &sid, node_env_test);
    let is_owner = mitch_lib::auth::is_owner_email(&state.store, &email);
    Some(VmActor {
        sid,
        email,
        is_admin,
        is_owner,
        auth_session_key: cookies
            .get(mitch_lib::auth::AUTH_COOKIE)
            .filter(|t| !t.is_empty())
            .map(mitch_lib::auth::hash_session_token)
            .unwrap_or_default(),
    })
}

/// `vmSameOriginRequest(req)` (server.js:27766-27769) — Origin / Referer header
/// check with port normalization and support for Mitch SSO hosts and AJAX requests.
pub(crate) fn vm_same_origin_request(headers: &HeaderMap) -> bool {
    let raw = headers
        .get(axum::http::header::ORIGIN)
        .or_else(|| headers.get(axum::http::header::REFERER))
        .and_then(|v| v.to_str().ok());
    let Some(raw) = raw else {
        return headers.get("x-mitch-requested-with").is_some()
            || headers.get("x-admin-passphrase").is_some();
    };
    let Ok(origin) = url::Url::parse(raw) else {
        return false;
    };
    let Some(origin_host) = origin.host_str() else {
        return false;
    };
    let origin_host = origin_host.to_lowercase();
    let req_host = crate::hosts::request_host(headers).to_lowercase();
    let req_host_bare = req_host.split(':').next().unwrap_or("").to_lowercase();

    let origin_with_port = match origin.port() {
        Some(port) => format!("{}:{}", origin_host, port),
        None => origin_host.clone(),
    };

    if origin_with_port == req_host || origin_host == req_host || origin_host == req_host_bare {
        return true;
    }

    let is_trusted_host = |h: &str| {
        h == "mitchdog.com"
            || h.ends_with(".mitchdog.com")
            || h == "mitch.pro"
            || h.ends_with(".mitch.pro")
            || h == "rjuhsd.school"
            || h.ends_with(".rjuhsd.school")
            || h == "sexypickleclub.com"
            || h.ends_with(".sexypickleclub.com")
            || h == "localhost"
            || h == "127.0.0.1"
            || h.starts_with("webserver")
            || h.is_empty()
    };

    if is_trusted_host(&origin_host) && is_trusted_host(&req_host_bare) {
        return true;
    }

    false
}

/// `vmRecordAllowedForActor(record, actor)` (server.js:27771-27773) —
/// canAccessVmRecord with the plain `isAdminEmail` option (no admin-grant
/// requirement) and owner bypass.
pub(crate) fn vm_record_allowed_for_actor(state: &AppState, record: &Value, actor: &Value) -> bool {
    if actor.get("isOwner").map(jsval::truthy).unwrap_or(false) {
        return true;
    }
    mitch_lib::vm_security::can_access_vm_record(
        Some(record),
        Some(actor),
        |email| mitch_lib::auth::is_admin_email(&state.store, email),
        false,
        None,
    )
}

// ── the data-dir JSON helpers ────────────────────────────────────────────────

/// `loadJson(join(DATA_DIR, name), default)`.
pub(crate) fn load_vm_json(state: &AppState, name: &str, default: Value) -> Value {
    state
        .store
        .read_document(&state.data_dir().join(name), default)
}

/// `saveJson(join(DATA_DIR, name), doc)`.
pub(crate) fn save_vm_json(state: &AppState, name: &str, doc: &Value) {
    let _ = state
        .store
        .write_document(&state.data_dir().join(name), doc);
}

// ── vmAudit (server.js:27746-27756) ──────────────────────────────────────────

pub(crate) fn vm_audit(
    state: &AppState,
    actor_email: &str,
    record: Option<&Value>,
    action: &str,
    success: bool,
    details: Option<&Value>,
) {
    let owner = record
        .and_then(|r| r.get("ownerEmail"))
        .filter(|v| jsval::truthy(v))
        .map(jsval::string)
        .unwrap_or_default();
    let id = record
        .and_then(|r| r.get("id"))
        .filter(|v| jsval::truthy(v))
        .map(jsval::string)
        .unwrap_or_default();
    let vmid = record.and_then(|r| r.get("vmid")).and_then(jsval::number);
    let actor = if actor_email.is_empty() {
        "system"
    } else {
        actor_email
    };
    let actor = mitch_lib::auth::normalize_email(actor);
    vmlib::append_vm_audit_log(
        &state.store,
        now_ms() as f64,
        &actor,
        &owner,
        &id,
        vmid,
        action,
        success,
        details,
    );
}

// ── admin grants (server.js:27099-27175) ─────────────────────────────────────

/// `getVmAdminGrant(recordId)`.
pub(crate) fn get_vm_admin_grant(state: &AppState, record_id: &str) -> Value {
    if record_id.is_empty() {
        return json!({ "allowed": false, "requested": false });
    }
    let grants = load_vm_json(state, "vm_admin_grants.json", json!({}));
    match grants.get(record_id) {
        Some(v) if !v.is_null() => v.clone(),
        _ => json!({ "allowed": false, "requested": false }),
    }
}

/// `isVmAdminAccessAllowed(recordId)`.
pub(crate) fn is_vm_admin_access_allowed(state: &AppState, record_id: &str) -> bool {
    if record_id.is_empty() {
        return false;
    }
    get_vm_admin_grant(state, record_id)
        .get("allowed")
        .map(jsval::truthy)
        .unwrap_or(false)
}

/// `isVmAdminAccessRequested(recordId)`.
pub(crate) fn is_vm_admin_access_requested(state: &AppState, record_id: &str) -> bool {
    if record_id.is_empty() {
        return false;
    }
    get_vm_admin_grant(state, record_id)
        .get("requested")
        .map(jsval::truthy)
        .unwrap_or(false)
}

/// `setVmAdminAccess(recordId, ownerEmail, allowed, actorEmail)`
/// (server.js:27115-27133).
pub(crate) fn set_vm_admin_access(
    state: &AppState,
    record_id: &str,
    owner_email: &str,
    allowed: bool,
    actor_email: &str,
) {
    if record_id.is_empty() {
        return;
    }
    let mut grants = load_vm_json(state, "vm_admin_grants.json", json!({}));
    let id = record_id.to_string();
    let current = grants.get(&id).cloned().unwrap_or_else(|| json!({}));
    let now = now_ms();
    let allowed_at = if allowed {
        json!(now)
    } else {
        // `current.allowedAt || null`
        current
            .get("allowedAt")
            .filter(|v| jsval::truthy(v))
            .cloned()
            .unwrap_or(Value::Null)
    };
    let allowed_by = if actor_email.is_empty() {
        jsval::str_or(current.get("allowedBy"), "")
    } else {
        actor_email.to_string()
    };
    let curr_owner = jsval::str_or(current.get("ownerEmail"), "");
    let owner_norm = mitch_lib::auth::normalize_email(if owner_email.is_empty() {
        &curr_owner
    } else {
        owner_email
    });
    let mut entry = current.as_object().cloned().unwrap_or_default();
    entry.insert("allowed".to_string(), json!(allowed));
    entry.insert("allowedAt".to_string(), allowed_at);
    entry.insert(
        "revokedAt".to_string(),
        if allowed { Value::Null } else { json!(now) },
    );
    entry.insert("allowedBy".to_string(), json!(allowed_by));
    entry.insert("ownerEmail".to_string(), json!(owner_norm));
    entry.insert(
        "requested".to_string(),
        json!(if allowed {
            false
        } else {
            current.get("requested").map(jsval::truthy).unwrap_or(false)
        }),
    );
    if let Some(obj) = grants.as_object_mut() {
        obj.insert(id, Value::Object(entry));
    }
    save_vm_json(state, "vm_admin_grants.json", &grants);
    if !allowed {
        revoke_vm_desktop_connections(state, record_id);
    }
}

/// `requestVmAdminAccess(record, adminEmail)` (server.js:27135-27175).
pub(crate) fn request_vm_admin_access(state: &AppState, record: &Value, admin_email: &str) {
    let record_id = jsval::str_or(record.get("id"), "");
    let raw_owner = jsval::str_or(record.get("ownerEmail"), "");
    if record_id.is_empty() || raw_owner.is_empty() {
        return;
    }
    let owner = mitch_lib::auth::normalize_email(&raw_owner);
    let admin = mitch_lib::auth::normalize_email(admin_email);
    if owner.is_empty() || owner == admin {
        return;
    }

    let mut grants = load_vm_json(state, "vm_admin_grants.json", json!({}));
    let current = grants.get(&record_id).cloned().unwrap_or_else(|| json!({}));
    let mut entry = current.as_object().cloned().unwrap_or_default();
    entry.insert(
        "allowed".to_string(),
        json!(current.get("allowed").map(jsval::truthy).unwrap_or(false)),
    );
    entry.insert("requested".to_string(), json!(true));
    entry.insert("requestedAt".to_string(), json!(now_ms()));
    entry.insert("requestedBy".to_string(), json!(admin));
    entry.insert("ownerEmail".to_string(), json!(owner));
    if let Some(obj) = grants.as_object_mut() {
        obj.insert(record_id.clone(), Value::Object(entry));
    }
    save_vm_json(state, "vm_admin_grants.json", &grants);

    let key = format!("{}:{}:{}", owner, record_id, admin);
    let now = now_ms();
    {
        let mut notices = state
            .last_admin_request_notice
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let last = *notices.get(&key).unwrap_or(&0);
        if now - last < 15 * 60 * 1000 {
            return;
        }
        notices.insert(key, now);
    }

    let vm_name = record
        .get("friendlyName")
        .and_then(Value::as_str)
        .or_else(|| record.get("hostname").and_then(Value::as_str))
        .unwrap_or("your computer");
    let url = format!(
        "/vms/?action=allow-admin&id={}",
        mitch_lib::auth::encode_uri_component(&record_id)
    );
    mitch_lib::coins::add_vm_admin_notification(
        &state.store,
        state.data_dir(),
        &owner,
        "Admin Access Request",
        &format!(
            "Administrator {} requested access to your computer \"{}\". You can allow or revoke access in your Computer settings.",
            admin, vm_name
        ),
        &admin,
        &url,
    );

    vm_audit(
        state,
        &admin,
        Some(record),
        "ADMIN_ACCESS_REQUESTED",
        true,
        Some(&json!({ "requestedBy": admin, "ownerEmail": owner })),
    );
}

// ── admin-usage + capacity notices (server.js:27177-27220) ───────────────────

/// `notifyOwnerAdminUsedVm(record, adminEmail, operation)` (server.js:27177).
pub(crate) fn notify_owner_admin_used_vm(
    state: &AppState,
    record: &Value,
    admin_email: &str,
    operation: &str,
) {
    let raw_owner = jsval::str_or(record.get("ownerEmail"), "");
    if raw_owner.is_empty() {
        return;
    }
    let owner = mitch_lib::auth::normalize_email(&raw_owner);
    let admin = mitch_lib::auth::normalize_email(admin_email);
    if owner.is_empty() || owner == admin {
        return;
    }

    let record_id = jsval::str_or(record.get("id"), "");
    let key = format!("{}:{}:{}", owner, record_id, operation);
    let now = now_ms();
    {
        let mut notices = state
            .last_admin_usage_notice
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let last = *notices.get(&key).unwrap_or(&0);
        if now - last < 5 * 60 * 1000 {
            return;
        }
        notices.insert(key, now);
    }

    let vm_name = record
        .get("friendlyName")
        .and_then(Value::as_str)
        .or_else(|| record.get("hostname").and_then(Value::as_str))
        .unwrap_or("your computer");
    let url = format!(
        "/vms/?id={}",
        mitch_lib::auth::encode_uri_component(&record_id)
    );
    mitch_lib::coins::add_vm_admin_notification(
        &state.store,
        state.data_dir(),
        &owner,
        "Admin Used Your Computer",
        &format!(
            "Administrator {} accessed your computer \"{}\" ({}).",
            admin, vm_name, operation
        ),
        &admin,
        &url,
    );
}

/// `notifyCapacityFullOnAttempt(userEmail, actionDesc, details)`
/// (server.js:27199-27220). The JS awaits ntfy; our ntfy helper spawns.
pub(crate) fn notify_capacity_full_on_attempt(
    state: &AppState,
    user_email: &str,
    action_desc: &str,
    details: &Value,
) {
    let norm = mitch_lib::auth::normalize_email(if user_email.is_empty() {
        "unknown"
    } else {
        user_email
    });
    let now = now_ms();
    {
        let mut last = state
            .last_capacity_ntfy
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let last_time = *last.get(&norm).unwrap_or(&0);
        if now - last_time < 60_000 {
            return;
        }
        last.insert(norm.clone(), now);
    }

    let title = "VM Capacity Alert";
    // `details.cores ?? VM_FLEET_MAX_CORES` — null and undefined both fall.
    let cores = details
        .get("cores")
        .filter(|v| !v.is_null())
        .and_then(jsval::number)
        .unwrap_or(VM_FLEET_MAX_CORES);
    let memory_gb = details
        .get("memoryMb")
        .filter(|v| !v.is_null())
        .and_then(jsval::number)
        .unwrap_or(VM_FLEET_MAX_MEMORY_MB)
        / 1024.0;
    let msg = format!(
        "VM capacity full ({}/{} cores, {}/{} GB RAM, 6/6 max slots): {} attempted to {}.",
        cores,
        VM_FLEET_MAX_CORES,
        memory_gb.round() as i64,
        (VM_FLEET_MAX_MEMORY_MB / 1024.0).round() as i64,
        norm,
        action_desc,
    );
    println!("[vm-ntfy] {}: {}", title, msg);
    if std::env::var("NODE_ENV").unwrap_or_default() == "test" {
        let mut list = load_vm_json(state, "test_vm_ntfy_log.json", json!([]));
        if let Some(arr) = list.as_array_mut() {
            arr.push(json!({
                "title": title,
                "message": msg,
                "priority": "high",
                "timestamp": now,
                "user": norm,
            }));
        }
        save_vm_json(state, "test_vm_ntfy_log.json", &list);
    }
    crate::routes::push::ntfy_notify(&msg, title, "high");
}

// ── upgrades (server.js:27222-27280) ─────────────────────────────────────────

/// `getUserVmUpgrades(email)` (server.js:27224-27248).
pub(crate) fn get_user_vm_upgrades(state: &AppState, email: &str) -> Value {
    if email.is_empty() {
        return json!({
            "cpuCores": VM_DEFAULT_CPU_CORES,
            "memoryMb": VM_DEFAULT_MEMORY_MB,
            "diskGb": VM_DEFAULT_DISK_GB,
            "dailyMaxSeconds": VM_DAILY_MAX_SECONDS,
            "sessionUpgradeExpiresAt": null,
        });
    }
    let norm = mitch_lib::auth::normalize_email(email);
    let data = load_vm_json(state, "vm_upgrades.json", json!({}));
    let user = data.get(&norm).cloned().unwrap_or_else(|| json!({}));
    let mut daily_max_seconds = js_num_or(user.get("dailyMaxSeconds"), VM_DAILY_MAX_SECONDS);
    // `Number(...) || null` — absent/invalid/zero model as 0.0 ("null").
    let session_expires = user
        .get("sessionUpgradeExpiresAt")
        .and_then(jsval::number)
        .filter(|n| *n != 0.0)
        .unwrap_or(0.0);
    let now = now_ms() as f64;
    let is_expired = session_expires != 0.0 && now > session_expires;
    if daily_max_seconds > VM_DAILY_MAX_SECONDS && is_expired {
        daily_max_seconds = VM_DAILY_MAX_SECONDS;
    }
    json!({
        "cpuCores": js_num_or(user.get("cpuCores"), VM_DEFAULT_CPU_CORES),
        "memoryMb": js_num_or(user.get("memoryMb"), VM_DEFAULT_MEMORY_MB),
        "diskGb": js_num_or(user.get("diskGb"), VM_DEFAULT_DISK_GB),
        "dailyMaxSeconds": daily_max_seconds,
        "sessionUpgradeExpiresAt": if is_expired || session_expires == 0.0 {
            Value::Null
        } else {
            jsval::num_value(session_expires)
        },
    })
}

/// `saveUserVmUpgrade(email, category, value, durationDays)` (server.js:27250).
/// Returns `null` for the empty-email guard, else the updated object.
pub(crate) fn save_user_vm_upgrade(
    state: &AppState,
    email: &str,
    category: &str,
    value: f64,
    duration_days: f64,
) -> Value {
    if email.is_empty() {
        return Value::Null;
    }
    let norm = mitch_lib::auth::normalize_email(email);
    let mut data = load_vm_json(state, "vm_upgrades.json", json!({}));
    let current = data
        .get(&norm)
        .cloned()
        .unwrap_or_else(|| {
            json!({
                "cpuCores": VM_DEFAULT_CPU_CORES,
                "memoryMb": VM_DEFAULT_MEMORY_MB,
                "diskGb": VM_DEFAULT_DISK_GB,
                "dailyMaxSeconds": VM_DAILY_MAX_SECONDS,
                "sessionUpgradeExpiresAt": null,
            })
        })
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut current = current;
    match category {
        "cpu" => {
            current.insert("cpuCores".to_string(), jsval::num_value(value));
        }
        "ram" => {
            current.insert("memoryMb".to_string(), jsval::num_value(value));
        }
        "disk" => {
            current.insert("diskGb".to_string(), jsval::num_value(value));
        }
        "session" => {
            current.insert("dailyMaxSeconds".to_string(), jsval::num_value(value));
            if value > VM_DAILY_MAX_SECONDS {
                // `Math.max(1, Number(durationDays) || 30) * 86400 * 1000`
                let duration = if duration_days.is_finite() && duration_days != 0.0 {
                    duration_days
                } else {
                    30.0
                };
                let ms = duration.max(1.0) * 86_400_000.0;
                let now = now_ms() as f64;
                let base = match current
                    .get("sessionUpgradeExpiresAt")
                    .and_then(jsval::number)
                {
                    Some(t) if t > now => t,
                    _ => now,
                };
                current.insert(
                    "sessionUpgradeExpiresAt".to_string(),
                    jsval::num_value(base + ms),
                );
            } else {
                current.insert("sessionUpgradeExpiresAt".to_string(), Value::Null);
            }
        }
        _ => {}
    }
    if let Some(obj) = data.as_object_mut() {
        obj.insert(norm, Value::Object(current.clone()));
    }
    save_vm_json(state, "vm_upgrades.json", &data);
    Value::Object(current)
}

// ── daily usage (server.js:27285-27302) ──────────────────────────────────────

/// `getUserDailyVmUsage(email, dayKey = getVmDayKey())`.
pub(crate) fn get_user_daily_vm_usage(state: &AppState, email: &str) -> f64 {
    let day = mitch_lib::vm_security::get_vm_day_key(now_ms());
    get_user_daily_vm_usage_day(state, email, &day)
}

pub(crate) fn get_user_daily_vm_usage_day(state: &AppState, email: &str, day_key: &str) -> f64 {
    if email.is_empty() {
        return 0.0;
    }
    let norm = mitch_lib::auth::normalize_email(email);
    let usage = load_vm_json(state, "vm_daily_usage.json", json!({}));
    // `Math.max(0, Number(usage[norm]?.[dayKey]) || 0)`
    let raw = usage
        .get(&norm)
        .and_then(|u| u.get(day_key))
        .and_then(jsval::number)
        .unwrap_or(0.0);
    if raw.is_finite() {
        raw.max(0.0)
    } else {
        0.0
    }
}

/// `recordDailyVmUsage(email, secondsToAdd, dayKey = getVmDayKey())`.
pub(crate) fn record_daily_vm_usage(state: &AppState, email: &str, seconds_to_add: f64) -> f64 {
    if email.is_empty() || seconds_to_add <= 0.0 {
        return 0.0;
    }
    let norm = mitch_lib::auth::normalize_email(email);
    let mut usage = load_vm_json(state, "vm_daily_usage.json", json!({}));
    let day = mitch_lib::vm_security::get_vm_day_key(now_ms());
    if !usage.get(&norm).is_some_and(Value::is_object) {
        if let Some(obj) = usage.as_object_mut() {
            obj.insert(norm.clone(), json!({}));
        }
    }
    let Some(user) = usage.get_mut(&norm) else {
        return 0.0;
    };
    let current = user
        .get(&day)
        .and_then(jsval::number)
        .filter(|n| n.is_finite())
        .unwrap_or(0.0)
        .max(0.0);
    let updated = current + seconds_to_add.max(0.0).floor();
    if let Some(uo) = user.as_object_mut() {
        uo.insert(day, jsval::num_value(updated));
    }
    save_vm_json(state, "vm_daily_usage.json", &usage);
    updated
}

// ── extensions (server.js:27598-27611) ───────────────────────────────────────

/// `canUserExtendToday(email, isAdmin)`.
pub(crate) fn can_user_extend_today(state: &AppState, email: &str, is_admin: bool) -> bool {
    if is_admin {
        return true;
    }
    let norm = mitch_lib::auth::normalize_email(email);
    let exts = load_vm_json(state, "vm_extensions.json", json!({}));
    let last_at = exts
        .get(&norm)
        .and_then(|e| e.get("lastExtensionAt"))
        .and_then(jsval::number)
        .unwrap_or(0.0);
    can_user_extend(last_at, is_admin, now_ms() as f64)
}

/// `recordUserExtension(email)`.
pub(crate) fn record_user_extension(state: &AppState, email: &str) {
    let norm = mitch_lib::auth::normalize_email(email);
    let mut exts = load_vm_json(state, "vm_extensions.json", json!({}));
    if let Some(obj) = exts.as_object_mut() {
        obj.insert(norm, json!({ "lastExtensionAt": now_ms() }));
    }
    save_vm_json(state, "vm_extensions.json", &exts);
}

// ── cooldowns (server.js:27613-27642) ────────────────────────────────────────

/// `getVmCooldownRemaining(email, isAdmin)`.
pub(crate) fn get_vm_cooldown_remaining(state: &AppState, email: &str, is_admin: bool) -> f64 {
    if is_admin {
        return 0.0;
    }
    let norm = mitch_lib::auth::normalize_email(email);
    let cooldowns = load_vm_json(state, "vm_cooldowns.json", json!({}));
    let until = cooldowns
        .get(&norm)
        .and_then(|c| c.get("cooldownUntil"))
        .and_then(jsval::number)
        .unwrap_or(0.0);
    compute_cooldown_remaining(until, is_admin, now_ms() as f64)
}

/// `triggerVmCooldown(email, reason)`.
pub(crate) fn trigger_vm_cooldown(state: &AppState, email: &str, reason: &str) {
    if email.is_empty() {
        return;
    }
    let norm = mitch_lib::auth::normalize_email(email);
    if mitch_lib::auth::is_admin_email(&state.store, &norm) {
        return;
    }
    let now = now_ms();
    let mut cooldowns = load_vm_json(state, "vm_cooldowns.json", json!({}));
    if let Some(obj) = cooldowns.as_object_mut() {
        obj.insert(
            norm,
            json!({
                "cooldownUntil": now as f64 + VM_COOLDOWN_DURATION_MS,
                "triggeredAt": now,
                "reason": reason,
            }),
        );
    }
    save_vm_json(state, "vm_cooldowns.json", &cooldowns);
}

/// `clearVmCooldown(email)`.
pub(crate) fn clear_vm_cooldown(state: &AppState, email: &str) {
    if email.is_empty() {
        return;
    }
    let norm = mitch_lib::auth::normalize_email(email);
    let mut cooldowns = load_vm_json(state, "vm_cooldowns.json", json!({}));
    let removed = cooldowns
        .as_object_mut()
        .map(|obj| obj.remove(&norm).is_some())
        .unwrap_or(false);
    if removed {
        save_vm_json(state, "vm_cooldowns.json", &cooldowns);
    }
}

// ── leases (server.js:27644-27717) ───────────────────────────────────────────

/// `getVmLease(recordId, currentUptime, {isAdmin, ownerEmail})` (server.js:27644).
/// The lease map keeps raw Value objects so the JS missing-field semantics
/// (undefined lastSeenUptime/trackedUptime) survive.
pub(crate) fn get_vm_lease(
    state: &AppState,
    record_id: &str,
    current_uptime: f64,
    is_admin: bool,
    owner_email: &str,
) -> Value {
    let now = now_ms() as f64;
    let normalized_uptime = current_uptime.max(0.0).floor();
    let user_upgrades = get_user_vm_upgrades(state, owner_email);
    let daily_max = user_upgrades
        .get("dailyMaxSeconds")
        .and_then(jsval::number)
        .unwrap_or(0.0);
    let is_unlimited_session = daily_max >= 24.0 * 3600.0;

    if is_admin
        || (!owner_email.is_empty() && mitch_lib::auth::is_admin_email(&state.store, owner_email))
        || is_unlimited_session
    {
        return json!({
            "isExempt": true,
            "extended": false,
            "maxUptimeSeconds": null,
            "remainingSeconds": null,
            "dailyRemainingSeconds": null,
            "canExtend": false,
            "currentUptime": normalized_uptime,
        });
    }

    let extended = {
        let mut leases = state.vm_leases.lock().unwrap_or_else(|e| e.into_inner());
        let lease = leases.entry(record_id.to_string()).or_insert_with(|| {
            json!({
                "extended": false,
                "startedAt": now - normalized_uptime * 1000.0,
                "lastSeenUptime": normalized_uptime,
                "trackedUptime": 0,
            })
        });
        let last_seen = lease.get("lastSeenUptime").and_then(jsval::number);
        // `normalizedUptime > 0 && lease.lastSeenUptime > 60 &&
        //  normalizedUptime < lease.lastSeenUptime - 60` — undefined > 60 is
        // false in the JS.
        let reset = normalized_uptime > 0.0
            && last_seen.is_some_and(|l| l > 60.0)
            && normalized_uptime < last_seen.unwrap_or(0.0) - 60.0;
        if reset {
            if let Some(obj) = lease.as_object_mut() {
                obj.insert("extended".to_string(), json!(false));
                obj.insert(
                    "startedAt".to_string(),
                    json!(now - normalized_uptime * 1000.0),
                );
                obj.insert("trackedUptime".to_string(), json!(0));
            }
        }
        if let Some(obj) = lease.as_object_mut() {
            obj.insert("lastSeenUptime".to_string(), json!(normalized_uptime));
        }

        // Track the daily usage delta (server.js:27678-27694).
        if !owner_email.is_empty() && !is_admin {
            let tracked = lease
                .get("trackedUptime")
                .and_then(jsval::number)
                .unwrap_or(0.0);
            if normalized_uptime > tracked {
                let delta = normalized_uptime - tracked;
                record_daily_vm_usage(state, owner_email, delta);
                if let Some(obj) = lease.as_object_mut() {
                    obj.insert("trackedUptime".to_string(), json!(normalized_uptime));
                }
                let rec = vmlib::get_virtual_machine_by_id(&state.store, record_id);
                let vmid = rec.as_ref().map(|r| r.vmid);
                let vm_name = rec
                    .as_ref()
                    .map(|r| {
                        if !r.friendly_name.is_empty() {
                            r.friendly_name.clone()
                        } else if !r.hostname.is_empty() {
                            r.hostname.clone()
                        } else {
                            "My Computer".to_string()
                        }
                    })
                    .unwrap_or_else(|| "My Computer".to_string());
                vmlib::record_vm_usage_sample(
                    &state.store,
                    None,
                    &mitch_lib::vm_security::get_vm_day_key(now_ms()),
                    None,
                    None,
                    owner_email,
                    record_id,
                    vmid,
                    &vm_name,
                    normalized_uptime,
                    "",
                    true,
                );
            }
        }
        lease.get("extended").map(jsval::truthy).unwrap_or(false)
    };

    let used_today = if owner_email.is_empty() {
        0.0
    } else {
        get_user_daily_vm_usage(state, owner_email)
    };
    let daily_remaining = get_remaining_daily_vm_seconds(used_today, false, daily_max);
    let base_session_seconds = VM_BASE_MAX_UPTIME_SECONDS.max(daily_max);
    let max_uptime_seconds = if extended {
        base_session_seconds + VM_MAX_EXTENSION_SECONDS
    } else {
        base_session_seconds
    };
    let session_remaining = (max_uptime_seconds - normalized_uptime).max(0.0);
    let remaining_seconds = match daily_remaining {
        Some(d) => session_remaining.min(d),
        None => session_remaining,
    };
    let can_extend = !extended
        && remaining_seconds > 0.0
        && match daily_remaining {
            Some(d) => d > remaining_seconds,
            None => true,
        };

    json!({
        "isExempt": false,
        "extended": extended,
        "maxUptimeSeconds": jsval::num_value(max_uptime_seconds),
        "remainingSeconds": jsval::num_value(remaining_seconds),
        "dailyRemainingSeconds": match daily_remaining {
            Some(d) => jsval::num_value(d),
            None => Value::Null,
        },
        "canExtend": can_extend,
        "currentUptime": normalized_uptime,
    })
}

/// `clearVmLease(recordId)`.
pub(crate) fn clear_vm_lease(state: &AppState, record_id: &str) {
    state
        .vm_leases
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(record_id);
}

// ── fleet usage stats (server.js:27304-27430) — serves the admin batch ──────

/// `getVmFleetUsageStats(dayKey)` (server.js:27304-27430).
pub(crate) fn get_vm_fleet_usage_stats(state: &AppState, day_key: &str) -> Value {
    let resolved_day = if day_key.is_empty() {
        mitch_lib::vm_security::get_vm_day_key(now_ms())
    } else {
        day_key.to_string()
    };
    let records = vmlib::list_virtual_machines(&state.store, true);
    let record_jsons: Vec<Value> = records.iter().map(|r| r.to_json()).collect();

    // activeSessions from the live desktop sockets (server.js:27307-27321).
    let _now = now_ms();
    let mut active_by_record: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
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
            let actor = jsval::str_or(d.get("actorEmail"), "");
            let owner = jsval::str_or(d.get("ownerEmail"), "");
            let user = if actor.is_empty() { owner } else { actor };
            active_by_record.entry(record_id).or_default().push(user);
        }
    }

    let timeline = vmlib::get_vm_usage_timeline(&state.store, &resolved_day);
    let daily_usage = load_vm_json(state, "vm_daily_usage.json", json!({}));
    let profiles = load_vm_json(state, "profiles.json", json!({}));

    // Build the user email set across records, dailyUsage and timeline
    // (server.js:27328-27333) — insertion-ordered like the JS Set.
    let mut user_emails: Vec<String> = Vec::new();
    for r in &record_jsons {
        let owner = jsval::str_or(r.get("ownerEmail"), "");
        if !owner.is_empty() {
            let norm = mitch_lib::auth::normalize_email(&owner);
            if !user_emails.contains(&norm) {
                user_emails.push(norm);
            }
        }
    }
    if let Some(obj) = daily_usage.as_object() {
        for key in obj.keys() {
            if key.is_empty() {
                continue;
            }
            let norm = mitch_lib::auth::normalize_email(key);
            if !user_emails.contains(&norm) {
                user_emails.push(norm);
            }
        }
    }
    if let Some(hours) = timeline.get("hours").and_then(Value::as_array) {
        for h in hours {
            if let Some(users) = h.get("users").and_then(Value::as_array) {
                for u in users {
                    let email = jsval::str_or(u.get("email"), "");
                    if !email.is_empty() {
                        let norm = mitch_lib::auth::normalize_email(&email);
                        if !user_emails.contains(&norm) {
                            user_emails.push(norm);
                        }
                    }
                }
            }
        }
    }

    let mut records_by_owner: std::collections::HashMap<String, &Value> =
        std::collections::HashMap::new();
    for r in &record_jsons {
        let owner = jsval::str_or(r.get("ownerEmail"), "");
        if !owner.is_empty() {
            records_by_owner.insert(mitch_lib::auth::normalize_email(&owner), r);
        }
    }

    let mut rankings: Vec<Value> = Vec::new();
    let mut total_fleet_seconds_today = 0.0;

    for email in &user_emails {
        let norm = mitch_lib::auth::normalize_email(email);
        let profile = profiles.get(&norm).cloned().unwrap_or_else(|| json!({}));
        let record = records_by_owner.get(&norm).copied();
        let user_daily = daily_usage.get(&norm).cloned().unwrap_or_else(|| json!({}));

        let mut today_seconds = js_num_or(user_daily.get(&resolved_day), 0.0).max(0.0);

        // Also take the max timeline-sample uptime for today.
        let mut max_timeline_uptime = 0.0;
        let mut timeline_sample_count = 0i64;
        if let Some(hours) = timeline.get("hours").and_then(Value::as_array) {
            for h in hours {
                let user = h.get("users").and_then(Value::as_array).and_then(|users| {
                    users
                        .iter()
                        .find(|u| {
                            mitch_lib::auth::normalize_email(&jsval::str_or(u.get("email"), ""))
                                == norm
                        })
                        .cloned()
                });
                if let Some(u) = user {
                    timeline_sample_count +=
                        u.get("samples").and_then(jsval::number).unwrap_or(0.0) as i64;
                    let mu = u
                        .get("maxUptimeSeconds")
                        .and_then(jsval::number)
                        .unwrap_or(0.0);
                    if mu > max_timeline_uptime {
                        max_timeline_uptime = mu;
                    }
                }
            }
        }
        if max_timeline_uptime > today_seconds {
            today_seconds = max_timeline_uptime;
        }

        // All-time seconds: sum across every day in the daily usage record.
        let mut all_time_seconds = 0.0;
        if let Some(obj) = user_daily.as_object() {
            for secs in obj.values() {
                all_time_seconds += js_num_or(Some(secs), 0.0).max(0.0);
            }
        }
        if today_seconds > all_time_seconds {
            all_time_seconds = today_seconds;
        }

        total_fleet_seconds_today += today_seconds;

        let active_users = record
            .map(|r| jsval::str_or(r.get("id"), ""))
            .and_then(|id| active_by_record.get(&id).cloned())
            .unwrap_or_default();
        let is_in_use = !active_users.is_empty();
        // recordViews is not supplied by any current caller → view is null,
        // so isRunning is just the in-use flag.
        let is_running = is_in_use;

        let display_name = {
            let dn = profile
                .get("displayName")
                .filter(|v| jsval::truthy(v))
                .map(jsval::string);
            dn.or_else(|| {
                profile
                    .get("nickname")
                    .filter(|v| jsval::truthy(v))
                    .map(jsval::string)
            })
            .unwrap_or_else(|| mitch_lib::profile::default_username_for_email(&norm))
        };
        let vm_name = record
            .map(|r| {
                let vmid = r.get("vmid").and_then(jsval::number).unwrap_or(0.0);
                let fallback = format!("VM {}", jsval::num_value(vmid));
                r.get("friendlyName")
                    .and_then(Value::as_str)
                    .or_else(|| r.get("hostname").and_then(Value::as_str))
                    .unwrap_or(&fallback)
                    .to_string()
            })
            .unwrap_or_else(|| "My Computer".to_string());

        rankings.push(json!({
            "email": norm,
            "displayName": display_name,
            "vmName": vm_name,
            "vmRecordId": record.map(|r| jsval::str_or(r.get("id"), "")).unwrap_or_default(),
            "vmid": record.and_then(|r| r.get("vmid")).and_then(jsval::number).filter(|v| *v != 0.0).map(jsval::num_value).unwrap_or(Value::Null),
            "todaySeconds": today_seconds,
            "todayFormatted": format_uptime_duration(today_seconds),
            "allTimeSeconds": all_time_seconds,
            "allTimeFormatted": format_uptime_duration(all_time_seconds),
            "isRunning": is_running,
            "isInUse": is_in_use,
            "activeUsers": active_users,
            "activeUsersCount": active_users.len(),
            "samplesToday": timeline_sample_count,
        }));
    }

    // Sort: most uptime today first; ties broken by all-time uptime.
    rankings.sort_by(|a, b| {
        let at = a.get("todaySeconds").and_then(jsval::number).unwrap_or(0.0);
        let bt = b.get("todaySeconds").and_then(jsval::number).unwrap_or(0.0);
        let aa = a
            .get("allTimeSeconds")
            .and_then(jsval::number)
            .unwrap_or(0.0);
        let ba = b
            .get("allTimeSeconds")
            .and_then(jsval::number)
            .unwrap_or(0.0);
        bt.partial_cmp(&at)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(ba.partial_cmp(&aa).unwrap_or(std::cmp::Ordering::Equal))
    });
    for (idx, item) in rankings.iter_mut().enumerate() {
        if let Some(obj) = item.as_object_mut() {
            obj.insert("rank".to_string(), json!(idx + 1));
        }
    }

    let top_user = if rankings
        .first()
        .and_then(|r| r.get("todaySeconds"))
        .and_then(jsval::number)
        .unwrap_or(0.0)
        > 0.0
    {
        rankings.first().cloned()
    } else {
        None
    };
    let running_now = rankings
        .iter()
        .filter(|r| r.get("isRunning").map(jsval::truthy).unwrap_or(false))
        .count() as i64;
    let peak_running_today = timeline
        .get("peakConcurrentToday")
        .and_then(jsval::number)
        .unwrap_or(0.0)
        .max(running_now as f64) as i64;
    let active_users_today = rankings
        .iter()
        .filter(|r| r.get("todaySeconds").and_then(jsval::number).unwrap_or(0.0) > 0.0)
        .count() as i64;

    json!({
        "dayKey": resolved_day,
        "hourlyTimeline": timeline.get("hours").cloned().unwrap_or_else(|| json!([])),
        "rankings": rankings,
        "summary": {
            "topUser": top_user.unwrap_or(Value::Null),
            "peakRunningToday": peak_running_today,
            "totalFleetSecondsToday": total_fleet_seconds_today,
            "totalFleetHoursToday": mitch_lib::data::js_to_fixed(total_fleet_seconds_today / 3600.0, 1),
            "activeUsersToday": active_users_today,
            "totalTrackedUsers": rankings.len(),
        },
    })
}

// ── eligibility + capacity (server.js:27509-27596) ───────────────────────────

/// `isEligibleForFreeVm(email, isAdmin)` (server.js:27509-27513).
pub(crate) fn is_eligible_for_free_vm(state: &AppState, email: &str, is_admin: bool) -> bool {
    if email.is_empty() {
        return false;
    }
    let norm = mitch_lib::auth::normalize_email(email);
    mitch_lib::vm_security::is_eligible_for_free_vm(
        &norm,
        is_admin,
        mitch_lib::auth::is_premium_email(&state.store, &norm),
    )
}

/// `getRunningNonAdminVmResources()` (server.js:27515-27573).
pub(crate) async fn get_running_non_admin_vm_resources(state: &AppState) -> Value {
    if std::env::var("NODE_ENV").unwrap_or_default() == "test" {
        let mock = load_vm_json(state, "test_vm_mock.json", Value::Null);
        if !mock.is_null() {
            let cores = mock.get("runningCores").and_then(jsval::number);
            let memory = mock.get("runningMemoryMb").and_then(jsval::number);
            if let (Some(c), Some(m)) = (cores, memory) {
                let count = mock
                    .get("runningNonAdminCount")
                    .and_then(jsval::number)
                    .unwrap_or_else(|| (c / VM_DEFAULT_CPU_CORES).ceil());
                return json!({ "count": count, "cores": c, "memoryMb": m });
            }
            if let Some(n) = mock.get("runningNonAdminCount").and_then(jsval::number) {
                let is_maxed = n >= 6.0;
                return json!({
                    "count": n,
                    "cores": if is_maxed { VM_FLEET_MAX_CORES } else { n * VM_DEFAULT_CPU_CORES },
                    "memoryMb": if is_maxed { VM_FLEET_MAX_MEMORY_MB } else { n * VM_DEFAULT_MEMORY_MB },
                });
            }
        }
    }
    let svc = crate::proxmox_desktop::ProxmoxDesktopService::desktop();
    if !svc.configured() {
        return json!({ "count": 0, "cores": 0, "memoryMb": 0 });
    }
    match svc.list_guests().await {
        Ok(guests) => {
            let mut count = 0.0;
            let mut cores = 0.0;
            let mut memory_mb = 0.0;
            for guest in &guests {
                let status = jsval::str_or(guest.get("status"), "");
                if jsval::truthy(&guest["template"]) || (status != "running" && status != "paused")
                {
                    continue;
                }
                let vmid = guest.get("vmid").and_then(jsval::number);
                let record = vmid.and_then(|v| vmlib::get_virtual_machine_by_vmid(&state.store, v));
                let record_json = record.as_ref().map(|r| r.to_json());
                let mut owner_email = record_json
                    .as_ref()
                    .and_then(|r| r.get("ownerEmail"))
                    .filter(|v| jsval::truthy(v))
                    .map(jsval::string)
                    .unwrap_or_default();
                if owner_email.is_empty() {
                    let free = state
                        .active_free_vms
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    for (email, entry) in free.iter() {
                        if entry.get("vmid").and_then(jsval::number) == vmid {
                            owner_email = email.clone();
                            break;
                        }
                    }
                }
                if owner_email.is_empty() {
                    let apps = vm_applications(state);
                    if let Some(obj) = apps.as_object() {
                        for (email, app) in obj {
                            if app.get("vmid").and_then(jsval::number) == vmid {
                                owner_email = jsval::str_or(app.get("email"), email);
                                break;
                            }
                        }
                    }
                }
                if !owner_email.is_empty()
                    && mitch_lib::auth::is_admin_email(&state.store, &owner_email)
                {
                    continue;
                }
                count += 1.0;
                cores += js_num_or(
                    record_json
                        .as_ref()
                        .and_then(|r| r.get("cpuCores"))
                        .filter(|v| jsval::truthy(v))
                        .or_else(|| guest.get("cpuCores").filter(|v| jsval::truthy(v))),
                    VM_DEFAULT_CPU_CORES,
                );
                memory_mb += js_num_or(
                    record_json
                        .as_ref()
                        .and_then(|r| r.get("memoryMb"))
                        .filter(|v| jsval::truthy(v))
                        .or_else(|| guest.get("memoryMb").filter(|v| jsval::truthy(v))),
                    VM_DEFAULT_MEMORY_MB,
                );
            }
            json!({ "count": count, "cores": cores, "memoryMb": memory_mb })
        }
        Err(err) => {
            eprintln!(
                "[vm] Error calculating running non-admin VM resources: {} ({})",
                err.code, err.message
            );
            json!({ "count": 0, "cores": 0, "memoryMb": 0 })
        }
    }
}

/// `canAccommodateVmResources(requestedCores, requestedMemoryMb)`
/// (server.js:27580-27596).
pub(crate) async fn can_accommodate_vm_resources(
    state: &AppState,
    requested_cores: f64,
    requested_memory_mb: f64,
) -> Value {
    let running = get_running_non_admin_vm_resources(state).await;
    let current_cores = running.get("cores").and_then(jsval::number).unwrap_or(0.0);
    let current_memory_mb = running
        .get("memoryMb")
        .and_then(jsval::number)
        .unwrap_or(0.0);
    // `Number(requestedCores || 0)` — NaN falls to 0.
    let req_cores = if requested_cores.is_finite() {
        requested_cores
    } else {
        0.0
    };
    let req_memory = if requested_memory_mb.is_finite() {
        requested_memory_mb
    } else {
        0.0
    };
    let would_cores = current_cores + req_cores;
    let would_memory_mb = current_memory_mb + req_memory;
    let ok = would_cores <= VM_FLEET_MAX_CORES && would_memory_mb <= VM_FLEET_MAX_MEMORY_MB;
    json!({
        "ok": ok,
        "currentCores": current_cores,
        "currentMemoryMb": current_memory_mb,
        "wouldCores": would_cores,
        "wouldMemoryMb": would_memory_mb,
        "maxCores": VM_FLEET_MAX_CORES,
        "maxMemoryMb": VM_FLEET_MAX_MEMORY_MB,
    })
}

// ── publicVmRecord (server.js:27791-27839) ───────────────────────────────────

pub(crate) fn public_vm_record(
    state: &AppState,
    record: &Value,
    runtime: Option<&Value>,
    actor: Option<&Value>,
) -> Value {
    let is_running = runtime
        .and_then(|r| r.get("state"))
        .map(|s| jsval::string(s) == "running")
        .unwrap_or(false);
    let actor_is_admin = actor
        .and_then(|a| a.get("isAdmin"))
        .map(jsval::truthy)
        .unwrap_or(false);
    let raw_owner = jsval::str_or(record.get("ownerEmail"), "");
    let owner_admin = jsval::truthy(&record["ownerEmail"])
        && !raw_owner.is_empty()
        && mitch_lib::auth::is_admin_email(&state.store, &raw_owner);
    let is_admin = actor_is_admin || owner_admin;
    let owner_email = match record.get("ownerEmail").filter(|v| jsval::truthy(v)) {
        Some(v) => jsval::string(v),
        None => actor
            .and_then(|a| a.get("email"))
            .filter(|v| jsval::truthy(v))
            .map(jsval::string)
            .unwrap_or_default(),
    };
    let user_upgrades = get_user_vm_upgrades(state, &owner_email);
    let daily_max = user_upgrades
        .get("dailyMaxSeconds")
        .and_then(jsval::number)
        .unwrap_or(0.0);
    let is_unlimited_session = daily_max >= 24.0 * 3600.0;
    let is_exempt = is_admin || is_unlimited_session;
    let base_session_seconds = VM_BASE_MAX_UPTIME_SECONDS.max(daily_max);

    let mut lease = if is_running {
        let uptime = runtime
            .and_then(|r| r.get("uptime"))
            .and_then(jsval::number)
            .unwrap_or(0.0);
        get_vm_lease(
            state,
            &jsval::str_or(record.get("id"), ""),
            uptime,
            is_admin,
            &owner_email,
        )
    } else {
        let daily_remaining = get_remaining_daily_vm_seconds(
            get_user_daily_vm_usage(state, &owner_email),
            is_admin,
            daily_max,
        );
        json!({
            "isExempt": is_exempt,
            "extended": false,
            "maxUptimeSeconds": if is_exempt { Value::Null } else { jsval::num_value(base_session_seconds) },
            "remainingSeconds": if is_exempt {
                Value::Null
            } else {
                jsval::num_value(match daily_remaining {
                    Some(d) => base_session_seconds.min(d),
                    None => base_session_seconds,
                })
            },
            "dailyRemainingSeconds": if is_exempt {
                Value::Null
            } else {
                match daily_remaining {
                    Some(d) => jsval::num_value(d),
                    None => Value::Null,
                }
            },
            "canExtend": !is_exempt
                && match daily_remaining {
                    Some(d) => d > 0.0,
                    None => true,
                },
            "currentUptime": 0,
        })
    };
    let daily_extension_used =
        !is_admin && !can_user_extend_today(state, &raw_owner, actor_is_admin);
    if daily_extension_used {
        if let Some(obj) = lease.as_object_mut() {
            obj.insert("canExtend".to_string(), json!(false));
            obj.insert("dailyExtensionUsed".to_string(), json!(true));
        }
    }
    let cooldown_remaining_seconds = get_vm_cooldown_remaining(state, &raw_owner, is_admin);

    // `record.status === 'provisioning' ? 'starting' :
    //  record.status === 'provisioning-failed' ? 'setup-incomplete' :
    //  runtime?.state || 'unknown'`.
    let status = match jsval::str_or(record.get("status"), "").as_str() {
        "provisioning" => "starting".to_string(),
        "provisioning-failed" => "setup-incomplete".to_string(),
        _ => runtime
            .and_then(|r| r.get("state"))
            .filter(|v| jsval::truthy(v))
            .map(jsval::string)
            .unwrap_or_else(|| "unknown".to_string()),
    };

    json!({
        "id": jsval::str_or(record.get("id"), ""),
        "name": jsval::str_or(record.get("friendlyName"), "My Computer"),
        "hostname": jsval::str_or(
            runtime
                .and_then(|r| r.get("hostname"))
                .filter(|v| jsval::truthy(v))
                .or_else(|| record.get("hostname")),
            "",
        ),
        "operatingSystem": jsval::str_or(record.get("operatingSystem"), "Linux Desktop"),
        "status": status,
        "cpuCores": runtime.and_then(|r| r.get("cpuCores")).filter(|v| jsval::truthy(v)).cloned()
            .unwrap_or_else(|| record.get("cpuCores").cloned().unwrap_or(Value::Null)),
        "cpuUsage": runtime.and_then(|r| r.get("cpuUsage")).and_then(jsval::number).filter(|v| *v != 0.0).unwrap_or(0.0),
        "memoryMb": record.get("memoryMb").cloned().unwrap_or(Value::Null),
        "memoryUsed": runtime.and_then(|r| r.get("memoryUsed")).and_then(jsval::number).filter(|v| *v != 0.0).unwrap_or(0.0),
        "memoryTotal": runtime.and_then(|r| r.get("memoryTotal")).and_then(jsval::number).filter(|v| *v != 0.0).unwrap_or_else(|| {
            record.get("memoryMb").and_then(jsval::number).unwrap_or(0.0) * 1024.0 * 1024.0
        }),
        "diskGb": record.get("diskGb").cloned().unwrap_or(Value::Null),
        "diskUsed": runtime.and_then(|r| r.get("diskUsed")).and_then(jsval::number).filter(|v| *v != 0.0).unwrap_or(0.0),
        "diskTotal": runtime.and_then(|r| r.get("diskTotal")).and_then(jsval::number).filter(|v| *v != 0.0).unwrap_or_else(|| {
            record.get("diskGb").and_then(jsval::number).unwrap_or(0.0) * 1024.0 * 1024.0 * 1024.0
        }),
        "ipAddress": jsval::str_or(
            runtime
                .and_then(|r| r.get("ipAddress"))
                .filter(|v| jsval::truthy(v))
                .or_else(|| record.get("ipAddress").filter(|v| jsval::truthy(v))),
            "",
        ),
        "uptime": runtime.and_then(|r| r.get("uptime")).and_then(jsval::number).filter(|v| *v != 0.0).unwrap_or(0.0),
        "desktopAvailable": jsval::str_or(record.get("guestType"), "") == "qemu",
        "createdAt": record.get("createdAt").cloned().unwrap_or(Value::Null),
        "lease": lease,
        "upgrades": user_upgrades,
        "cooldownRemainingSeconds": cooldown_remaining_seconds,
        "adminAccessAllowed": is_vm_admin_access_allowed(state, &jsval::str_or(record.get("id"), "")),
        "adminAccessRequested": is_vm_admin_access_requested(state, &jsval::str_or(record.get("id"), "")),
    })
}

// ── desktop session registry (server.js:27841-27872) ─────────────────────────

/// `cleanupVmDesktopSessions()` — expires/used sessions drop from the map;
/// sockets whose authorization lapsed get the 1008 close signal (the owning
/// bridge task removes itself).
pub(crate) fn cleanup_vm_desktop_sessions(state: &AppState) {
    let now = now_ms();
    {
        let mut sessions = state
            .vm_desktop_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        sessions.retain(|_, s| {
            let expired = s.get("expiresAt").and_then(jsval::number).unwrap_or(0.0) <= now as f64;
            let used = s.get("used").map(jsval::truthy).unwrap_or(false);
            !(s.is_null() || expired || used)
        });
    }
    {
        let sockets = state
            .vm_desktop_sockets
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for client in sockets.values() {
            if !vm_desktop_socket_authorized(state, &client.data) {
                let _ = client
                    .close_tx
                    .send((1008, "Desktop access expired".to_string()));
            }
        }
    }
    state
        .vm_power_gate
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .cleanup(now as f64);
}

/// `vmDesktopSocketAuthorized(ws)` (server.js:27853-27867) against the
/// registered `ws.data` subset.
pub(crate) fn vm_desktop_socket_authorized(state: &AppState, data: &Value) -> bool {
    let node_env_test = std::env::var("NODE_ENV").unwrap_or_default() == "test";
    let sid = jsval::str_or(data.get("sid"), "");
    if sid.is_empty()
        || !mitch_lib::auth::valid_id(&sid, &state.id_secret)
        || is_revoked_id(state, &sid)
        || mitch_lib::auth::banned_info_for_sid(&state.store, &state.id_secret, &sid).is_some()
    {
        return false;
    }
    let session_key = jsval::str_or(data.get("authSessionKey"), "");
    if !session_key.is_empty() {
        // `loadAuthSessions()[key]` — data/auth_sessions.json keyed by the
        // sha256 session-token hash.
        let sessions = state
            .store
            .read_document(&state.data_dir().join("auth_sessions.json"), json!({}));
        let session = sessions.get(&session_key);
        let expires_ok = session
            .and_then(|s| s.get("expiresAt"))
            .and_then(jsval::number)
            .map(|e| e > now_ms() as f64)
            .unwrap_or(false);
        let stored_gen = session
            .and_then(|s| s.get("gen"))
            .and_then(jsval::number)
            .unwrap_or(0.0) as i64;
        let current_gen = mitch_lib::auth::current_session_generation(
            &state.store,
            &jsval::str_or(data.get("actorEmail"), ""),
        );
        if !expires_ok || stored_gen != current_gen {
            return false;
        }
    } else if !node_env_test {
        return false;
    }
    let record =
        vmlib::get_virtual_machine_by_id(&state.store, &jsval::str_or(data.get("recordId"), ""));
    let Some(record) = record else {
        return false;
    };
    let rj = record.to_json();
    if jsval::str_or(rj.get("ownerEmail"), "") != jsval::str_or(data.get("ownerEmail"), "") {
        return false;
    }
    if rj.get("vmid").and_then(jsval::number) != data.get("vmid").and_then(jsval::number) {
        return false;
    }
    if jsval::str_or(rj.get("node"), "") != jsval::str_or(data.get("node"), "") {
        return false;
    }
    let is_admin =
        mitch_lib::auth::is_admin_id(&state.store, &state.id_secret, &sid, node_env_test);
    let is_owner = mitch_lib::auth::is_owner_id(&state.store, &state.id_secret, &sid);
    let owner_norm = jsval::str_or(rj.get("ownerEmail"), "");
    if is_admin
        && !is_owner
        && jsval::truthy(&rj["ownerEmail"])
        && mitch_lib::auth::normalize_email(&owner_norm)
            != mitch_lib::auth::normalize_email(&jsval::str_or(data.get("actorEmail"), ""))
        && !is_vm_admin_access_allowed(state, &jsval::str_or(rj.get("id"), ""))
    {
        return false;
    }
    vm_record_allowed_for_actor(
        state,
        &rj,
        &json!({
            "email": jsval::str_or(data.get("actorEmail"), ""),
            "isAdmin": is_admin,
            "isOwner": is_owner,
        }),
    )
}

/// `revokeVmDesktopConnections(recordId)` (server.js:27869-27872).
pub(crate) fn revoke_vm_desktop_connections(state: &AppState, record_id: &str) {
    {
        let mut sessions = state
            .vm_desktop_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        sessions.retain(|_, s| jsval::str_or(s.get("recordId"), "") != record_id);
    }
    let sockets = state
        .vm_desktop_sockets
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for client in sockets.values() {
        if jsval::str_or(client.data.get("recordId"), "") == record_id {
            let _ = client
                .close_tx
                .send((1008, "Desktop access changed".to_string()));
        }
    }
}

// ── the PVE provisioning helpers (server.js:27988-28463) ─────────────────────

/// `PVE_TEMPLATE_LINUX` (server.js:27080).
pub(crate) fn pve_template_linux() -> i64 {
    std::env::var("PVE_TEMPLATE_LINUX")
        .ok()
        .and_then(|v| js_parse_int(&v))
        .unwrap_or(9000)
}

/// `PVE_LXC_TEMPLATE` (server.js:27081).
pub(crate) fn pve_lxc_template() -> String {
    std::env::var("PVE_LXC_TEMPLATE")
        .unwrap_or_else(|_| "local:vztmpl/debian-13-standard_13.6-1_amd64.tar.zst".to_string())
}

/// `parseInt(s, 10)` — leading-digit prefix parse.
fn js_parse_int(s: &str) -> Option<i64> {
    let trimmed = s.trim_start();
    let digits: String = trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse::<i64>().ok()
    }
}

/// `VMID ${vmid}` — the JS template interpolation of a number.
fn js_vmid_str(vmid: f64) -> String {
    jsval::string(&json!(vmid))
}

/// `assertVmIdInRange(vmid, where)` (server.js:27996-28002) — the caller
/// maps the message into its own result shape.
pub(crate) fn assert_vmid_in_range(vmid: f64, where_: &str) -> Result<(), String> {
    if !vmid.is_finite() || vmid < PVE_VMID_MIN || vmid > PVE_VMID_MAX {
        return Err(format!(
            "VMID {} out of allowed range [{}, {}]{}",
            js_vmid_str(vmid),
            PVE_VMID_MIN,
            PVE_VMID_MAX,
            if where_.is_empty() {
                String::new()
            } else {
                format!(" ({where_})")
            },
        ));
    }
    Ok(())
}

/// A PVE client without the JS fetchWithDeadline — the plain `fetch` calls
/// (powerUserVm, cloneUserVm, stop/destroy) have no timeout in the JS.
fn pve_client_no_deadline() -> reqwest::Client {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap_or_default()
}

/// `getExistingVmids()` (server.js:28008-28035) — the node's LXC + QEMU
/// lists folded into a vmid→name map.
pub(crate) async fn get_existing_vmids() -> std::collections::HashMap<i64, String> {
    let mut ids = std::collections::HashMap::new();
    for type_ in ["lxc", "qemu"] {
        let url = format!("{}/nodes/{}/{type_}", pve_url(), pve_node());
        let res = pve_client(5000)
            .get(&url)
            .header("Authorization", pve_token());
        if let Ok(res) = res.send().await {
            if res.status().is_success() {
                if let Ok(data) = res.json::<Value>().await {
                    if let Some(list) = data.get("data").and_then(Value::as_array) {
                        for vm in list {
                            let parsed = match vm.get("vmid") {
                                Some(Value::Number(n)) => n.as_i64(),
                                Some(Value::String(s)) => js_parse_int(s),
                                _ => None,
                            };
                            if let Some(id) = parsed {
                                ids.insert(id, jsval::str_or(vm.get("name"), ""));
                            }
                        }
                    }
                }
            }
        }
    }
    ids
}

/// `powerUserVm(vmid, action)` (server.js:28145-28166).
pub(crate) async fn power_user_vm(state: &AppState, vmid: f64, action: &str) -> Value {
    if pve_token().is_empty() {
        return json!({ "success": false, "error": "Proxmox token not configured." });
    }
    if !["start", "stop", "reboot"].contains(&action) {
        return json!({ "success": false, "error": "Invalid power action." });
    }
    let type_ = get_vm_type_by_vmid(state, vmid);
    let vmid_int = if vmid.is_finite() && vmid.fract() == 0.0 && vmid >= 0.0 {
        vmid as i64
    } else {
        return json!({ "success": false, "error": "Invalid power action." });
    };
    let url = format!(
        "{}/nodes/{}/{type_}/{vmid_int}/status/{action}",
        pve_url(),
        pve_node()
    );
    match pve_client_no_deadline()
        .post(&url)
        .header("Authorization", pve_token())
        .send()
        .await
    {
        Ok(res) => {
            if !res.status().is_success() {
                return json!({
                    "success": false,
                    "error": format!("Failed to set power state: {}", res.status().as_u16())
                });
            }
            json!({ "success": true })
        }
        Err(err) => {
            eprintln!("[proxmox] Power state control error for {type_}: {}", err);
            json!({ "success": false, "error": err.to_string() })
        }
    }
}

/// `attachSshdHookToLxc(vmid)` (server.js:28182-28239) — SSH to the Proxmox
/// host and run the `mitch-attach-hook <vmid>` forced command.
pub(crate) async fn attach_sshd_hook_to_lxc(vmid: f64) -> Value {
    if !is_vm_id_in_range(vmid) {
        return json!({
            "success": false,
            "error": format!("vmid {} out of allowed range [{}, {}]", js_vmid_str(vmid), PVE_VMID_MIN, PVE_VMID_MAX)
        });
    }
    let host = std::env::var("PVE_SSH_HOST").unwrap_or_else(|_| "192.168.100.1".to_string());
    let user = std::env::var("PVE_SSH_USER").unwrap_or_else(|_| "root".to_string());
    let key_path = std::env::var("PVE_SSH_KEY_PATH").unwrap_or_default();
    let port = js_parse_int(&std::env::var("PVE_SSH_PORT").unwrap_or_else(|_| "39222".to_string()))
        .unwrap_or(39222);
    if key_path.is_empty() {
        return json!({
            "success": false,
            "error": "PVE_SSH_KEY_PATH is not configured. process.env.PVE_SSH_KEY_PATH is undefined — check that .env contains `PVE_SSH_KEY_PATH=/path/to/key` (no quotes, no trailing comment) and that the [env] startup log reports it as loaded."
        });
    }
    if !std::path::Path::new(&key_path).exists() {
        return json!({
            "success": false,
            "error": format!("PVE_SSH_KEY_PATH points at '{}' but that file is not accessible to this process. If this is running inside a container, .env needs to use the container-internal path (e.g. /app/data/portal_id_rsa) — host paths like /home/mitch/... aren't visible. To find the real key: ls data/portal_id_rsa, then set PVE_SSH_KEY_PATH to the path as it appears from inside the bun-server container.", key_path)
        });
    }

    let cmd = format!("mitch-attach-hook {}", js_vmid_str(vmid));
    let mut command = tokio::process::Command::new("ssh");
    command
        .arg("-i")
        .arg(&key_path)
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ConnectTimeout=5")
        .arg("-o")
        .arg("StrictHostKeyChecking=accept-new")
        .arg("-p")
        .arg(port.to_string())
        .arg(format!("{}@{}", user, host))
        .arg(&cmd);
    let attempt = command.output();
    let result = match tokio::time::timeout(std::time::Duration::from_millis(15_000), attempt).await
    {
        Ok(Ok(out)) => out,
        Ok(Err(err)) => {
            return json!({ "success": false, "error": format!("ssh spawn failed: {}", err) });
        }
        Err(_) => {
            // spawnSync timeout → status null → `exit null`.
            return json!({ "success": false, "error": "mitch-attach-hook exit null" });
        }
    };
    if !result.status.success() {
        let status = result
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "null".to_string());
        let stderr = String::from_utf8_lossy(&result.stderr).trim().to_string();
        return json!({
            "success": false,
            "error": format!("mitch-attach-hook exit {}", status),
            "stderr": stderr,
        });
    }
    json!({
        "success": true,
        "stdout": String::from_utf8_lossy(&result.stdout).trim().to_string(),
    })
}

/// `createLxcContainer(email, tier, vmid, password)` (server.js:28241-28320).
pub(crate) async fn create_lxc_container(
    _state: &AppState,
    _email: &str,
    tier: &str,
    vmid: f64,
    password: &str,
) -> Value {
    if pve_token().is_empty() {
        return json!({ "success": false, "error": "Proxmox token not configured." });
    }
    if let Err(msg) = assert_vmid_in_range(vmid, "createLxcContainer") {
        return json!({ "success": false, "error": msg });
    }

    let cores = if tier == "premium" { 2 } else { 1 };
    let memory = if tier == "premium" { 4096 } else { 1024 };
    let ip_suffix = if vmid >= 300.0 { vmid - 200.0 } else { vmid } as i64;
    let container_ip = format!("10.0.0.{}", ip_suffix);
    let hostname = if tier == "premium" {
        format!("premium-lxc-{}", vmid as i64)
    } else {
        format!("student-lxc-{}", vmid as i64)
    };

    let body_str = {
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        form.append_pair("vmid", &js_vmid_str(vmid));
        form.append_pair("ostemplate", &pve_lxc_template());
        form.append_pair("cores", &cores.to_string());
        form.append_pair("memory", &memory.to_string());
        form.append_pair("swap", "512");
        form.append_pair("hostname", &hostname);
        form.append_pair(
            "password",
            if password.is_empty() {
                "password"
            } else {
                password
            },
        );
        form.append_pair("rootfs", "local-lvm:8");
        form.append_pair(
            "net0",
            &format!("name=eth0,bridge=vmbr2,firewall=0,ip={container_ip}/24,gw=10.0.0.1"),
        );
        form.append_pair("nameserver", "1.1.1.1");
        form.append_pair("unprivileged", "1");
        form.append_pair("start", "1");
        form.append_pair("pool", "sandboxes");
        form.finish()
    };
    let url = format!("{}/nodes/{}/lxc", pve_url(), pve_node());

    let res = pve_client(15000)
        .post(&url)
        .header("Authorization", pve_token())
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body_str)
        .send()
        .await;
    let res = match res {
        Ok(r) => r,
        Err(err) => {
            if err.is_timeout() {
                // fetchWithDeadline null → the pct create timed out.
                return json!({ "success": false, "error": "Proxmox unreachable (pct create timed out)" });
            }
            eprintln!("[proxmox] Error creating LXC container: {}", err);
            return json!({ "success": false, "error": err.to_string() });
        }
    };
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    let data: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => {
            eprintln!(
                "[proxmox] LXC creation: non-JSON response: {} {}",
                status.as_u16(),
                text.chars().take(500).collect::<String>()
            );
            let preview: String = text.chars().take(200).collect();
            let shown = if preview.is_empty() {
                "(empty body)".to_string()
            } else {
                preview
            };
            return json!({
                "success": false,
                "error": format!("Proxmox returned non-JSON ({}): {}", status.as_u16(), shown)
            });
        }
    };
    if !status.is_success() {
        eprintln!("[proxmox] LXC creation failed: {}", status.as_u16());
        let error = match data.get("errors") {
            Some(v) if !v.is_null() => v.to_string(),
            _ => jsval::str_or(data.get("message"), "LXC creation failed"),
        };
        return json!({ "success": false, "error": error });
    }

    // Fire-and-forget sshd hook attach — failures log but do not roll back.
    tokio::spawn(async move {
        let r = attach_sshd_hook_to_lxc(vmid).await;
        if r.get("success").and_then(Value::as_bool).unwrap_or(false) {
            println!("[proxmox] sshd hook attached for LXC {}", vmid as i64);
        } else {
            eprintln!(
                "[proxmox] sshd hook attach failed for LXC {}: {} {}",
                vmid as i64,
                jsval::str_or(r.get("error"), ""),
                jsval::str_or(r.get("stderr"), "")
            );
        }
    });

    json!({ "success": true, "vmid": vmid })
}

/// `cloneUserVm(email, tier, vmid, password)` (server.js:28322-28410).
pub(crate) async fn clone_user_vm(
    _state: &AppState,
    email: &str,
    tier: &str,
    vmid: f64,
    password: &str,
) -> Value {
    let _ = email;
    if pve_token().is_empty() {
        eprintln!("[proxmox] API token is not configured in environment.");
        return json!({ "success": false, "error": "Proxmox token not configured." });
    }
    if let Err(msg) = assert_vmid_in_range(vmid, "cloneUserVm") {
        return json!({ "success": false, "error": msg });
    }
    let vmid_int = vmid as i64;
    let template_id = pve_template_linux();

    // 1. Clone the template (linked clone).
    let clone_url = format!(
        "{}/nodes/{}/qemu/{}/clone",
        pve_url(),
        pve_node(),
        template_id
    );
    let mut clone_form = url::form_urlencoded::Serializer::new(String::new());
    clone_form.append_pair("newid", &js_vmid_str(vmid));
    clone_form.append_pair("name", &format!("student-{}", vmid_int));
    clone_form.append_pair("full", "0");
    clone_form.append_pair("pool", "sandboxes");
    let clone_res = pve_client_no_deadline()
        .post(&clone_url)
        .header("Authorization", pve_token())
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(clone_form.finish())
        .send()
        .await;
    let clone_res = match clone_res {
        Ok(r) => r,
        Err(err) => {
            eprintln!("[proxmox] Error during VM provisioning: {}", err);
            return json!({ "success": false, "error": err.to_string() });
        }
    };
    let clone_status = clone_res.status();
    let clone_data: Value = match clone_res.json::<Value>().await {
        Ok(v) => v,
        Err(err) => {
            eprintln!("[proxmox] Error during VM provisioning: {}", err);
            return json!({ "success": false, "error": err.to_string() });
        }
    };
    if !clone_status.is_success() || clone_data.get("data").map(Value::is_null).unwrap_or(true) {
        eprintln!(
            "[proxmox] Clone request failed: {} {}",
            clone_status.as_u16(),
            clone_data
        );
        let error = match clone_data.get("errors") {
            Some(v) if v.is_object() || v.is_array() => v.to_string(),
            Some(v) if !v.is_null() && jsval::truthy(v) => jsval::string(v),
            _ => jsval::str_or(clone_data.get("message"), "Proxmox clone failed."),
        };
        return json!({ "success": false, "error": error });
    }

    // 2. Configure memory ballooning, cores, and the cloud-init login.
    let paid = tier == "paid" || tier == "premium";
    let mem_max = if paid { 16384 } else { 4096 };
    let mem_min = if paid { 4096 } else { 2048 };
    let cores = if paid { 6 } else { 2 };

    let config_url = format!(
        "{}/nodes/{}/qemu/{}/config",
        pve_url(),
        pve_node(),
        vmid_int
    );
    let mut config_form = url::form_urlencoded::Serializer::new(String::new());
    config_form.append_pair("memory", &mem_max.to_string());
    config_form.append_pair("balloon", &mem_min.to_string());
    config_form.append_pair("cores", &cores.to_string());
    config_form.append_pair("sockets", "1");
    config_form.append_pair(
        "cipassword",
        if password.is_empty() {
            "password"
        } else {
            password
        },
    );
    config_form.append_pair("ciuser", "debian");
    let config_res = pve_client_no_deadline()
        .post(&config_url)
        .header("Authorization", pve_token())
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(config_form.finish())
        .send()
        .await;
    match config_res {
        Ok(r) if !r.status().is_success() => {
            eprintln!(
                "[proxmox] Warning: Configuration update returned status {}",
                r.status().as_u16()
            );
        }
        Err(_) => {
            eprintln!("[proxmox] Warning: Configuration update request failed");
        }
        _ => {}
    }

    // 3. Start the VM.
    let start_url = format!(
        "{}/nodes/{}/qemu/{}/status/start",
        pve_url(),
        pve_node(),
        vmid_int
    );
    let start_res = pve_client_no_deadline()
        .post(&start_url)
        .header("Authorization", pve_token())
        .send()
        .await;
    match start_res {
        Ok(r) if !r.status().is_success() => {
            eprintln!(
                "[proxmox] Warning: VM start returned status {}",
                r.status().as_u16()
            );
        }
        Err(_) => {
            eprintln!("[proxmox] Warning: VM start request failed");
        }
        _ => {}
    }

    json!({ "success": true, "vmid": vmid })
}

/// `stopUserVm(vmid)` (server.js:28412-28430).
pub(crate) async fn stop_user_vm(state: &AppState, vmid: f64) -> Value {
    if pve_token().is_empty() {
        return json!({ "success": false, "error": "Proxmox token not configured." });
    }
    let type_ = get_vm_type_by_vmid(state, vmid);
    let vmid_int = if vmid.is_finite() && vmid.fract() == 0.0 && vmid >= 0.0 {
        vmid as i64
    } else {
        return json!({ "success": false, "error": "invalid vmid" });
    };
    let url = format!(
        "{}/nodes/{}/{type_}/{vmid_int}/status/stop",
        pve_url(),
        pve_node()
    );
    match pve_client_no_deadline()
        .post(&url)
        .header("Authorization", pve_token())
        .send()
        .await
    {
        Ok(res) => {
            if res.status().as_u16() == 404 {
                return json!({ "success": true });
            }
            json!({ "success": res.status().is_success() })
        }
        Err(err) => {
            eprintln!("[proxmox] Error stopping {type_}: {}", err);
            json!({ "success": false, "error": err.to_string() })
        }
    }
}

/// `destroyUserVm(vmid)` (server.js:28432-28455).
pub(crate) async fn destroy_user_vm(state: &AppState, vmid: f64) -> Value {
    if pve_token().is_empty() {
        return json!({ "success": false, "error": "Proxmox token not configured." });
    }
    let type_ = get_vm_type_by_vmid(state, vmid);
    let vmid_int = if vmid.is_finite() && vmid.fract() == 0.0 && vmid >= 0.0 {
        vmid as i64
    } else {
        return json!({ "success": false, "error": "invalid vmid" });
    };
    let url = format!("{}/nodes/{}/{type_}/{vmid_int}", pve_url(), pve_node());
    let res = match pve_client_no_deadline()
        .delete(&url)
        .header("Authorization", pve_token())
        .send()
        .await
    {
        Ok(r) => r,
        Err(err) => {
            eprintln!("[proxmox] Error destroying {type_}: {}", err);
            return json!({ "success": false, "error": err.to_string() });
        }
    };
    let destroy_ok = res.status().is_success();
    if res.status().as_u16() == 404 {
        println!(
            "[proxmox] {} VMID {} already deleted (404). Treating as successful destruction.",
            type_, vmid_int
        );
        return json!({ "success": true });
    }
    let data: Value = match res.json::<Value>().await {
        Ok(v) => v,
        Err(err) => {
            eprintln!("[proxmox] Error destroying {type_}: {}", err);
            return json!({ "success": false, "error": err.to_string() });
        }
    };
    if !destroy_ok {
        let fallback = format!("Proxmox {type_} deletion failed.");
        return json!({
            "success": false,
            "error": jsval::str_or(data.get("errors"), &fallback),
        });
    }
    json!({ "success": true })
}

/// `terminateUserVm(vmid)` (server.js:28457-28463) — stop, wait 3s, destroy.
pub(crate) async fn terminate_user_vm(state: &AppState, vmid: f64) -> Value {
    let _ = stop_user_vm(state, vmid).await;
    tokio::time::sleep(std::time::Duration::from_millis(3000)).await;
    destroy_user_vm(state, vmid).await
}

/// `migrateLegacyVmOwnership()` (server.js:27719-27744) — lift approved
/// vm_applications entries into the virtual_machines table.
pub(crate) fn migrate_legacy_vm_ownership(state: &AppState) {
    let legacy = vm_applications(state);
    for (owner_email, app) in legacy.as_object().cloned().unwrap_or_default() {
        let vmid = app.get("vmid").and_then(jsval::number).unwrap_or(f64::NAN);
        if jsval::str_or(app.get("status"), "") != "approved"
            || !(vmid.is_finite() && vmid.fract() == 0.0)
            || vmlib::get_virtual_machine_by_vmid(&state.store, vmid).is_some()
        {
            continue;
        }
        let norm = mitch_lib::auth::normalize_email(&jsval::str_or(
            app.get("email"),
            owner_email.as_str(),
        ));
        let tier = jsval::str_or(app.get("tier"), "");
        let node_default = pve_node();
        let def_guest = if tier == "premium" { "lxc" } else { "qemu" };
        let def_host = format!("student-{}", vmid as i64);
        let def_os = if tier == "paid" {
            "Linux Desktop"
        } else {
            "Linux"
        };
        let created_opt = js_num_or_opt(app.get("approvedAt"), app.get("appliedAt"));
        let created_at = if created_opt != 0.0 && created_opt.is_finite() {
            created_opt
        } else {
            now_ms() as f64
        };
        vmlib::upsert_virtual_machine(
            &state.store,
            &json!({
                "id": format!("vm-{}", vmid as i64),
                "ownerEmail": norm,
                "ownerUserId": mitch_lib::profile::get_uid_for_email(&state.store, &state.id_secret, &norm).unwrap_or_default(),
                "vmid": vmid,
                "node": jsval::str_or(app.get("node"), &node_default),
                "guestType": jsval::str_or(app.get("type"), def_guest),
                "friendlyName": jsval::str_or(app.get("friendlyName"), "My Computer"),
                "hostname": jsval::str_or(app.get("hostname"), &def_host),
                "operatingSystem": jsval::str_or(app.get("operatingSystem"), def_os),
                "templateVmid": jsval::or(app.get("templateVmid"), json!(if tier == "paid" { jsval::num_value(pve_template_linux() as f64) } else { Value::Null })),
                "cpuCores": jsval::num_value(js_num_or(app.get("cpuCores"), if tier == "paid" { 4.0 } else { 2.0 })),
                "memoryMb": jsval::num_value(js_num_or(app.get("memoryMb"), if tier == "paid" { 8192.0 } else { 4096.0 })),
                "diskGb": jsval::num_value(js_num_or(app.get("diskGb"), if tier == "paid" { 64.0 } else { 8.0 })),
                "ipAddress": jsval::str_or(app.get("ip"), ""),
                "status": "assigned",
                "createdAt": jsval::num_value(created_at),
            }),
        );
    }
}

/// `Number(a) || Number(b) || fallback` — the `app.approvedAt ||
/// app.appliedAt || Date.now()` ladder.
fn js_num_or_opt(a: Option<&Value>, b: Option<&Value>) -> f64 {
    match a.and_then(jsval::number) {
        Some(n) if n != 0.0 && n.is_finite() => n,
        _ => js_num_or(b, 0.0),
    }
}

// ── Desktop WebSocket bridge (/api/vm/desktop/ws) ────────────────────────────

#[derive(Debug)]
struct NoCertVerifier;

impl rustls::client::danger::ServerCertVerifier for NoCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls_pki_types::CertificateDer<'_>,
        _intermediates: &[rustls_pki_types::CertificateDer<'_>],
        _server_name: &rustls_pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls_pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls_pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls_pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn make_pve_vnc_connector() -> tokio_tungstenite::Connector {
    let provider = rustls::crypto::ring::default_provider();
    let config = rustls::ClientConfig::builder_with_provider(provider.into())
        .with_safe_default_protocol_versions()
        .unwrap_or_else(|_| {
            panic!("rustls default protocol versions failed");
        })
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoCertVerifier))
        .with_no_client_auth();
    tokio_tungstenite::Connector::Rustls(Arc::new(config))
}

pub fn handle_desktop_ws_upgrade(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    search: &str,
    upgrade: Option<WebSocketUpgrade>,
) -> Option<Response> {
    if !vm_same_origin_request(headers) {
        return Some(json_response(
            403,
            json!({ "error": "WebSocket origin rejected." }),
        ));
    }
    cleanup_vm_desktop_sessions(state);
    let actor = match authenticated_vm_actor(state, headers) {
        Some(a) => a,
        None => {
            return Some(json_response(401, json!({ "error": "Sign in required." })));
        }
    };

    let session_id = search
        .trim_start_matches('?')
        .split('&')
        .find_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next()?;
            if key == "session" {
                parts.next().map(|v| {
                    form_urlencoded::parse(v.as_bytes())
                        .next()
                        .map(|(k, _)| k.into_owned())
                        .unwrap_or_else(|| v.to_string())
                })
            } else {
                None
            }
        })
        .unwrap_or_default();

    let session = {
        let sessions = state
            .vm_desktop_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        sessions.get(&session_id).cloned()
    };
    let Some(session) = session else {
        return Some(json_response(
            401,
            json!({ "error": "Desktop connection expired." }),
        ));
    };

    let used = session.get("used").map(jsval::truthy).unwrap_or(false);
    let expires_at = session
        .get("expiresAt")
        .and_then(jsval::number)
        .unwrap_or(0.0);
    if used || expires_at <= (now_ms() as f64) {
        return Some(json_response(
            401,
            json!({ "error": "Desktop connection expired." }),
        ));
    }

    let record_id = jsval::str_or(session.get("recordId"), "");
    let record_opt = vmlib::get_virtual_machine_by_id(&state.store, &record_id);
    let record_json = record_opt.as_ref().map(|r| r.to_json());
    let actor_json = actor.to_json();
    let (ok, status, _code) = validate_desktop_session(
        Some(&session),
        Some(&actor_json),
        record_json.as_ref(),
        now_ms() as f64,
        |e| mitch_lib::auth::is_admin_email(&state.store, e),
        false,
        None,
    );
    if !ok {
        let err_msg = if status == 401 {
            "Desktop connection expired."
        } else {
            "You do not have permission to access this computer."
        };
        return Some(json_response(status, json!({ "error": err_msg })));
    }

    let record_owner = record_opt
        .as_ref()
        .map(|r| r.owner_email.clone())
        .unwrap_or_default();
    let is_admin_using_other_vm = actor.is_admin
        && !record_owner.is_empty()
        && mitch_lib::auth::normalize_email(&record_owner)
            != mitch_lib::auth::normalize_email(&actor.email);
    if is_admin_using_other_vm && !actor.is_owner && !is_vm_admin_access_allowed(state, &record_id) {
        return Some(json_response(
            403,
            json!({ "error": "The owner has not allowed administrator access to this computer." }),
        ));
    }

    {
        let mut sessions = state
            .vm_desktop_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        sessions.remove(&session_id);
    }

    let Some(on_upgrade) = upgrade else {
        return Some(json_response(
            400,
            json!({ "error": "Desktop connection could not be opened." }),
        ));
    };

    let ws_data = json!({
        "isProxmoxVnc": true,
        "recordId": record_id,
        "actorEmail": actor.email,
        "sid": actor.sid,
        "authSessionKey": actor.auth_session_key,
        "ownerEmail": record_owner,
        "vmid": session.get("vmid").cloned().unwrap_or(Value::Null),
        "node": session.get("node").cloned().unwrap_or(Value::Null),
        "upstreamUrl": session.get("wsUrl").cloned().unwrap_or(Value::Null),
        "upstreamAuthorization": session.get("authorization").cloned().unwrap_or(Value::Null),
        "upstreamTlsOptions": session.get("tlsOptions").cloned().unwrap_or(Value::Null),
    });

    let st = Arc::clone(state);
    Some(on_upgrade.on_upgrade(move |socket| async move {
        run_desktop_bridge(st, socket, ws_data).await;
    }))
}

async fn run_desktop_bridge(
    state: Arc<AppState>,
    mut client_ws: axum::extract::ws::WebSocket,
    mut data: Value,
) {
    if !vm_desktop_socket_authorized(&state, &data) {
        let _ = client_ws
            .send(axum::extract::ws::Message::Close(Some(CloseFrame {
                code: 1008,
                reason: "Desktop access expired".into(),
            })))
            .await;
        return;
    }

    let sid = jsval::str_or(data.get("sid"), "");
    let actor_email = jsval::str_or(data.get("actorEmail"), "");
    let owner_email = jsval::str_or(data.get("ownerEmail"), "");
    let record_id = jsval::str_or(data.get("recordId"), "");
    let is_owner_sid = mitch_lib::auth::is_owner_id(&state.store, &state.id_secret, &sid);
    let is_admin_using_other =
        mitch_lib::auth::is_any_admin_id(&state.store, &state.id_secret, &sid, false)
            && !owner_email.is_empty()
            && mitch_lib::auth::normalize_email(&owner_email)
                != mitch_lib::auth::normalize_email(&actor_email);
    if is_admin_using_other && !is_owner_sid && !is_vm_admin_access_allowed(&state, &record_id) {
        let _ = client_ws
            .send(axum::extract::ws::Message::Close(Some(CloseFrame {
                code: 1008,
                reason: "Administrator access not permitted by owner".into(),
            })))
            .await;
        return;
    }

    if is_admin_using_other {
        let r = vmlib::get_virtual_machine_by_id(&state.store, &record_id).map(|r| r.to_json());
        println!(
            "[vm-audit] Admin {} connected to desktop console on VM {} owned by {}",
            actor_email, record_id, owner_email
        );
        vm_audit(
            &state,
            &actor_email,
            r.as_ref(),
            "ADMIN_VM_USED",
            true,
            Some(&json!({
                "operation": "desktop-stream",
                "targetUser": owner_email,
                "vmid": data.get("vmid"),
            })),
        );
        if let Some(rec) = &r {
            notify_owner_admin_used_vm(&state, rec, &actor_email, "connected to desktop console");
        }
    }

    let now = now_ms();
    data["connectedAt"] = json!(now as f64);
    state
        .vm_page_presence
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(record_id.clone(), now);

    let (close_tx, mut close_rx) = tokio::sync::mpsc::unbounded_channel::<(u16, String)>();
    let socket_id = state.vm_desktop_next_id.fetch_add(1, Ordering::Relaxed);
    {
        let mut sockets = state
            .vm_desktop_sockets
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        sockets.insert(
            socket_id,
            VmDesktopClient {
                data: data.clone(),
                close_tx,
            },
        );
    }

    struct SocketGuard {
        state: Arc<AppState>,
        socket_id: u64,
        record_id: String,
    }
    impl Drop for SocketGuard {
        fn drop(&mut self) {
            let now = now_ms();
            self.state
                .vm_page_presence
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(self.record_id.clone(), now);
            self.state
                .vm_desktop_sockets
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&self.socket_id);
        }
    }
    let _guard = SocketGuard {
        state: Arc::clone(&state),
        socket_id,
        record_id: record_id.clone(),
    };

    let upstream_url = jsval::str_or(data.get("upstreamUrl"), "");
    let upstream_auth = jsval::str_or(data.get("upstreamAuthorization"), "");

    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut req = match upstream_url.into_client_request() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("[desktop] Invalid upstream WS url: {e}");
            let _ = client_ws
                .send(axum::extract::ws::Message::Close(Some(CloseFrame {
                    code: 1011,
                    reason: "Desktop connection failed".into(),
                })))
                .await;
            return;
        }
    };
    if !upstream_auth.is_empty() {
        if let Ok(hv) = upstream_auth.parse() {
            req.headers_mut().insert("Authorization", hv);
        }
    }

    let connector = make_pve_vnc_connector();
    let upstream_pair =
        tokio_tungstenite::connect_async_tls_with_config(req, None, false, Some(connector)).await;

    let (upstream_ws, _) = match upstream_pair {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!(
                "[desktop] Proxmox console bridge failed for record {}: {}",
                record_id,
                err
            );
            let _ = client_ws
                .send(axum::extract::ws::Message::Close(Some(CloseFrame {
                    code: 1011,
                    reason: "Desktop connection failed".into(),
                })))
                .await;
            return;
        }
    };

    let (mut client_sink, mut client_stream) = client_ws.split();
    let (mut upstream_sink, mut upstream_stream) = upstream_ws.split();

    let mut last_auth_check = now_ms();

    loop {
        tokio::select! {
            biased;

            Some((code, reason)) = close_rx.recv() => {
                let _ = client_sink
                    .send(axum::extract::ws::Message::Close(Some(CloseFrame {
                        code,
                        reason: reason.into(),
                    })))
                    .await;
                break;
            }

            upstream_msg = upstream_stream.next() => {
                match upstream_msg {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(bin))) => {
                        if client_sink
                            .send(axum::extract::ws::Message::Binary(bin))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Text(txt))) => {
                        if client_sink
                            .send(axum::extract::ws::Message::Text(txt.as_str().into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Ping(p))) => {
                        if client_sink
                            .send(axum::extract::ws::Message::Ping(p))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Pong(p))) => {
                        if client_sink
                            .send(axum::extract::ws::Message::Pong(p))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) => {
                        let _ = client_sink
                            .send(axum::extract::ws::Message::Close(Some(CloseFrame {
                                code: 1012,
                                reason: "Desktop connection ended".into(),
                            })))
                            .await;
                        break;
                    }
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Frame(_))) => {}
                    Some(Err(e)) => {
                        tracing::warn!("[desktop] Upstream error: {e}");
                        let _ = client_sink
                            .send(axum::extract::ws::Message::Close(Some(CloseFrame {
                                code: 1011,
                                reason: "Desktop connection failed".into(),
                            })))
                            .await;
                        break;
                    }
                    None => {
                        let _ = client_sink
                            .send(axum::extract::ws::Message::Close(Some(CloseFrame {
                                code: 1012,
                                reason: "Desktop connection ended".into(),
                            })))
                            .await;
                        break;
                    }
                }
            }

            client_msg = client_stream.next() => {
                let Some(msg) = client_msg else {
                    break;
                };
                let msg = match msg {
                    Ok(m) => m,
                    Err(_) => break,
                };

                let now = now_ms();
                state
                    .vm_page_presence
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(record_id.clone(), now);

                if now - last_auth_check > 1000 {
                    last_auth_check = now;
                    if !vm_desktop_socket_authorized(&state, &data) {
                        let _ = client_sink
                            .send(axum::extract::ws::Message::Close(Some(CloseFrame {
                                code: 1008,
                                reason: "Desktop access expired".into(),
                            })))
                            .await;
                        break;
                    }
                }

                match msg {
                    axum::extract::ws::Message::Binary(bin) => {
                        if upstream_sink
                            .send(tokio_tungstenite::tungstenite::Message::Binary(bin))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    axum::extract::ws::Message::Text(txt) => {
                        if upstream_sink
                            .send(tokio_tungstenite::tungstenite::Message::Text(txt.as_str().into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    axum::extract::ws::Message::Ping(p) => {
                        if upstream_sink
                            .send(tokio_tungstenite::tungstenite::Message::Ping(p))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    axum::extract::ws::Message::Pong(p) => {
                        if upstream_sink
                            .send(tokio_tungstenite::tungstenite::Message::Pong(p))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    axum::extract::ws::Message::Close(_) => {
                        break;
                    }
                }
            }
        }
    }
}

pub(crate) async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
    _search: &str,
) -> Option<Response> {
    if !path.starts_with("/api/vm/") {
        return None;
    }

    // POST /api/vm/free/launch — launch or connect to the ephemeral free VM (server.js:18849-18939)
    if path == "/api/vm/free/launch" && *method == Method::POST {
        let cookies = crate::routes::me::cookies_of(state, headers);
        let sid = cookies
            .get("studentId")
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| cookies.get("id").unwrap_or(""))
            .to_string();
        if sid.is_empty()
            || !mitch_lib::auth::valid_id(&sid, &state.id_secret)
            || is_revoked_id(state, &sid)
        {
            return Some(json_response(401, json!({ "error": "auth required" })));
        }
        let email = match mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid) {
            Some(e) if !e.is_empty() => e,
            _ => return Some(json_response(401, json!({ "error": "auth required" }))),
        };
        let norm = mitch_lib::auth::normalize_email(&email);

        // Check if user already has an active free VM
        let existing = {
            let mut guard = state
                .active_free_vms
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = guard.get_mut(&norm) {
                entry["lastActive"] = json!(now_ms() as f64);
                let vmid = entry.get("vmid").and_then(jsval::number).unwrap_or(0.0);
                let password = jsval::str_or(entry.get("password"), "password").to_string();
                Some((vmid, password))
            } else {
                None
            }
        };
        if let Some((vmid, password)) = existing {
            let pve_status = get_user_vm_status(state, vmid).await;
            if pve_status
                .get("success")
                .map(jsval::truthy)
                .unwrap_or(false)
                && jsval::str_or(pve_status.get("status"), "") == "stopped"
            {
                let _ = power_user_vm(state, vmid, "start").await;
            }
            let suffix = if vmid >= 300.0 { vmid - 200.0 } else { vmid } as i64;
            let fallback_ip = format!("10.0.0.{suffix}");
            let ip = jsval::str_or(pve_status.get("ip"), &fallback_ip);
            return Some(json_response(
                200,
                json!({
                    "success": true,
                    "ip": ip,
                    "vmid": vmid,
                    "password": password,
                }),
            ));
        }

        // Check pool capacity (Max 10) by querying Proxmox directly
        let existing_ids = get_existing_vmids().await;
        let mut target_vmid: Option<i64> = None;
        for id in 200..210 {
            if !existing_ids.contains_key(&id) {
                target_vmid = Some(id);
                break;
            }
        }

        if target_vmid.is_none() {
            let mut oldest_id = 200;
            let active_vmids: std::collections::HashSet<i64> = state
                .active_free_vms
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .values()
                .filter_map(|v| v.get("vmid").and_then(jsval::number).map(|n| n as i64))
                .collect();

            for id in 200..210 {
                if existing_ids.contains_key(&id) && !active_vmids.contains(&id) {
                    oldest_id = id;
                    break;
                }
            }

            if active_vmids.contains(&oldest_id) {
                let mut oldest_user: Option<String> = None;
                let mut oldest_time = f64::INFINITY;
                {
                    let mut active_lock = state
                        .active_free_vms
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    for (user, data) in active_lock.iter() {
                        let last_active = data
                            .get("lastActive")
                            .and_then(jsval::number)
                            .unwrap_or(0.0);
                        if last_active < oldest_time {
                            oldest_time = last_active;
                            oldest_user = Some(user.clone());
                        }
                    }
                    if let Some(ou) = oldest_user {
                        if let Some(entry) = active_lock.remove(&ou) {
                            if let Some(v) = entry.get("vmid").and_then(jsval::number) {
                                oldest_id = v as i64;
                            }
                        }
                    }
                }
            }

            println!("[free-vm] Evicting VMID {oldest_id} to free up slot.");
            let _ = terminate_user_vm(state, oldest_id as f64).await;
            target_vmid = Some(oldest_id);
        }

        let target_vmid = target_vmid.unwrap_or(200);
        let secure_password = js_random_password();
        let clone_result =
            create_lxc_container(state, &email, "free", target_vmid as f64, &secure_password).await;

        if !clone_result
            .get("success")
            .map(jsval::truthy)
            .unwrap_or(false)
        {
            return Some(json_response(
                500,
                json!({ "error": clone_result.get("error") }),
            ));
        }

        let now = now_ms() as f64;
        state
            .active_free_vms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                norm,
                json!({
                    "vmid": target_vmid,
                    "password": secure_password,
                    "startedAt": now,
                    "lastActive": now,
                }),
            );

        tokio::time::sleep(Duration::from_millis(4000)).await;
        let pve_status = get_user_vm_status(state, target_vmid as f64).await;
        let suffix = if target_vmid >= 300 {
            target_vmid - 200
        } else {
            target_vmid
        };
        let fallback_ip = format!("10.0.0.{suffix}");
        let ip = jsval::str_or(pve_status.get("ip"), &fallback_ip);

        return Some(json_response(
            200,
            json!({
                "success": true,
                "vmid": target_vmid,
                "ip": ip,
                "password": secure_password,
            }),
        ));
    }

    // POST /api/vm/apply (server.js:18942-18994)
    if path == "/api/vm/apply" && *method == Method::POST {
        let cookies = crate::routes::me::cookies_of(state, headers);
        let sid = cookies
            .get("studentId")
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| cookies.get("id").unwrap_or(""))
            .to_string();
        if sid.is_empty()
            || !mitch_lib::auth::valid_id(&sid, &state.id_secret)
            || is_revoked_id(state, &sid)
        {
            return Some(json_response(401, json!({ "error": "auth required" })));
        }
        let email = match mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid) {
            Some(e) if !e.is_empty() => e,
            _ => return Some(json_response(401, json!({ "error": "auth required" }))),
        };

        let body: Value = match serde_json::from_slice(body_bytes) {
            Ok(v) => v,
            Err(_) => {
                return Some(json_response(
                    400,
                    json!({ "error": "Invalid JSON payload" }),
                ));
            }
        };

        let tier = jsval::str_or(body.get("tier"), "").trim().to_lowercase();
        let note = jsval::str_or(body.get("note"), "")
            .trim()
            .chars()
            .take(1000)
            .collect::<String>();

        if tier != "premium" && tier != "paid" {
            return Some(json_response(
                400,
                json!({ "error": "Invalid tier requested" }),
            ));
        }

        let mut data = vm_applications(state);
        let norm = mitch_lib::auth::normalize_email(&email);

        if let Some(app) = data.get(&norm) {
            let status = jsval::str_or(app.get("status"), "");
            if status == "pending" || status == "approved" || status == "deleted" {
                let err_msg = if status == "pending" {
                    "You already have a VM request awaiting review."
                } else {
                    "You already have a VM assigned to this account. Contact an admin to change it."
                };
                return Some(json_response(409, json!({ "error": err_msg })));
            }
        }

        let is_admin = mitch_lib::auth::is_admin_email(&state.store, &email);
        let is_premium = mitch_lib::auth::is_premium_email(&state.store, &norm);
        let premium_included = tier == "premium" && is_premium;
        let complimentary = is_admin || premium_included;

        let entry = json!({
            "email": email,
            "tier": tier,
            "os": "linux",
            "note": note,
            "billing": if complimentary {
                if is_admin { "admin_comped" } else { "premium_included" }
            } else {
                "standard"
            },
            "priceUsd": if complimentary { json!(0) } else { Value::Null },
            "status": "pending",
            "appliedAt": now_ms() as f64,
        });

        if let Some(obj) = data.as_object_mut() {
            obj.insert(norm, entry);
        }
        save_vm_json(state, "vm_applications.json", &data);

        return Some(json_response(
            200,
            json!({
                "success": true,
                "complimentary": complimentary,
                "message": if complimentary {
                    "Your no-cost workspace request was submitted. Approval and capacity checks still apply."
                } else {
                    "Application submitted successfully. Please contact Mitch to complete approval."
                },
            }),
        ));
    }

    // GET /api/vm/status (server.js:18997-19055)
    if path == "/api/vm/status" && *method == Method::GET {
        let cookies = crate::routes::me::cookies_of(state, headers);
        let sid = cookies
            .get("studentId")
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| cookies.get("id").unwrap_or(""))
            .to_string();
        if sid.is_empty()
            || !mitch_lib::auth::valid_id(&sid, &state.id_secret)
            || is_revoked_id(state, &sid)
        {
            return Some(json_response(401, json!({ "error": "auth required" })));
        }
        let email = match mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid) {
            Some(e) if !e.is_empty() => e,
            _ => return Some(json_response(401, json!({ "error": "auth required" }))),
        };

        let norm = mitch_lib::auth::normalize_email(&email);
        let is_premium = mitch_lib::auth::is_premium_email(&state.store, &norm);
        let is_admin = mitch_lib::auth::is_admin_email(&state.store, &email);

        let access = json!({
            "isPremium": is_premium,
            "isAdmin": is_admin,
            "adminVmBenefit": is_admin,
            "platform": "mitch.pro Linux / Proxmox",
        });

        // Ephemeral free VM
        let free_entry = {
            let mut guard = state
                .active_free_vms
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(free_vm) = guard.get_mut(&norm) {
                free_vm["lastActive"] = json!(now_ms() as f64);
                let vmid = free_vm.get("vmid").and_then(jsval::number).unwrap_or(0.0);
                let password = jsval::str_or(free_vm.get("password"), "password").to_string();
                Some((vmid, password))
            } else {
                None
            }
        };
        if let Some((vmid, password)) = free_entry {
            let pve_status = get_user_vm_status(state, vmid).await;
            let vm_status = if pve_status
                .get("success")
                .map(jsval::truthy)
                .unwrap_or(false)
            {
                jsval::str_or(pve_status.get("status"), "unknown")
            } else {
                "unknown".to_string()
            };
            let ip = if pve_status
                .get("success")
                .map(jsval::truthy)
                .unwrap_or(false)
            {
                jsval::str_or(pve_status.get("ip"), "")
            } else {
                String::new()
            };

            let mut resp = json!({
                "status": "approved",
                "vmid": vmid,
                "tier": "free",
                "vmStatus": vm_status,
                "ip": ip,
                "password": password,
            });
            if let (Some(r), Some(a)) = (resp.as_object_mut(), access.as_object()) {
                for (k, v) in a {
                    r.insert(k.clone(), v.clone());
                }
            }
            return Some(json_response(200, resp));
        }

        let data = vm_applications(state);
        let app = data.get(&norm);
        let Some(app) = app else {
            let mut resp = json!({ "status": "none" });
            if let (Some(r), Some(a)) = (resp.as_object_mut(), access.as_object()) {
                for (k, v) in a {
                    r.insert(k.clone(), v.clone());
                }
            }
            return Some(json_response(200, resp));
        };

        let status = jsval::str_or(app.get("status"), "");
        if status == "pending" {
            let price_usd = app.get("priceUsd").and_then(jsval::number);
            let complimentary = is_admin || price_usd == Some(0.0);
            let mut resp = json!({
                "status": "pending",
                "tier": app.get("tier"),
                "complimentary": complimentary,
            });
            if let (Some(r), Some(a)) = (resp.as_object_mut(), access.as_object()) {
                for (k, v) in a {
                    r.insert(k.clone(), v.clone());
                }
            }
            return Some(json_response(200, resp));
        }

        let vmid = app.get("vmid").and_then(jsval::number);
        if status == "approved" {
            if let Some(vmid) = vmid {
                let pve_status = get_user_vm_status(state, vmid).await;
                let vm_status = if pve_status
                    .get("success")
                    .map(jsval::truthy)
                    .unwrap_or(false)
                {
                    jsval::str_or(pve_status.get("status"), "unknown")
                } else {
                    "unknown".to_string()
                };
                let ip = if pve_status
                    .get("success")
                    .map(jsval::truthy)
                    .unwrap_or(false)
                {
                    jsval::str_or(pve_status.get("ip"), "")
                } else {
                    String::new()
                };
                let price_usd = app.get("priceUsd").and_then(jsval::number);
                let complimentary = is_admin || price_usd == Some(0.0);

                let mut resp = json!({
                    "status": "approved",
                    "vmid": vmid,
                    "tier": app.get("tier"),
                    "vmStatus": vm_status,
                    "ip": ip,
                    "password": jsval::str_or(app.get("password"), "password"),
                    "complimentary": complimentary,
                });
                if let (Some(r), Some(a)) = (resp.as_object_mut(), access.as_object()) {
                    for (k, v) in a {
                        r.insert(k.clone(), v.clone());
                    }
                }
                return Some(json_response(200, resp));
            }
        }

        let mut resp = json!({ "status": "none" });
        if let (Some(r), Some(a)) = (resp.as_object_mut(), access.as_object()) {
            for (k, v) in a {
                r.insert(k.clone(), v.clone());
            }
        }
        return Some(json_response(200, resp));
    }

    // POST /api/vm/free/reset (server.js:19058-19137)
    if path == "/api/vm/free/reset" && *method == Method::POST {
        let cookies = crate::routes::me::cookies_of(state, headers);
        let sid = cookies
            .get("studentId")
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| cookies.get("id").unwrap_or(""))
            .to_string();
        if sid.is_empty()
            || !mitch_lib::auth::valid_id(&sid, &state.id_secret)
            || is_revoked_id(state, &sid)
        {
            return Some(json_response(401, json!({ "error": "auth required" })));
        }
        let email = match mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid) {
            Some(e) if !e.is_empty() => e,
            _ => return Some(json_response(401, json!({ "error": "auth required" }))),
        };
        let norm = mitch_lib::auth::normalize_email(&email);

        let old_vmid = {
            let mut active = state
                .active_free_vms
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            active
                .remove(&norm)
                .and_then(|v| v.get("vmid").and_then(jsval::number))
        };
        if let Some(vmid) = old_vmid {
            println!("[free-vm] User {email} requested reset. Wiping old VMID {vmid}");
            let _ = terminate_user_vm(state, vmid).await;
        }

        let existing_ids = get_existing_vmids().await;
        let mut target_vmid: Option<i64> = None;
        for id in 200..210 {
            if !existing_ids.contains_key(&id) {
                target_vmid = Some(id);
                break;
            }
        }

        if target_vmid.is_none() {
            let mut oldest_id = 200;
            let active_vmids: std::collections::HashSet<i64> = state
                .active_free_vms
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .values()
                .filter_map(|v| v.get("vmid").and_then(jsval::number).map(|n| n as i64))
                .collect();

            for id in 200..210 {
                if existing_ids.contains_key(&id) && !active_vmids.contains(&id) {
                    oldest_id = id;
                    break;
                }
            }

            if active_vmids.contains(&oldest_id) {
                let mut oldest_user: Option<String> = None;
                let mut oldest_time = f64::INFINITY;
                {
                    let mut active_lock = state
                        .active_free_vms
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    for (user, data) in active_lock.iter() {
                        let last_active = data
                            .get("lastActive")
                            .and_then(jsval::number)
                            .unwrap_or(0.0);
                        if last_active < oldest_time {
                            oldest_time = last_active;
                            oldest_user = Some(user.clone());
                        }
                    }
                    if let Some(ou) = oldest_user {
                        if let Some(entry) = active_lock.remove(&ou) {
                            if let Some(v) = entry.get("vmid").and_then(jsval::number) {
                                oldest_id = v as i64;
                            }
                        }
                    }
                }
            }

            println!("[free-vm] Reset eviction: wiping VMID {oldest_id}");
            let _ = terminate_user_vm(state, oldest_id as f64).await;
            target_vmid = Some(oldest_id);
        }

        let target_vmid = target_vmid.unwrap_or(200);
        let secure_password = js_random_password();
        let clone_result =
            create_lxc_container(state, &email, "free", target_vmid as f64, &secure_password).await;

        if !clone_result
            .get("success")
            .map(jsval::truthy)
            .unwrap_or(false)
        {
            return Some(json_response(
                500,
                json!({ "error": clone_result.get("error") }),
            ));
        }

        let now = now_ms() as f64;
        state
            .active_free_vms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                norm,
                json!({
                    "vmid": target_vmid,
                    "password": secure_password,
                    "startedAt": now,
                    "lastActive": now,
                }),
            );

        tokio::time::sleep(Duration::from_millis(4000)).await;
        let pve_status = get_user_vm_status(state, target_vmid as f64).await;
        let suffix = if target_vmid >= 300 {
            target_vmid - 200
        } else {
            target_vmid
        };
        let fallback_ip = format!("10.0.0.{suffix}");
        let ip = jsval::str_or(pve_status.get("ip"), &fallback_ip);

        return Some(json_response(
            200,
            json!({
                "success": true,
                "vmid": target_vmid,
                "ip": ip,
                "password": secure_password,
            }),
        ));
    }

    // POST /api/vm/power (legacy user VM power, server.js:19140-19173)
    if path == "/api/vm/power" && *method == Method::POST {
        let cookies = crate::routes::me::cookies_of(state, headers);
        let sid = cookies
            .get("studentId")
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| cookies.get("id").unwrap_or(""))
            .to_string();
        if sid.is_empty()
            || !mitch_lib::auth::valid_id(&sid, &state.id_secret)
            || is_revoked_id(state, &sid)
        {
            return Some(json_response(401, json!({ "error": "auth required" })));
        }
        let email = match mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid) {
            Some(e) if !e.is_empty() => e,
            _ => return Some(json_response(401, json!({ "error": "auth required" }))),
        };

        let body: Value = match serde_json::from_slice(body_bytes) {
            Ok(v) => v,
            Err(_) => {
                return Some(json_response(
                    400,
                    json!({ "error": "Invalid JSON payload" }),
                ));
            }
        };

        let action = jsval::str_or(body.get("action"), "").trim().to_lowercase();
        if action != "start" && action != "stop" && action != "reboot" {
            return Some(json_response(
                400,
                json!({ "error": "Invalid power action" }),
            ));
        }

        let data = vm_applications(state);
        let norm = mitch_lib::auth::normalize_email(&email);
        let app = data.get(&norm);

        let Some(app) = app else {
            return Some(json_response(
                400,
                json!({ "error": "No approved VM workspace found" }),
            ));
        };
        if jsval::str_or(app.get("status"), "") != "approved" || app.get("vmid").is_none() {
            return Some(json_response(
                400,
                json!({ "error": "No approved VM workspace found" }),
            ));
        }

        let vmid = app.get("vmid").and_then(jsval::number).unwrap_or(0.0);
        let result = power_user_vm(state, vmid, &action).await;
        if !result.get("success").map(jsval::truthy).unwrap_or(false) {
            return Some(json_response(500, json!({ "error": result.get("error") })));
        }

        return Some(json_response(
            200,
            json!({
                "success": true,
                "message": format!("VM {action} command sent successfully."),
            }),
        ));
    }

    // GET /api/vm/computers (server.js:19178-19197)
    if path == "/api/vm/computers" && *method == Method::GET {
        let actor = match authenticated_vm_actor(state, headers) {
            Some(a) => a,
            None => {
                return Some(json_response(
                    401,
                    json!({ "error": "Sign in to view your computers." }),
                ));
            }
        };

        let records = vmlib::get_virtual_machines_for_owner(&state.store, &actor.email);
        let mut computers: Vec<Value> = Vec::with_capacity(records.len());
        let actor_json = actor.to_json();

        for rec in records {
            let record = rec.to_json();
            let record_id = jsval::str_or(record.get("id"), "");
            let runtime = match crate::proxmox_desktop::desktop().get_status(&record).await {
                Ok(rt) => {
                    if let Some(ip) = rt.get("ipAddress").and_then(Value::as_str) {
                        vmlib::update_virtual_machine_runtime(
                            &state.store,
                            &record_id,
                            Some(ip),
                            None,
                        );
                    }
                    rt
                }
                Err(e) => {
                    tracing::warn!("get_status failed for VM {record_id}: {e}");
                    json!({ "state": "unavailable" })
                }
            };
            computers.push(public_vm_record(
                state,
                &record,
                Some(&runtime),
                Some(&actor_json),
            ));
        }

        return Some(json_response(
            200,
            json!({
                "computers": computers,
                "serviceAvailable": crate::proxmox_desktop::desktop().configured(),
                "isEligible": is_eligible_for_free_vm(state, &actor.email, actor.is_admin),
            }),
        ));
    }

    // GET /api/vm/upgrades (server.js:19199-19217)
    if path == "/api/vm/upgrades" && *method == Method::GET {
        let actor = match authenticated_vm_actor(state, headers) {
            Some(a) => a,
            None => {
                return Some(json_response(401, json!({ "error": "Sign in required." })));
            }
        };

        let current = get_user_vm_upgrades(state, &actor.email);
        let coins = mitch_lib::coins::get_coins(&state.store, state.data_dir(), &actor.email);
        let running_resources = get_running_non_admin_vm_resources(state).await;

        return Some(json_response(
            200,
            json!({
                "catalog": mitch_lib::vm_security::vm_upgrade_catalog(),
                "current": current,
                "coins": coins,
                "fleet": {
                    "runningCores": running_resources.get("cores").cloned().unwrap_or(json!(0)),
                    "runningMemoryMb": running_resources.get("memoryMb").cloned().unwrap_or(json!(0)),
                    "maxCores": VM_FLEET_MAX_CORES,
                    "maxMemoryMb": VM_FLEET_MAX_MEMORY_MB,
                },
            }),
        ));
    }

    // POST /api/vm/upgrade (server.js:19219-19338)
    if path == "/api/vm/upgrade" && *method == Method::POST {
        let actor = match authenticated_vm_actor(state, headers) {
            Some(a) => a,
            None => {
                return Some(json_response(401, json!({ "error": "Sign in required." })));
            }
        };

        if !vm_same_origin_request(headers) {
            return Some(json_response(
                403,
                json!({ "error": "Request origin rejected." }),
            ));
        }

        let body: Value = if body_bytes.is_empty() {
            json!({})
        } else {
            match serde_json::from_slice(body_bytes) {
                Ok(v) => v,
                Err(_) => {
                    return Some(json_response(400, json!({ "error": "Invalid request." })));
                }
            }
        };

        let category = jsval::str_or(body.get("category"), "")
            .trim()
            .to_lowercase();
        let target_value = body
            .get("targetValue")
            .and_then(jsval::number)
            .unwrap_or(f64::NAN);
        if !["cpu", "ram", "disk", "session"].contains(&category.as_str())
            || !target_value.is_finite()
        {
            return Some(json_response(
                400,
                json!({ "error": "Invalid category or target value." }),
            ));
        }

        let catalog = mitch_lib::vm_security::vm_upgrade_catalog();
        let tiers = catalog.get(&category).and_then(Value::as_array);
        let target_tier = tiers.and_then(|arr| {
            arr.iter()
                .find(|t| t.get("value").and_then(jsval::number) == Some(target_value))
        });
        let Some(target_tier) = target_tier else {
            return Some(json_response(
                400,
                json!({ "error": "Target upgrade tier not found in catalog." }),
            ));
        };

        let duration = if jsval::str_or(body.get("duration"), "month").to_lowercase() == "week" {
            "week"
        } else {
            "month"
        };
        let duration_days = if duration == "week" { 7.0 } else { 30.0 };

        let current_upgrades = get_user_vm_upgrades(state, &actor.email);
        let current_value = match category.as_str() {
            "cpu" => current_upgrades
                .get("cpuCores")
                .and_then(jsval::number)
                .unwrap_or(0.0),
            "ram" => current_upgrades
                .get("memoryMb")
                .and_then(jsval::number)
                .unwrap_or(0.0),
            "disk" => current_upgrades
                .get("diskGb")
                .and_then(jsval::number)
                .unwrap_or(0.0),
            "session" => current_upgrades
                .get("dailyMaxSeconds")
                .and_then(jsval::number)
                .unwrap_or(0.0),
            _ => 0.0,
        };

        let is_session_renewal = category == "session"
            && target_value == current_value
            && current_value > VM_DAILY_MAX_SECONDS;
        if target_value < current_value || (target_value == current_value && !is_session_renewal) {
            return Some(json_response(
                400,
                json!({ "error": "You already possess this tier or a higher tier." }),
            ));
        }

        let current_tier = tiers.and_then(|arr| {
            arr.iter()
                .find(|t| t.get("value").and_then(jsval::number) == Some(current_value))
        });
        let current_cost = current_tier
            .and_then(|t| t.get("cost"))
            .and_then(jsval::number)
            .unwrap_or(0.0);
        let target_cost = target_tier
            .get("cost")
            .and_then(jsval::number)
            .unwrap_or(0.0);
        let mut cost = if is_session_renewal {
            target_cost
        } else {
            (target_cost - current_cost).max(0.0)
        };
        if category == "session" && duration == "week" {
            cost = (cost * 0.35).round();
        }

        let user_coins = mitch_lib::coins::get_coins(&state.store, state.data_dir(), &actor.email);
        if user_coins < cost {
            return Some(json_response(
                402,
                json!({
                    "error": format!(
                        "Insufficient Mitch Coins. You need {} coins, but only have {}.",
                        cost as i64, user_coins.floor() as i64
                    ),
                    "code": "insufficient_coins",
                    "required": cost,
                    "current": user_coins,
                }),
            ));
        }

        let user_vms: Vec<Value> =
            vmlib::get_virtual_machines_for_owner(&state.store, &actor.email)
                .into_iter()
                .map(|r| r.to_json())
                .filter(|r| jsval::str_or(r.get("status"), "") != "unassigned")
                .collect();
        let mut vm_record = user_vms.into_iter().next();
        let mut is_vm_running = false;
        if let Some(rec) = &vm_record {
            if crate::proxmox_desktop::desktop().configured() {
                if let Ok(rt) = crate::proxmox_desktop::desktop().get_status(rec).await {
                    is_vm_running = jsval::str_or(rt.get("state"), "") == "running";
                }
            }
        }

        if is_vm_running && !actor.is_admin {
            let additional_cores = if category == "cpu" {
                target_value - current_value
            } else {
                0.0
            };
            let additional_mem = if category == "ram" {
                target_value - current_value
            } else {
                0.0
            };
            if additional_cores > 0.0 || additional_mem > 0.0 {
                let cap_check =
                    can_accommodate_vm_resources(state, additional_cores, additional_mem).await;
                if !cap_check.get("ok").map(jsval::truthy).unwrap_or(false) {
                    let target_label = jsval::str_or(target_tier.get("label"), "");
                    notify_capacity_full_on_attempt(
                        state,
                        &actor.email,
                        &format!("upgrade {category} to {target_label}"),
                        &json!({
                            "cores": cap_check.get("wouldCores"),
                            "memoryMb": cap_check.get("wouldMemoryMb"),
                        }),
                    );
                    let cur_c = cap_check
                        .get("currentCores")
                        .and_then(jsval::number)
                        .unwrap_or(0.0) as i64;
                    let max_c = cap_check
                        .get("maxCores")
                        .and_then(jsval::number)
                        .unwrap_or(0.0) as i64;
                    let cur_m = (cap_check
                        .get("currentMemoryMb")
                        .and_then(jsval::number)
                        .unwrap_or(0.0)
                        / 1024.0)
                        .round() as i64;
                    let max_m = (cap_check
                        .get("maxMemoryMb")
                        .and_then(jsval::number)
                        .unwrap_or(0.0)
                        / 1024.0)
                        .round() as i64;
                    return Some(json_response(
                        409,
                        json!({
                            "error": format!(
                                "Host capacity reached: not enough resources to apply this upgrade while your computer is running ({cur_c}/{max_c} cores, {cur_m}/{max_m} GB RAM). Stop your computer to upgrade, or try again later."
                            ),
                            "code": "capacity_limit_reached",
                        }),
                    ));
                }
            }
        }

        let target_label = jsval::str_or(target_tier.get("label"), "");
        if cost > 0.0 {
            mitch_lib::coins::add_coins(
                &state.store,
                state.data_dir(),
                &actor.email,
                -cost,
                1.0,
                &format!("vm-upgrade: {category} to {target_label}"),
            );
        }

        let new_upgrades =
            save_user_vm_upgrade(state, &actor.email, &category, target_value, duration_days);

        if let Some(rec) = vm_record.as_mut() {
            let rec_id = jsval::str_or(rec.get("id"), "");
            let mut update_specs = json!({});
            if category == "cpu" {
                update_specs["cpuCores"] = json!(target_value);
            }
            if category == "ram" {
                update_specs["memoryMb"] = json!(target_value);
            }
            if category == "disk" {
                update_specs["diskGb"] = json!(target_value);
            }

            let cpu_opt = update_specs.get("cpuCores").and_then(jsval::number);
            let mem_opt = update_specs.get("memoryMb").and_then(jsval::number);
            let disk_opt = update_specs.get("diskGb").and_then(jsval::number);

            if update_specs
                .as_object()
                .map(|o| !o.is_empty())
                .unwrap_or(false)
            {
                vmlib::update_virtual_machine_specs(
                    &state.store,
                    &rec_id,
                    cpu_opt,
                    mem_opt,
                    disk_opt,
                );
                if let Some(c) = update_specs.get("cpuCores") {
                    rec["cpuCores"] = c.clone();
                }
                if let Some(m) = update_specs.get("memoryMb") {
                    rec["memoryMb"] = m.clone();
                }
                if let Some(d) = update_specs.get("diskGb") {
                    rec["diskGb"] = d.clone();
                }

                if crate::proxmox_desktop::desktop().configured() {
                    if let Err(hw_err) = crate::proxmox_desktop::desktop()
                        .update_hardware(rec, cpu_opt, mem_opt, disk_opt)
                        .await
                    {
                        tracing::warn!(
                            "[vm-upgrade] Hardware update warning for {rec_id}: {hw_err:?}"
                        );
                    }
                }
            }

            vm_audit(
                state,
                &actor.email,
                Some(rec),
                "VM_UPGRADED",
                true,
                Some(&json!({
                    "category": category,
                    "targetValue": target_value,
                    "cost": cost,
                    "newUpgrades": new_upgrades,
                })),
            );
        }

        return Some(json_response(
            200,
            json!({
                "success": true,
                "message": format!("Successfully upgraded {category} to {target_label}!"),
                "upgrades": new_upgrades,
                "coins": mitch_lib::coins::get_coins(&state.store, state.data_dir(), &actor.email),
            }),
        ));
    }

    // POST /api/vm/my-computer/create (server.js:19340-19420)
    if path == "/api/vm/my-computer/create" && *method == Method::POST {
        let actor = match authenticated_vm_actor(state, headers) {
            Some(a) => a,
            None => {
                return Some(json_response(401, json!({ "error": "Sign in required." })));
            }
        };

        if !vm_same_origin_request(headers) {
            return Some(json_response(
                403,
                json!({ "error": "Request origin rejected." }),
            ));
        }

        if !is_eligible_for_free_vm(state, &actor.email, actor.is_admin) {
            return Some(json_response(
                403,
                json!({ "error": "Free computers are available to RJUHSD students (@student.rjuhsd.us) and Premium members." }),
            ));
        }

        let existing: Vec<Value> =
            vmlib::get_virtual_machines_for_owner(&state.store, &actor.email)
                .into_iter()
                .map(|r| r.to_json())
                .filter(|r| jsval::str_or(r.get("status"), "") != "unassigned")
                .collect();
        if !existing.is_empty() {
            return Some(json_response(
                409,
                json!({ "error": "You already have a computer assigned. Use the recreate option if you want to start fresh." }),
            ));
        }

        let user_upgrades = get_user_vm_upgrades(state, &actor.email);
        let daily_max = user_upgrades
            .get("dailyMaxSeconds")
            .and_then(jsval::number)
            .unwrap_or(21600.0);
        let cpu_cores = user_upgrades
            .get("cpuCores")
            .and_then(jsval::number)
            .unwrap_or(2.0);
        let memory_mb = user_upgrades
            .get("memoryMb")
            .and_then(jsval::number)
            .unwrap_or(4096.0);
        let disk_gb = user_upgrades
            .get("diskGb")
            .and_then(jsval::number)
            .unwrap_or(32.0);

        if !actor.is_admin {
            let used_today = get_user_daily_vm_usage(state, &actor.email);
            if is_daily_vm_limit_reached(used_today, false, daily_max) {
                return Some(json_response(
                    429,
                    json!({
                        "error": format!(
                            "Daily limit reached: you have used your maximum {} hours of computer time for today. Your daily limit will reset tomorrow.",
                            (daily_max / 3600.0).round() as i64
                        ),
                        "code": "daily_limit_reached",
                        "dailyRemainingSeconds": 0,
                    }),
                ));
            }
            let cooldown_remaining = get_vm_cooldown_remaining(state, &actor.email, actor.is_admin);
            if cooldown_remaining > 0.0 {
                let mins = (cooldown_remaining / 60.0).ceil() as i64;
                return Some(json_response(
                    429,
                    json!({
                        "error": format!(
                            "Computer is cooling down. You can start/create a computer in {mins} minute{}.",
                            if mins == 1 { "" } else { "s" }
                        ),
                        "code": "vm_cooldown_active",
                    }),
                ));
            }
            let cap_check = can_accommodate_vm_resources(state, cpu_cores, memory_mb).await;
            if !cap_check.get("ok").map(jsval::truthy).unwrap_or(false) {
                notify_capacity_full_on_attempt(
                    state,
                    &actor.email,
                    "create a computer",
                    &json!({
                        "cores": cap_check.get("wouldCores"),
                        "memoryMb": cap_check.get("wouldMemoryMb"),
                    }),
                );
                return Some(json_response(
                    409,
                    json!({
                        "error": format!("Server capacity reached: a maximum of {VM_MAX_CONCURRENT_RUNNING} computers can run at once. Please try again later."),
                        "code": "capacity_limit_reached",
                    }),
                ));
            }
        }

        let body: Value = if body_bytes.is_empty() {
            json!({})
        } else {
            match serde_json::from_slice(body_bytes) {
                Ok(v) => v,
                Err(_) => {
                    return Some(json_response(400, json!({ "error": "Invalid request." })));
                }
            }
        };

        let lock_key = format!("create-{}", actor.email);
        {
            let mut requests = state
                .vm_power_requests
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if requests.contains_key(&lock_key) {
                return Some(json_response(
                    409,
                    json!({ "error": "A computer is already being created for your account." }),
                ));
            }
            requests.insert(lock_key.clone(), json!({ "startedAt": now_ms() as f64 }));
        }

        let email_prefix = actor.email.split('@').next().unwrap_or("student");
        let mut base_user = email_prefix.to_lowercase();
        base_user.retain(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if !base_user.starts_with(|c: char| c.is_ascii_lowercase()) {
            base_user = format!("u{base_user}");
        }
        base_user.truncate(30);
        if matches!(base_user.as_str(), "root" | "daemon" | "nobody" | "ubuntu") {
            base_user = format!("u{base_user}");
        }

        let requested_username = jsval::str_or(body.get("desktopUsername"), &base_user)
            .trim()
            .to_lowercase();
        let raw_password = jsval::str_or(body.get("desktopPassword"), "");
        let desktop_login = match crate::proxmox_desktop::desktop()
            .validate_desktop_login(&requested_username, &raw_password)
        {
            Ok(dl) => dl,
            Err(_) => {
                state
                    .vm_power_requests
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&lock_key);
                return Some(json_response(
                    400,
                    json!({
                        "error": "Choose a password of 8 to 128 characters.",
                        "code": "invalid_desktop_login",
                    }),
                ));
            }
        };

        let template_vmid = crate::proxmox_desktop::desktop()
            .template_vmids()
            .first()
            .copied()
            .unwrap_or(9010) as f64;

        let all_records = vmlib::list_virtual_machines(&state.store, true);
        let existing_vmids: Vec<f64> = all_records.iter().map(|r| r.vmid).collect();

        let vmid = match crate::proxmox_desktop::desktop()
            .next_available_vmid(PVE_VMID_MIN, PVE_VMID_MAX, &existing_vmids)
            .await
        {
            Ok(v) => v as f64,
            Err(e) => {
                state
                    .vm_power_requests
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&lock_key);
                let friendly = friendly_vm_error(&VmError::Service(e));
                return Some(json_response(
                    friendly.0,
                    json!({ "error": friendly.1, "code": friendly.2 }),
                ));
            }
        };

        let hostname = format!("student-{}", vmid as i64);
        let pending_rec = vmlib::reserve_virtual_machine(
            &state.store,
            &json!({
                "id": format!("vm-{}", vmid as i64),
                "ownerEmail": actor.email,
                "ownerUserId": mitch_lib::profile::get_uid_for_email(&state.store, &state.id_secret, &actor.email).unwrap_or_default(),
                "vmid": vmid,
                "node": crate::proxmox_desktop::desktop().node(),
                "guestType": "qemu",
                "friendlyName": "My Computer",
                "hostname": hostname,
                "operatingSystem": "Linux Desktop",
                "templateVmid": template_vmid,
                "cpuCores": cpu_cores,
                "memoryMb": memory_mb,
                "diskGb": disk_gb,
                "status": "provisioning",
                "createdAt": now_ms() as f64,
            }),
        ).map(|r| r.to_json());

        let Some(pending_record) = pending_rec else {
            state
                .vm_power_requests
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&lock_key);
            return Some(json_response(
                500,
                json!({ "error": "Could not reserve computer slot. Please try again." }),
            ));
        };

        let clone_params = crate::proxmox_desktop::CloneDesktopParams {
            template_vmid: Some(template_vmid),
            vmid: Some(vmid),
            hostname: hostname.clone(),
            cpu_cores: Some(cpu_cores),
            memory_mb: Some(memory_mb),
            disk_gb: Some(disk_gb),
            desktop_username: desktop_login.0,
            desktop_password: desktop_login.1,
        };

        let clone_res = crate::proxmox_desktop::desktop()
            .clone_desktop(&clone_params)
            .await;
        state
            .vm_power_requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&lock_key);

        match clone_res {
            Ok(created) => {
                let mut merged = js_object_merge(&pending_record, &created);
                merged["status"] = json!("assigned");
                let record = vmlib::upsert_virtual_machine(&state.store, &merged)
                    .map(|r| r.to_json())
                    .unwrap_or(merged);
                let rec_id = jsval::str_or(record.get("id"), "");
                state
                    .vm_page_presence
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(rec_id, now_ms());
                vm_audit(state, &actor.email, Some(&record), "VM_CREATED", true, None);
                let pub_rec = public_vm_record(
                    state,
                    &record,
                    Some(&json!({ "state": "starting" })),
                    Some(&actor.to_json()),
                );
                return Some(json_response(
                    201,
                    json!({ "success": true, "computer": pub_rec }),
                ));
            }
            Err(err) => {
                let rec_id = jsval::str_or(pending_record.get("id"), "");
                if !rec_id.is_empty() {
                    vmlib::update_virtual_machine_runtime(
                        &state.store,
                        &rec_id,
                        None,
                        Some("provisioning-failed"),
                    );
                }
                vm_audit(
                    state,
                    &actor.email,
                    Some(&pending_record),
                    "VM_CREATED",
                    false,
                    Some(&json!({ "code": err.code })),
                );
                let friendly = friendly_vm_error(&VmError::Service(err));
                return Some(json_response(
                    friendly.0,
                    json!({ "error": friendly.1, "code": friendly.2 }),
                ));
            }
        }
    }

    // POST /api/vm/my-computer/recreate (server.js:19422-19521)
    if path == "/api/vm/my-computer/recreate" && *method == Method::POST {
        let actor = match authenticated_vm_actor(state, headers) {
            Some(a) => a,
            None => {
                return Some(json_response(401, json!({ "error": "Sign in required." })));
            }
        };

        if !vm_same_origin_request(headers) {
            return Some(json_response(
                403,
                json!({ "error": "Request origin rejected." }),
            ));
        }

        if !is_eligible_for_free_vm(state, &actor.email, actor.is_admin) {
            return Some(json_response(
                403,
                json!({ "error": "Free computers are available to RJUHSD students (@student.rjuhsd.us) and Premium members." }),
            ));
        }

        let user_upgrades = get_user_vm_upgrades(state, &actor.email);
        let daily_max = user_upgrades
            .get("dailyMaxSeconds")
            .and_then(jsval::number)
            .unwrap_or(21600.0);
        let cpu_cores = user_upgrades
            .get("cpuCores")
            .and_then(jsval::number)
            .unwrap_or(2.0);
        let memory_mb = user_upgrades
            .get("memoryMb")
            .and_then(jsval::number)
            .unwrap_or(4096.0);
        let disk_gb = user_upgrades
            .get("diskGb")
            .and_then(jsval::number)
            .unwrap_or(32.0);

        if !actor.is_admin {
            let used_today = get_user_daily_vm_usage(state, &actor.email);
            if is_daily_vm_limit_reached(used_today, false, daily_max) {
                return Some(json_response(
                    429,
                    json!({
                        "error": format!(
                            "Daily limit reached: you have used your maximum {} hours of computer time for today. Your daily limit will reset tomorrow.",
                            (daily_max / 3600.0).round() as i64
                        ),
                        "code": "daily_limit_reached",
                        "dailyRemainingSeconds": 0,
                    }),
                ));
            }
            let cap_check = can_accommodate_vm_resources(state, cpu_cores, memory_mb).await;
            if !cap_check.get("ok").map(jsval::truthy).unwrap_or(false) {
                notify_capacity_full_on_attempt(
                    state,
                    &actor.email,
                    "recreate a computer",
                    &json!({
                        "cores": cap_check.get("wouldCores"),
                        "memoryMb": cap_check.get("wouldMemoryMb"),
                    }),
                );
                return Some(json_response(
                    409,
                    json!({
                        "error": format!("Server capacity reached: a maximum of {VM_MAX_CONCURRENT_RUNNING} computers can run at once. Please try again later."),
                        "code": "capacity_limit_reached",
                    }),
                ));
            }
        }

        let body: Value = if body_bytes.is_empty() {
            json!({})
        } else {
            match serde_json::from_slice(body_bytes) {
                Ok(v) => v,
                Err(_) => {
                    return Some(json_response(400, json!({ "error": "Invalid request." })));
                }
            }
        };

        let lock_key = format!("recreate-{}", actor.email);
        {
            let mut requests = state
                .vm_power_requests
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if requests.contains_key(&lock_key)
                || requests.contains_key(&format!("create-{}", actor.email))
            {
                return Some(json_response(
                    409,
                    json!({ "error": "A computer operation is already in progress for your account." }),
                ));
            }
            requests.insert(lock_key.clone(), json!({ "startedAt": now_ms() as f64 }));
        }

        let email_prefix = actor.email.split('@').next().unwrap_or("student");
        let mut base_user = email_prefix.to_lowercase();
        base_user.retain(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if !base_user.starts_with(|c: char| c.is_ascii_lowercase()) {
            base_user = format!("u{base_user}");
        }
        base_user.truncate(30);
        if matches!(base_user.as_str(), "root" | "daemon" | "nobody" | "ubuntu") {
            base_user = format!("u{base_user}");
        }

        let requested_username = jsval::str_or(body.get("desktopUsername"), &base_user)
            .trim()
            .to_lowercase();
        let raw_password = jsval::str_or(body.get("desktopPassword"), "");
        let desktop_login = match crate::proxmox_desktop::desktop()
            .validate_desktop_login(&requested_username, &raw_password)
        {
            Ok(dl) => dl,
            Err(_) => {
                state
                    .vm_power_requests
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&lock_key);
                return Some(json_response(
                    400,
                    json!({
                        "error": "Choose a password of 8 to 128 characters.",
                        "code": "invalid_desktop_login",
                    }),
                ));
            }
        };

        // 1. Delete existing computer(s) owned by user
        let existing_records = vmlib::get_virtual_machines_for_owner(&state.store, &actor.email);
        for old_rec in existing_records {
            let old = old_rec.to_json();
            let old_id = jsval::str_or(old.get("id"), "");
            if let Err(err) = crate::proxmox_desktop::desktop()
                .delete_guest(&old, true)
                .await
            {
                tracing::warn!("[recreate] Note: Proxmox delete for {old_id} returned: {err:?}");
            }
            vmlib::delete_virtual_machine(&state.store, &old_id);
            revoke_vm_desktop_connections(state, &old_id);
            clear_vm_lease(state, &old_id);
            state
                .vm_page_presence
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&old_id);
            vm_audit(
                state,
                &actor.email,
                Some(&old),
                "VM_DELETED_FOR_RECREATE",
                true,
                None,
            );
        }

        let mut vm_apps = vm_applications(state);
        if let Some(obj) = vm_apps.as_object_mut() {
            if obj.remove(&actor.email).is_some() {
                save_vm_json(state, "vm_applications.json", &vm_apps);
            }
        }
        clear_vm_cooldown(state, &actor.email);

        // 2. Clone new computer
        let template_vmid = crate::proxmox_desktop::desktop()
            .template_vmids()
            .first()
            .copied()
            .unwrap_or(9010) as f64;

        let all_records = vmlib::list_virtual_machines(&state.store, true);
        let existing_vmids: Vec<f64> = all_records.iter().map(|r| r.vmid).collect();

        let vmid = match crate::proxmox_desktop::desktop()
            .next_available_vmid(PVE_VMID_MIN, PVE_VMID_MAX, &existing_vmids)
            .await
        {
            Ok(v) => v as f64,
            Err(e) => {
                state
                    .vm_power_requests
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&lock_key);
                let friendly = friendly_vm_error(&VmError::Service(e));
                return Some(json_response(
                    friendly.0,
                    json!({ "error": friendly.1, "code": friendly.2 }),
                ));
            }
        };

        let hostname = format!("student-{}", vmid as i64);
        let pending_rec = vmlib::reserve_virtual_machine(
            &state.store,
            &json!({
                "id": format!("vm-{}", vmid as i64),
                "ownerEmail": actor.email,
                "ownerUserId": mitch_lib::profile::get_uid_for_email(&state.store, &state.id_secret, &actor.email).unwrap_or_default(),
                "vmid": vmid,
                "node": crate::proxmox_desktop::desktop().node(),
                "guestType": "qemu",
                "friendlyName": "My Computer",
                "hostname": hostname,
                "operatingSystem": "Linux Desktop",
                "templateVmid": template_vmid,
                "cpuCores": cpu_cores,
                "memoryMb": memory_mb,
                "diskGb": disk_gb,
                "status": "provisioning",
                "createdAt": now_ms() as f64,
            }),
        ).map(|r| r.to_json());

        let Some(pending_record) = pending_rec else {
            state
                .vm_power_requests
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&lock_key);
            return Some(json_response(
                500,
                json!({ "error": "Could not reserve computer slot. Please try again." }),
            ));
        };

        let clone_params = crate::proxmox_desktop::CloneDesktopParams {
            template_vmid: Some(template_vmid),
            vmid: Some(vmid),
            hostname: hostname.clone(),
            cpu_cores: Some(cpu_cores),
            memory_mb: Some(memory_mb),
            disk_gb: Some(disk_gb),
            desktop_username: desktop_login.0,
            desktop_password: desktop_login.1,
        };

        let clone_res = crate::proxmox_desktop::desktop()
            .clone_desktop(&clone_params)
            .await;
        state
            .vm_power_requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&lock_key);

        match clone_res {
            Ok(created) => {
                let mut merged = js_object_merge(&pending_record, &created);
                merged["status"] = json!("assigned");
                let record = vmlib::upsert_virtual_machine(&state.store, &merged)
                    .map(|r| r.to_json())
                    .unwrap_or(merged);
                let rec_id = jsval::str_or(record.get("id"), "");
                state
                    .vm_page_presence
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(rec_id, now_ms());
                vm_audit(
                    state,
                    &actor.email,
                    Some(&record),
                    "VM_RECREATED",
                    true,
                    None,
                );
                let pub_rec = public_vm_record(
                    state,
                    &record,
                    Some(&json!({ "state": "starting" })),
                    Some(&actor.to_json()),
                );
                return Some(json_response(
                    201,
                    json!({ "success": true, "computer": pub_rec }),
                ));
            }
            Err(err) => {
                let rec_id = jsval::str_or(pending_record.get("id"), "");
                if !rec_id.is_empty() {
                    vmlib::update_virtual_machine_runtime(
                        &state.store,
                        &rec_id,
                        None,
                        Some("provisioning-failed"),
                    );
                }
                vm_audit(
                    state,
                    &actor.email,
                    Some(&pending_record),
                    "VM_RECREATED",
                    false,
                    Some(&json!({ "code": err.code })),
                );
                let friendly = friendly_vm_error(&VmError::Service(err));
                return Some(json_response(
                    friendly.0,
                    json!({ "error": friendly.1, "code": friendly.2 }),
                ));
            }
        }
    }

    // vmComputerMatch: /api/vm/computers/:id(/:operation)? (server.js:19523-19808)
    static COMPUTER_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = COMPUTER_RE.get_or_init(|| {
        regex::Regex::new(r"^/api/vm/computers/([a-zA-Z0-9_-]{1,80})(?:/(power|desktop-session|extend|heartbeat|admin-access|request-admin-access))?$")
            .unwrap_or_else(|_| {
                regex::Regex::new("$^")
                    .unwrap_or_else(|_| regex::Regex::new("$^").unwrap_or_else(|_| unreachable!()))
            })
    });

    if let Some(caps) = re.captures(path) {
        let comp_id = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let operation = caps.get(2).map(|m| m.as_str()).unwrap_or("");

        let actor = match authenticated_vm_actor(state, headers) {
            Some(a) => a,
            None => {
                return Some(json_response(
                    401,
                    json!({ "error": "Sign in to access your computer." }),
                ));
            }
        };

        let record = match vmlib::get_virtual_machine_by_id(&state.store, comp_id) {
            Some(r) => r.to_json(),
            None => {
                return Some(json_response(
                    404,
                    json!({ "error": "Computer not found." }),
                ));
            }
        };

        let actor_json = actor.to_json();
        if !vm_record_allowed_for_actor(state, &record, &actor_json) {
            return Some(json_response(
                403,
                json!({ "error": "You do not have permission to access this computer." }),
            ));
        }

        if *method == Method::POST && !vm_same_origin_request(headers) {
            return Some(json_response(
                403,
                json!({ "error": "Request origin rejected." }),
            ));
        }

        let record_status = jsval::str_or(record.get("status"), "");
        if !operation.is_empty() && record_status == "provisioning" {
            return Some(json_response(
                409,
                json!({
                    "error": "Your computer is still being prepared.",
                    "code": "computer_starting",
                }),
            ));
        }
        if !operation.is_empty() && record_status == "provisioning-failed" {
            return Some(json_response(
                409,
                json!({
                    "error": "This computer needs administrator attention before it can be opened.",
                    "code": "setup_incomplete",
                }),
            ));
        }

        let record_owner = jsval::str_or(record.get("ownerEmail"), "");

        if operation == "admin-access" {
            if *method == Method::GET {
                let is_owner = mitch_lib::auth::normalize_email(&record_owner)
                    == mitch_lib::auth::normalize_email(&actor.email);
                if !is_owner && !actor.is_admin {
                    return Some(json_response(403, json!({ "error": "Permission denied." })));
                }
                return Some(json_response(
                    200,
                    json!({
                        "allowed": is_vm_admin_access_allowed(state, comp_id),
                        "requested": is_vm_admin_access_requested(state, comp_id),
                        "grant": get_vm_admin_grant(state, comp_id),
                    }),
                ));
            }
            if *method == Method::POST {
                let is_owner = mitch_lib::auth::normalize_email(&record_owner)
                    == mitch_lib::auth::normalize_email(&actor.email);
                if !is_owner {
                    return Some(json_response(
                        403,
                        json!({ "error": "Only the computer owner can grant or revoke admin access." }),
                    ));
                }
                let body: Value = if body_bytes.is_empty() {
                    json!({})
                } else {
                    match serde_json::from_slice(body_bytes) {
                        Ok(v) => v,
                        Err(_) => {
                            return Some(json_response(
                                400,
                                json!({ "error": "Invalid request." }),
                            ));
                        }
                    }
                };
                let allow = body
                    .get("allow")
                    .or_else(|| body.get("allowed"))
                    .map(jsval::truthy)
                    .unwrap_or(false);
                set_vm_admin_access(state, comp_id, &record_owner, allow, &actor.email);
                vm_audit(
                    state,
                    &actor.email,
                    Some(&record),
                    if allow {
                        "ADMIN_ACCESS_ALLOWED"
                    } else {
                        "ADMIN_ACCESS_REVOKED"
                    },
                    true,
                    Some(&json!({ "allowed": allow, "ownerEmail": record_owner })),
                );
                return Some(json_response(
                    200,
                    json!({
                        "success": true,
                        "allowed": allow,
                        "message": if allow { "Administrator access granted." } else { "Administrator access revoked." },
                    }),
                ));
            }
            return Some(json_response(
                405,
                json!({ "error": "Method not allowed." }),
            ));
        }

        if operation == "request-admin-access" && *method == Method::POST {
            if !actor.is_admin {
                return Some(json_response(
                    403,
                    json!({ "error": "Only administrators can request computer access." }),
                ));
            }
            if mitch_lib::auth::normalize_email(&record_owner)
                == mitch_lib::auth::normalize_email(&actor.email)
            {
                return Some(json_response(
                    200,
                    json!({ "success": true, "message": "You are the owner of this computer." }),
                ));
            }
            request_vm_admin_access(state, &record, &actor.email);
            return Some(json_response(
                200,
                json!({
                    "success": true,
                    "requested": true,
                    "message": "Access request sent to the computer owner.",
                }),
            ));
        }

        if operation.is_empty() && *method == Method::GET {
            match crate::proxmox_desktop::desktop().get_status(&record).await {
                Ok(runtime) => {
                    if let Some(ip) = runtime.get("ipAddress").and_then(Value::as_str) {
                        vmlib::update_virtual_machine_runtime(
                            &state.store,
                            comp_id,
                            Some(ip),
                            None,
                        );
                    }
                    let pub_rec =
                        public_vm_record(state, &record, Some(&runtime), Some(&actor_json));
                    return Some(json_response(200, json!({ "computer": pub_rec })));
                }
                Err(err) => {
                    let friendly = friendly_vm_error(&VmError::Service(err));
                    return Some(json_response(
                        friendly.0,
                        json!({ "error": friendly.1, "code": friendly.2 }),
                    ));
                }
            }
        }

        if operation == "heartbeat" && *method == Method::POST {
            let now = now_ms();
            state
                .vm_page_presence
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(comp_id.to_string(), now);
            return Some(json_response(
                200,
                json!({ "success": true, "lastSeen": now }),
            ));
        }

        if operation == "extend" && *method == Method::POST {
            if actor.is_admin || mitch_lib::auth::is_admin_email(&state.store, &record_owner) {
                return Some(json_response(
                    200,
                    json!({
                        "success": true,
                        "message": "Admins have unlimited session time.",
                        "lease": {
                            "isExempt": true,
                            "remainingSeconds": Value::Null,
                            "maxUptimeSeconds": Value::Null,
                            "canExtend": false,
                        },
                    }),
                ));
            }

            if !can_user_extend_today(state, &actor.email, actor.is_admin) {
                return Some(json_response(
                    400,
                    json!({
                        "error": "Only one 30-minute extension is allowed per day.",
                        "code": "daily_extension_limit_reached",
                    }),
                ));
            }

            let user_upgrades = get_user_vm_upgrades(state, &record_owner);
            let daily_max = user_upgrades
                .get("dailyMaxSeconds")
                .and_then(jsval::number)
                .unwrap_or(21600.0);
            let used_today = get_user_daily_vm_usage(state, &record_owner);
            if is_daily_vm_limit_reached(used_today, actor.is_admin, daily_max) {
                return Some(json_response(
                    400,
                    json!({
                        "error": "Daily computer limit reached.",
                        "code": "daily_limit_reached",
                    }),
                ));
            }

            match crate::proxmox_desktop::desktop().get_status(&record).await {
                Ok(runtime) => {
                    if jsval::str_or(runtime.get("state"), "") != "running" {
                        return Some(json_response(
                            400,
                            json!({ "error": "Computer must be running to extend session." }),
                        ));
                    }
                    let uptime = runtime.get("uptime").and_then(jsval::number).unwrap_or(0.0);
                    let lease_info =
                        get_vm_lease(state, comp_id, uptime, actor.is_admin, &record_owner);
                    let extended = lease_info
                        .get("extended")
                        .map(jsval::truthy)
                        .unwrap_or(false);
                    let can_extend = lease_info
                        .get("canExtend")
                        .map(jsval::truthy)
                        .unwrap_or(false);
                    if extended || !can_extend {
                        return Some(json_response(
                            400,
                            json!({
                                "error": "Maximum extension already applied (30 minutes maximum).",
                                "code": "extension_limit_reached",
                            }),
                        ));
                    }

                    {
                        let mut leases = state.vm_leases.lock().unwrap_or_else(|e| e.into_inner());
                        let mut lease = leases.get(comp_id).cloned().unwrap_or_else(
                            || json!({ "startedAt": (now_ms() as f64) - (uptime * 1000.0) }),
                        );
                        lease["extended"] = json!(true);
                        lease["extendedAt"] = json!(now_ms() as f64);
                        lease["lastSeenUptime"] = json!(uptime);
                        leases.insert(comp_id.to_string(), lease);
                    }

                    record_user_extension(state, &actor.email);
                    let updated_lease =
                        get_vm_lease(state, comp_id, uptime, actor.is_admin, &record_owner);
                    vm_audit(
                        state,
                        &actor.email,
                        Some(&record),
                        "VM_LEASE_EXTENDED",
                        true,
                        Some(&json!({
                            "remainingSeconds": updated_lease.get("remainingSeconds"),
                            "maxUptimeSeconds": updated_lease.get("maxUptimeSeconds"),
                        })),
                    );

                    return Some(json_response(
                        200,
                        json!({
                            "success": true,
                            "message": "Session extended by 30 minutes.",
                            "lease": updated_lease,
                        }),
                    ));
                }
                Err(err) => {
                    let friendly = friendly_vm_error(&VmError::Service(err));
                    return Some(json_response(
                        friendly.0,
                        json!({ "error": friendly.1, "code": friendly.2 }),
                    ));
                }
            }
        }

        if operation == "power" && *method == Method::POST {
            let body: Value = if body_bytes.is_empty() {
                json!({})
            } else {
                match serde_json::from_slice(body_bytes) {
                    Ok(v) => v,
                    Err(_) => {
                        return Some(json_response(400, json!({ "error": "Invalid request." })));
                    }
                }
            };

            let action = jsval::str_or(body.get("action"), "").trim().to_lowercase();
            if !["start", "shutdown", "restart", "force-stop"].contains(&action.as_str()) {
                return Some(json_response(
                    400,
                    json!({ "error": "Invalid power action." }),
                ));
            }

            let is_admin_using_other_vm = actor.is_admin
                && !record_owner.is_empty()
                && mitch_lib::auth::normalize_email(&record_owner)
                    != mitch_lib::auth::normalize_email(&actor.email);

            let has_admin_passphrase = headers
                .get("X-Admin-Passphrase")
                .and_then(|v| v.to_str().ok())
                .map(|p| p.trim())
                .filter(|p| !p.is_empty())
                .map(|p| {
                    mitch_lib::admin::verify_admin_passphrase_raw(
                        &state.store,
                        &state.id_secret,
                        &state.cfg.data_dir,
                        &actor.sid,
                        p,
                    )
                })
                .unwrap_or(false);

            let is_admin_power_override = actor.is_owner
                || has_admin_passphrase
                || action == "shutdown"
                || action == "force-stop"
                || action == "restart";

            if is_admin_using_other_vm && !is_admin_power_override && !is_vm_admin_access_allowed(state, comp_id) {
                request_vm_admin_access(state, &record, &actor.email);
                return Some(json_response(
                    403,
                    json!({
                        "error": "The owner has not allowed administrator access to this computer. An access request has been sent to their notification center.",
                        "code": "admin_access_not_allowed",
                        "accessRequested": true,
                    }),
                ));
            }

            if (action == "start" || action == "restart") && !actor.is_admin {
                let user_upgrades = get_user_vm_upgrades(state, &record_owner);
                let daily_max = user_upgrades
                    .get("dailyMaxSeconds")
                    .and_then(jsval::number)
                    .unwrap_or(21600.0);
                let used_today = get_user_daily_vm_usage(state, &record_owner);
                if is_daily_vm_limit_reached(used_today, actor.is_admin, daily_max) {
                    return Some(json_response(
                        429,
                        json!({
                            "error": "Daily limit reached: you have used your maximum 6 hours of computer time for today. Your daily limit will reset tomorrow.",
                            "code": "daily_limit_reached",
                            "dailyRemainingSeconds": 0,
                        }),
                    ));
                }
            }

            if (action == "start" || action == "restart") && !actor.is_admin {
                let cooldown_remaining =
                    get_vm_cooldown_remaining(state, &record_owner, actor.is_admin);
                if cooldown_remaining > 0.0 {
                    let mins = (cooldown_remaining / 60.0).ceil() as i64;
                    return Some(json_response(
                        429,
                        json!({
                            "error": format!(
                                "Computer is cooling down. You can start it again in {mins} minute{}.",
                                if mins == 1 { "" } else { "s" }
                            ),
                            "code": "vm_cooldown_active",
                            "cooldownRemainingSeconds": cooldown_remaining,
                        }),
                    ));
                }
            }

            if action == "start" || action == "restart" {
                let is_running = match crate::proxmox_desktop::desktop().get_status(&record).await {
                    Ok(rt) => jsval::str_or(rt.get("state"), "") == "running",
                    Err(_) => false,
                };
                if !is_running {
                    let req_cores = record
                        .get("cpuCores")
                        .and_then(jsval::number)
                        .unwrap_or(VM_DEFAULT_CPU_CORES);
                    let req_mem = record
                        .get("memoryMb")
                        .and_then(jsval::number)
                        .unwrap_or(VM_DEFAULT_MEMORY_MB);
                    let cap_check = can_accommodate_vm_resources(state, req_cores, req_mem).await;
                    if !cap_check.get("ok").map(jsval::truthy).unwrap_or(false) {
                        let vm_name = record
                            .get("friendlyName")
                            .or_else(|| record.get("hostname"))
                            .or_else(|| record.get("id"))
                            .and_then(Value::as_str)
                            .unwrap_or("My Computer");
                        notify_capacity_full_on_attempt(
                            state,
                            &actor.email,
                            &format!("start computer {vm_name}"),
                            &json!({
                                "cores": cap_check.get("wouldCores"),
                                "memoryMb": cap_check.get("wouldMemoryMb"),
                            }),
                        );
                        if !actor.is_admin {
                            return Some(json_response(
                                409,
                                json!({
                                    "error": format!("Server capacity reached: a maximum of {VM_MAX_CONCURRENT_RUNNING} computers can run at once. Please try again later."),
                                    "code": "capacity_limit_reached",
                                }),
                            ));
                        }
                    }
                }
            }

            let acquired = state
                .vm_power_gate
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .acquire(comp_id, &action, now_ms() as f64);
            if !acquired {
                return Some(json_response(
                    409,
                    json!({
                        "error": "A power request is already in progress.",
                        "code": "request_in_progress",
                    }),
                ));
            }

            if is_admin_using_other_vm {
                let vm_name = record
                    .get("friendlyName")
                    .or_else(|| record.get("hostname"))
                    .and_then(Value::as_str)
                    .unwrap_or("My Computer");
                println!(
                    "[vm-audit] Admin {} used VM {} ({}) owned by {}. Action: power-{}",
                    actor.email, comp_id, vm_name, record_owner, action
                );
                vm_audit(
                    state,
                    &actor.email,
                    Some(&record),
                    "ADMIN_VM_USED",
                    true,
                    Some(&json!({
                        "operation": format!("power-{action}"),
                        "targetUser": record_owner,
                        "vmid": record.get("vmid"),
                        "hostname": record.get("hostname"),
                    })),
                );
                notify_owner_admin_used_vm(
                    state,
                    &record,
                    &actor.email,
                    &format!("power action: {action}"),
                );
            }

            match crate::proxmox_desktop::desktop()
                .power(&record, &action)
                .await
            {
                Ok(task) => {
                    clear_vm_lease(state, comp_id);
                    if action == "start" {
                        state
                            .vm_page_presence
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(comp_id.to_string(), now_ms());
                        state
                            .vm_shutdown_initiated
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(comp_id);
                    }
                    if action == "shutdown" || action == "force-stop" {
                        revoke_vm_desktop_connections(state, comp_id);
                        trigger_vm_cooldown(state, &record_owner, "user_power_off");
                        state
                            .vm_page_presence
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(comp_id);
                    }
                    if action == "shutdown" {
                        state
                            .vm_shutdown_initiated
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .entry(comp_id.to_string())
                            .or_insert_with(|| now_ms() as i64);
                    } else if action == "force-stop" || action == "restart" {
                        state
                            .vm_shutdown_initiated
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(comp_id);
                    }

                    let node =
                        jsval::str_or(record.get("node"), crate::proxmox_desktop::desktop().node());
                    let state_arc = Arc::clone(state);
                    let rec_clone = record.clone();
                    let actor_email = actor.email.clone();
                    let action_clone = action.clone();
                    let comp_id_str = comp_id.to_string();

                    tokio::spawn(async move {
                        let wait_res = crate::proxmox_desktop::desktop()
                            .wait_for_task(&node, Some(&task), 180_000)
                            .await;
                        let action_tag = if action_clone == "restart" {
                            "VM_RESTARTED"
                        } else if action_clone == "start" {
                            "VM_STARTED"
                        } else {
                            "VM_STOPPED"
                        };
                        match wait_res {
                            Ok(_) => {
                                vm_audit(
                                    &state_arc,
                                    &actor_email,
                                    Some(&rec_clone),
                                    action_tag,
                                    true,
                                    None,
                                );
                            }
                            Err(e) => {
                                vm_audit(
                                    &state_arc,
                                    &actor_email,
                                    Some(&rec_clone),
                                    action_tag,
                                    false,
                                    Some(&json!({ "code": e.code })),
                                );
                            }
                        }
                        state_arc
                            .vm_power_gate
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .release(&comp_id_str, now_ms() as f64);
                    });

                    let state_desc = if action == "start" {
                        "starting"
                    } else if action == "restart" {
                        "restarting"
                    } else {
                        "shutting-down"
                    };
                    return Some(json_response(
                        202,
                        json!({ "success": true, "state": state_desc }),
                    ));
                }
                Err(err) => {
                    state
                        .vm_power_gate
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .release(comp_id, now_ms() as f64);
                    let action_tag = if action == "restart" {
                        "VM_RESTARTED"
                    } else if action == "start" {
                        "VM_STARTED"
                    } else {
                        "VM_STOPPED"
                    };
                    vm_audit(
                        state,
                        &actor.email,
                        Some(&record),
                        action_tag,
                        false,
                        Some(&json!({ "code": err.code })),
                    );
                    let friendly = friendly_vm_error(&VmError::Service(err));
                    return Some(json_response(
                        friendly.0,
                        json!({ "error": friendly.1, "code": friendly.2 }),
                    ));
                }
            }
        }

        if operation == "desktop-session" && *method == Method::POST {
            let is_admin_using_other_vm = actor.is_admin
                && !record_owner.is_empty()
                && mitch_lib::auth::normalize_email(&record_owner)
                    != mitch_lib::auth::normalize_email(&actor.email);

            if is_admin_using_other_vm && !actor.is_owner && !is_vm_admin_access_allowed(state, comp_id) {
                request_vm_admin_access(state, &record, &actor.email);
                return Some(json_response(
                    403,
                    json!({
                        "error": "The owner has not allowed administrator access to this computer. An access request has been sent to their notification center.",
                        "code": "admin_access_not_allowed",
                        "accessRequested": true,
                    }),
                ));
            }

            if is_admin_using_other_vm {
                let vm_name = record
                    .get("friendlyName")
                    .or_else(|| record.get("hostname"))
                    .and_then(Value::as_str)
                    .unwrap_or("My Computer");
                println!(
                    "[vm-audit] Admin {} used VM {} ({}) owned by {}. Action: desktop-session",
                    actor.email, comp_id, vm_name, record_owner
                );
                vm_audit(
                    state,
                    &actor.email,
                    Some(&record),
                    "ADMIN_VM_USED",
                    true,
                    Some(&json!({
                        "operation": "desktop-session",
                        "targetUser": record_owner,
                        "vmid": record.get("vmid"),
                        "hostname": record.get("hostname"),
                    })),
                );
                notify_owner_admin_used_vm(state, &record, &actor.email, "opened desktop session");
            }

            cleanup_vm_desktop_sessions(state);

            match crate::proxmox_desktop::desktop()
                .create_console(&record)
                .await
            {
                Ok(console_session) => {
                    let rand_bytes = mitch_lib::crypto::random_bytes(24);
                    let session_id =
                        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rand_bytes);
                    let node_str =
                        jsval::str_or(record.get("node"), crate::proxmox_desktop::desktop().node());

                    {
                        let mut sessions = state
                            .vm_desktop_sessions
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        sessions.insert(
                            session_id.clone(),
                            json!({
                                "actorEmail": actor.email,
                                "sid": actor.sid,
                                "authSessionKey": actor.auth_session_key,
                                "recordId": comp_id,
                                "ownerEmail": record_owner,
                                "vmid": record.get("vmid"),
                                "node": node_str,
                                "wsUrl": console_session.get("wsUrl"),
                                "authorization": console_session.get("authorization"),
                                "tlsOptions": console_session.get("tlsOptions"),
                                "expiresAt": (now_ms() as f64) + VM_DESKTOP_SESSION_TTL_MS,
                                "used": false,
                            }),
                        );
                    }

                    vm_audit(
                        state,
                        &actor.email,
                        Some(&record),
                        "DESKTOP_OPENED",
                        true,
                        None,
                    );

                    let enc_session = mitch_lib::auth::encode_uri_component(&session_id);
                    let ticket = jsval::str_or(console_session.get("ticket"), "");
                    return Some(json_response(
                        201,
                        json!({
                            "socketPath": format!("/api/vm/desktop/ws?session={enc_session}"),
                            "credentials": { "password": ticket },
                            "expiresIn": (VM_DESKTOP_SESSION_TTL_MS / 1000.0).floor() as i64,
                        }),
                    ));
                }
                Err(err) => {
                    vm_audit(
                        state,
                        &actor.email,
                        Some(&record),
                        "DESKTOP_OPENED",
                        false,
                        Some(&json!({ "code": err.code })),
                    );
                    let friendly = friendly_vm_error(&VmError::Service(err));
                    return Some(json_response(
                        friendly.0,
                        json!({ "error": friendly.1, "code": friendly.2 }),
                    ));
                }
            }
        }

        return Some(json_response(
            405,
            json!({ "error": "Method not allowed." }),
        ));
    }

    if path.starts_with("/api/vm/computers/") {
        return Some(json_response(
            400,
            json!({ "error": "Invalid computer ID.", "code": "invalid_request" }),
        ));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vmid_range() {
        assert!(is_vm_id_in_range(200.0));
        assert!(is_vm_id_in_range(999.0));
        assert!(is_vm_id_in_range(350.0));
        assert!(!is_vm_id_in_range(199.0));
        assert!(!is_vm_id_in_range(1000.0));
        assert!(!is_vm_id_in_range(f64::NAN));
        assert!(!is_vm_id_in_range(1.0 / 0.0));
    }

    #[test]
    fn vm_type_fallback_range() {
        // Outside any registry (empty state), the JS falls to the range rule:
        // 200-399 → lxc, else qemu. get_vm_type_by_vmid with no state access
        // to free-VM apps resolves through vm_applications.json ({} here).
        let dir = std::env::temp_dir().join(format!(
            "mitch-server-vm-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap_or_default();
        std::fs::write(dir.join("vm_applications.json"), "{}").unwrap_or_default();
        let cfg = crate::hosts::SiteConfig::load();
        let cfg = crate::hosts::SiteConfig {
            data_dir: dir.to_path_buf(),
            ..cfg
        };
        let store = Arc::new(
            mitch_lib::data::DataStore::open(&dir, &dir).unwrap_or_else(|e| {
                panic!("store: {e}");
            }),
        );
        let state = AppState::new(cfg, Arc::clone(&store));
        assert_eq!(get_vm_type_by_vmid(&state, 250.0), "lxc");
        assert_eq!(get_vm_type_by_vmid(&state, 399.0), "lxc");
        assert_eq!(get_vm_type_by_vmid(&state, 400.0), "qemu");
        assert_eq!(get_vm_type_by_vmid(&state, 900.0), "qemu");
        // activeFreeVms wins first.
        state
            .active_free_vms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert("a@b".to_string(), json!({ "vmid": 900.0 }));
        assert_eq!(get_vm_type_by_vmid(&state, 900.0), "lxc");
    }

    #[test]
    fn ten_ip_regex() {
        assert!(js_is_10_ip("10.0.0.5"));
        assert!(js_is_10_ip("10.0.0.123"));
        assert!(!js_is_10_ip("10.0.0.1234"));
        assert!(!js_is_10_ip("10.0.0."));
        assert!(!js_is_10_ip("10.0.0.x"));
        assert!(!js_is_10_ip("192.168.0.5"));
        assert!(!js_is_10_ip(""));
    }

    #[test]
    fn test_vm_same_origin_request() {
        let mut headers = HeaderMap::new();
        headers.insert(axum::http::header::HOST, "mitchdog.com".parse().unwrap());
        headers.insert(
            axum::http::header::ORIGIN,
            "https://mitchdog.com".parse().unwrap(),
        );
        assert!(vm_same_origin_request(&headers));

        // Behind reverse proxy with port 443 in Host header
        let mut headers_port = HeaderMap::new();
        headers_port.insert(axum::http::header::HOST, "mitchdog.com:443".parse().unwrap());
        headers_port.insert(
            axum::http::header::ORIGIN,
            "https://mitchdog.com".parse().unwrap(),
        );
        assert!(vm_same_origin_request(&headers_port));

        // Internal upstream Host header with trusted origin
        let mut headers_upstream = HeaderMap::new();
        headers_upstream.insert(
            axum::http::header::HOST,
            "webserver-blue:6800".parse().unwrap(),
        );
        headers_upstream.insert(
            axum::http::header::ORIGIN,
            "https://mitchdog.com".parse().unwrap(),
        );
        assert!(vm_same_origin_request(&headers_upstream));

        // Referer fallback
        let mut headers_ref = HeaderMap::new();
        headers_ref.insert(axum::http::header::HOST, "mitchdog.com".parse().unwrap());
        headers_ref.insert(
            axum::http::header::REFERER,
            "https://mitchdog.com/admin/vms/".parse().unwrap(),
        );
        assert!(vm_same_origin_request(&headers_ref));

        // X-Mitch-Requested-With fallback
        let mut headers_xhr = HeaderMap::new();
        headers_xhr.insert(axum::http::header::HOST, "mitchdog.com".parse().unwrap());
        headers_xhr.insert("x-mitch-requested-with", "1".parse().unwrap());
        assert!(vm_same_origin_request(&headers_xhr));

        // Untrusted origin rejected
        let mut headers_bad = HeaderMap::new();
        headers_bad.insert(axum::http::header::HOST, "mitchdog.com".parse().unwrap());
        headers_bad.insert(
            axum::http::header::ORIGIN,
            "https://evil.attacker.com".parse().unwrap(),
        );
        assert!(!vm_same_origin_request(&headers_bad));
    }
}
