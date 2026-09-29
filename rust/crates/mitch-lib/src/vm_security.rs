//! Pure VM security/policy helpers — port of lib/vm_security.js (205 lines).
//!
//! Compatibility contract: every constant, gate and formatter matches the JS
//! exactly (same defaults, same clamp windows, same message strings) — these
//! feed both the route bodies and the response shapes the frontend renders.

use crate::jsval;
use serde_json::{json, Map, Value};
use std::collections::HashMap;

/// The `options.isGrantAllowed(record.id, record)` closure shape of
/// `canAccessVmRecord` (vm_security.js:15-17).
pub type VmGrantCheck<'a> = &'a (dyn Fn(&str, &Value) -> bool + 'a);

/// `canAccessVmRecord(record, actor, options)` (vm_security.js:1-22). `record`
/// /`actor` are the camelCase JSON objects; `options` mirrors the JS call
/// shapes the routes use — `{ isAdminEmail: fn|Set|Array }` and
/// `{ requireAdminGrant: true, isGrantAllowed: fn }` (plus the plain
/// `isAdminEmail` function form).
pub fn can_access_vm_record(
    record: Option<&Value>,
    actor: Option<&Value>,
    is_admin_email: impl Fn(&str) -> bool,
    require_admin_grant: bool,
    is_grant_allowed: Option<VmGrantCheck<'_>>,
) -> bool {
    let (Some(record), Some(actor)) = (record, actor) else {
        return false;
    };
    let owner = jsval::string(&jsval::or(record.get("ownerEmail"), json!("")))
        .trim()
        .to_lowercase();
    let email = jsval::string(&jsval::or(actor.get("email"), json!("")))
        .trim()
        .to_lowercase();
    let status = jsval::string(&jsval::or(record.get("status"), json!("")));
    let is_owner =
        status != "unassigned" && !owner.is_empty() && !email.is_empty() && owner == email;
    if is_owner {
        return true;
    }
    let admin = actor.get("isAdmin").map(jsval::truthy).unwrap_or(false);
    if admin {
        let owner_is_admin = !owner.is_empty() && is_admin_email(&owner);
        if owner_is_admin {
            return false;
        }
        if require_admin_grant {
            return match is_grant_allowed {
                Some(f) => f(
                    &jsval::string(&jsval::or(record.get("id"), json!(""))),
                    record,
                ),
                None => record
                    .get("adminAccessAllowed")
                    .map(jsval::truthy)
                    .unwrap_or(false),
            };
        }
        return true;
    }
    false
}

/// `validateDesktopSession(session, actor, record, now, options)`
/// (vm_security.js:24-32) — the full ladder: expired → identity → record →
/// owner → authSessionKey → access. Returns the JS `{ok, status, code}`.
pub fn validate_desktop_session(
    session: Option<&Value>,
    actor: Option<&Value>,
    record: Option<&Value>,
    now: f64,
    is_admin_email: impl Fn(&str) -> bool,
    require_admin_grant: bool,
    is_grant_allowed: Option<VmGrantCheck<'_>>,
) -> (bool, u16, &'static str) {
    let Some(session) = session else {
        return (false, 401, "expired");
    };
    if session.get("used").map(jsval::truthy).unwrap_or(false) {
        return (false, 401, "expired");
    }
    let expires_at = session
        .get("expiresAt")
        .and_then(jsval::number)
        .unwrap_or(0.0);
    if expires_at <= now {
        return (false, 401, "expired");
    }
    let (Some(actor), Some(record)) = (actor, record) else {
        return (false, 403, "forbidden");
    };
    let sid_matches = jsval::string(&jsval::or(session.get("sid"), json!("")))
        == jsval::string(&jsval::or(actor.get("sid"), json!("")));
    let email_matches = jsval::string(&jsval::or(session.get("actorEmail"), json!("")))
        == jsval::string(&jsval::or(actor.get("email"), json!("")));
    if !sid_matches || !email_matches {
        return (false, 403, "forbidden");
    }
    let record_id = jsval::string(&jsval::or(record.get("id"), json!("")));
    let session_record_id = jsval::string(&jsval::or(session.get("recordId"), json!("")));
    let record_vmid = record.get("vmid").and_then(jsval::number);
    let session_vmid = session.get("vmid").and_then(jsval::number);
    // JS `Number(a) !== Number(b)`: absent keys give NaN, and NaN !== NaN is
    // true → forbidden. Only two present equal numbers pass.
    let vmid_matches = match (record_vmid, session_vmid) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    };
    if record_id != session_record_id || !vmid_matches {
        return (false, 403, "forbidden");
    }
    if jsval::string(&jsval::or(record.get("node"), json!("")))
        != jsval::string(&jsval::or(session.get("node"), json!("")))
    {
        return (false, 403, "forbidden");
    }
    // `session.ownerEmail !== undefined && session.ownerEmail !== record.ownerEmail`
    // — a present key of any JSON value (null included) enters the compare.
    if let Some(owner) = session.get("ownerEmail") {
        if *owner != record.get("ownerEmail").cloned().unwrap_or(Value::Null) {
            return (false, 403, "forbidden");
        }
    }
    let session_key = jsval::string(&jsval::or(session.get("authSessionKey"), json!("")));
    if !session_key.is_empty()
        && session_key != jsval::string(&jsval::or(actor.get("authSessionKey"), json!("")))
    {
        return (false, 403, "forbidden");
    }
    if !can_access_vm_record(
        Some(record),
        Some(actor),
        is_admin_email,
        require_admin_grant,
        is_grant_allowed,
    ) {
        return (false, 403, "forbidden");
    }
    (true, 101, "ok")
}

/// `VmOperationGate` (vm_security.js:34-55) — per-key operation mutex with a
/// post-completion cooldown. Same map semantics as the JS class.
#[derive(Debug)]
pub struct VmOperationGate {
    cooldown_ms: u64,
    active: HashMap<String, GateEntry>,
}

#[derive(Debug, Clone)]
struct GateEntry {
    pending: bool,
    finished_at: f64,
}

impl VmOperationGate {
    pub fn new(cooldown_ms: u64) -> Self {
        Self {
            cooldown_ms,
            active: HashMap::new(),
        }
    }

    /// `acquire(key, action, now)` — false when the key is pending or inside
    /// its cooldown window; otherwise records the start and returns true.
    pub fn acquire(&mut self, key: &str, action: &str, now: f64) -> bool {
        if let Some(current) = self.active.get(key) {
            if current.pending || now - current.finished_at < self.cooldown_ms as f64 {
                return false;
            }
        }
        let _ = action; // JS records action/startedAt on the entry; only
                        // pending/finishedAt drive the gate's behavior.
        self.active.insert(
            key.to_string(),
            GateEntry {
                pending: true,
                finished_at: 0.0,
            },
        );
        true
    }

    /// `release(key, now)`.
    pub fn release(&mut self, key: &str, now: f64) {
        if let Some(entry) = self.active.get_mut(key) {
            entry.pending = false;
            entry.finished_at = now;
        }
    }

    /// `cleanup(now)` — drop finished entries past the cooldown.
    pub fn cleanup(&mut self, now: f64) {
        self.active
            .retain(|_, state| state.pending || now - state.finished_at < self.cooldown_ms as f64);
    }
}

/// `VM_DEFAULT_CPU_CORES` (vm_security.js:57).
pub const VM_DEFAULT_CPU_CORES: f64 = 2.0;
/// `VM_DEFAULT_MEMORY_MB`.
pub const VM_DEFAULT_MEMORY_MB: f64 = 4096.0;
/// `VM_DEFAULT_BALLOON_MB`.
pub const VM_DEFAULT_BALLOON_MB: f64 = 1024.0;
/// `VM_DEFAULT_DISK_GB`.
pub const VM_DEFAULT_DISK_GB: f64 = 64.0;

/// `VM_MAX_UPGRADE_CPU_CORES`.
pub const VM_MAX_UPGRADE_CPU_CORES: f64 = 6.0;
/// `VM_MAX_UPGRADE_MEMORY_MB`.
pub const VM_MAX_UPGRADE_MEMORY_MB: f64 = 16384.0;
/// `VM_MAX_UPGRADE_DISK_GB`.
pub const VM_MAX_UPGRADE_DISK_GB: f64 = 128.0;

/// `VM_FLEET_MAX_CORES`.
pub const VM_FLEET_MAX_CORES: f64 = 36.0;
/// `VM_FLEET_MAX_MEMORY_MB` = 6 * 16 * 1024.
pub const VM_FLEET_MAX_MEMORY_MB: f64 = 98_304.0;
/// `VM_MAX_CONCURRENT_RUNNING` = 36 / 2.
pub const VM_MAX_CONCURRENT_RUNNING: f64 = 18.0;

/// `VM_DAILY_MAX_SECONDS` = 6 hours.
pub const VM_DAILY_MAX_SECONDS: f64 = 21_600.0;
/// `VM_EXTENSION_COOLDOWN_MS` = 24 hours.
pub const VM_EXTENSION_COOLDOWN_MS: f64 = 86_400_000.0;
/// `VM_COOLDOWN_DURATION_MS` = 30 minutes.
pub const VM_COOLDOWN_DURATION_MS: f64 = 1_800_000.0;
/// `VM_OFFPAGE_INACTIVITY_MS` = 10 minutes.
pub const VM_OFFPAGE_INACTIVITY_MS: f64 = 600_000.0;
/// `VM_ADMIN_OFFPAGE_INACTIVITY_MS` = 30 minutes.
pub const VM_ADMIN_OFFPAGE_INACTIVITY_MS: f64 = 1_800_000.0;

/// `VM_UPGRADE_CATALOG` (vm_security.js:75-101) — the exact tiers, labels,
/// costs and durations the /api/vm/upgrade + /upgrades endpoints serve.
pub fn vm_upgrade_catalog() -> Value {
    let tier = |value: f64, label: &str, cost: f64, extra: &[(&str, Value)]| -> Value {
        let mut obj = Map::new();
        obj.insert("value".into(), json!(value));
        obj.insert("label".into(), json!(label));
        obj.insert("cost".into(), json!(cost));
        for (k, v) in extra {
            obj.insert((*k).into(), v.clone());
        }
        Value::Object(obj)
    };
    json!({
        "cpu": [
            tier(2.0, "2 Cores (Default)", 0.0, &[]),
            tier(4.0, "4 Cores", 400.0, &[]),
            tier(6.0, "6 Cores (Max)", 800.0, &[]),
        ],
        "ram": [
            tier(4096.0, "4 GB (Default)", 0.0, &[]),
            tier(8192.0, "8 GB", 400.0, &[]),
            tier(12288.0, "12 GB", 800.0, &[]),
            tier(16384.0, "16 GB (Max)", 1200.0, &[]),
        ],
        "disk": [
            tier(64.0, "64 GB (Default)", 0.0, &[]),
            tier(80.0, "80 GB", 150.0, &[]),
            tier(96.0, "96 GB", 300.0, &[]),
            tier(112.0, "112 GB", 600.0, &[]),
            tier(128.0, "128 GB (Max)", 1200.0, &[]),
        ],
        "session": [
            tier(21600.0, "6 Hours / Day (Default)", 0.0, &[("duration", json!("Permanent"))]),
            tier(28800.0, "8 Hours / Day (+2h) - 30-Day Pass", 300.0, &[("durationDays", json!(30.0))]),
            tier(36000.0, "10 Hours / Day (+4h) - 30-Day Pass", 600.0, &[("durationDays", json!(30.0))]),
            tier(43200.0, "12 Hours / Day (+6h) - 30-Day Pass", 900.0, &[("durationDays", json!(30.0))]),
            tier(86400.0, "Unlimited (24h / Day) - 30-Day Pass", 1800.0, &[("durationDays", json!(30.0))]),
        ],
    })
}

/// `checkFleetResourceCapacity` (vm_security.js:103-114).
pub fn check_fleet_resource_capacity(
    running_cores: f64,
    running_memory_mb: f64,
    adding_cores: f64,
    adding_memory_mb: f64,
) -> Value {
    let total_cores = running_cores + adding_cores;
    let total_memory_mb = running_memory_mb + adding_memory_mb;
    json!({
        "ok": total_cores <= VM_FLEET_MAX_CORES && total_memory_mb <= VM_FLEET_MAX_MEMORY_MB,
        "totalCores": total_cores,
        "totalMemoryMb": total_memory_mb,
        "maxCores": VM_FLEET_MAX_CORES,
        "maxMemoryMb": VM_FLEET_MAX_MEMORY_MB,
    })
}

/// `getRemainingDailyVmSeconds` (vm_security.js:116-120) — admins and
/// unlimited-session users get Infinity (modeled as `None`).
pub fn get_remaining_daily_vm_seconds(
    used_seconds: f64,
    is_admin: bool,
    daily_max_seconds: f64,
) -> Option<f64> {
    if is_admin || daily_max_seconds >= 86_400.0 {
        return None;
    }
    let used = used_seconds.max(0.0).floor();
    Some((daily_max_seconds - used).max(0.0))
}

/// `isDailyVmLimitReached` (vm_security.js:122-125).
pub fn is_daily_vm_limit_reached(
    used_seconds: f64,
    is_admin: bool,
    daily_max_seconds: f64,
) -> bool {
    if is_admin || daily_max_seconds >= 86_400.0 {
        return false;
    }
    used_seconds >= daily_max_seconds
}

/// `getVmDayKey(now)` (vm_security.js:127-129).
pub fn get_vm_day_key(now: i64) -> String {
    crate::coins::js_iso_date_from(now)
}

/// `isEligibleForFreeVm(email, {isAdmin, isPremium})` (vm_security.js:131-138).
pub fn is_eligible_for_free_vm(email: &str, is_admin: bool, is_premium: bool) -> bool {
    if email.is_empty() {
        return false;
    }
    if is_admin {
        return true;
    }
    let e = email.trim().to_lowercase();
    if e.ends_with("@student.rjuhsd.us") || e.ends_with("@student.mitch.pro") {
        return true;
    }
    is_premium
}

/// `canUserExtend(lastExtensionAt, {isAdmin, now, cooldownMs})`
/// (vm_security.js:140-144).
pub fn can_user_extend(last_extension_at: f64, is_admin: bool, now: f64) -> bool {
    if is_admin {
        return true;
    }
    if last_extension_at == 0.0 {
        return true;
    }
    now - last_extension_at >= VM_EXTENSION_COOLDOWN_MS
}

/// `computeCooldownRemaining(cooldownUntil, {isAdmin, now})`
/// (vm_security.js:146-151).
pub fn compute_cooldown_remaining(cooldown_until: f64, is_admin: bool, now: f64) -> f64 {
    if is_admin {
        return 0.0;
    }
    if cooldown_until == 0.0 {
        return 0.0;
    }
    let rem_ms = cooldown_until - now;
    if rem_ms > 0.0 {
        (rem_ms / 1000.0).ceil()
    } else {
        0.0
    }
}

/// `isVmInactive(lastSeen, {now, timeoutMs, isAdmin})` (vm_security.js:153-156).
pub fn is_vm_inactive(last_seen: f64, now: f64, is_admin: bool) -> bool {
    if last_seen == 0.0 {
        return false;
    }
    let timeout = if is_admin {
        VM_ADMIN_OFFPAGE_INACTIVITY_MS
    } else {
        VM_OFFPAGE_INACTIVITY_MS
    };
    now - last_seen >= timeout
}

/// `isVmAdminAccessAllowed(recordId, grants)` (vm_security.js:158-161).
pub fn is_vm_admin_access_allowed(record_id: &str, grants: &Value) -> bool {
    if record_id.is_empty() {
        return false;
    }
    grants
        .get(record_id)
        .and_then(|g| g.get("allowed"))
        .map(jsval::truthy)
        .unwrap_or(false)
}

/// `isVmAdminAccessRequested(recordId, grants)` (vm_security.js:163-166).
pub fn is_vm_admin_access_requested(record_id: &str, grants: &Value) -> bool {
    if record_id.is_empty() {
        return false;
    }
    grants
        .get(record_id)
        .and_then(|g| g.get("requested"))
        .map(jsval::truthy)
        .unwrap_or(false)
}

/// `shouldNotifyCapacityAlert(email, now, lastMap, cooldownMs)`
/// (vm_security.js:168-174) — mutates the shared last-notify map.
pub fn should_notify_capacity_alert(
    email: &str,
    now: f64,
    last_map: &mut HashMap<String, f64>,
    cooldown_ms: f64,
) -> bool {
    let norm = if email.is_empty() {
        "unknown".to_string()
    } else {
        email.trim().to_lowercase()
    };
    let last = last_map.get(&norm).copied().unwrap_or(0.0);
    if now - last < cooldown_ms {
        return false;
    }
    last_map.insert(norm, now);
    true
}

/// `formatCapacityFullAlert(userEmail, actionDesc, maxLimit)`
/// (vm_security.js:176-183).
pub fn format_capacity_full_alert(
    user_email: &str,
    action_desc: &str,
    max_limit: f64,
) -> (String, String, &'static str) {
    let norm = if user_email.is_empty() {
        "unknown".to_string()
    } else {
        user_email.trim().to_lowercase()
    };
    (
        "VM Capacity Alert".to_string(),
        format!(
            "VM capacity full ({max}/{max}): {norm} attempted to {action_desc}.",
            max = js_number_string(max_limit)
        ),
        "high",
    )
}

/// `formatAdminUsageNotice(adminEmail, vmName, operation)`
/// (vm_security.js:185-190).
pub fn format_admin_usage_notice(
    admin_email: &str,
    vm_name: &str,
    operation: &str,
) -> (String, String) {
    (
        "Admin Used Your Computer".to_string(),
        format!("Administrator {admin_email} accessed your computer \"{vm_name}\" ({operation})."),
    )
}

/// `formatAdminAccessRequest(adminEmail, vmName)` (vm_security.js:192-197).
pub fn format_admin_access_request(admin_email: &str, vm_name: &str) -> (String, String) {
    (
        "Admin Access Request".to_string(),
        format!(
            "Administrator {admin_email} requested access to your computer \"{vm_name}\". You can allow or revoke access in your Computer settings."
        ),
    )
}

/// `formatUptimeDuration(seconds)` (vm_security.js:199-205) — `"Xh Ym"` /
/// `"Ym"`.
pub fn format_uptime_duration(seconds: f64) -> String {
    let s = seconds.max(0.0).floor();
    let h = (s / 3600.0).floor();
    let m = ((s % 3600.0) / 60.0).floor();
    if h > 0.0 {
        format!("{}h {}m", h as i64, m as i64)
    } else {
        format!("{}m", m as i64)
    }
}

/// JS `String(Number)` rendering for the capacity alert's max limit.
fn js_number_string(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        v.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(owner: &str, status: &str) -> Value {
        json!({ "id": "vm-1", "ownerEmail": owner, "status": status, "vmid": 200.0, "node": "tartarus" })
    }

    fn actor(email: &str, admin: bool) -> Value {
        json!({ "email": email, "isAdmin": admin, "sid": "s1" })
    }

    const NOT_ADMIN: fn(&str) -> bool = |_| false;
    const IS_ADMIN: fn(&str) -> bool = |e: &str| e == "admin@mitch.pro";

    #[test]
    fn access_ladder() {
        // Owner match wins even when the owner is an admin.
        assert!(can_access_vm_record(
            Some(&record("a@b.pro", "assigned")),
            Some(&actor("a@b.pro", true)),
            IS_ADMIN,
            false,
            None,
        ));
        // Unassigned records never match by email.
        assert!(!can_access_vm_record(
            Some(&record("a@b.pro", "unassigned")),
            Some(&actor("a@b.pro", false)),
            NOT_ADMIN,
            false,
            None,
        ));
        // Non-owner non-admin denied.
        assert!(!can_access_vm_record(
            Some(&record("a@b.pro", "assigned")),
            Some(&actor("c@d.pro", false)),
            NOT_ADMIN,
            false,
            None,
        ));
        // Admin allowed on a normal user's record.
        assert!(can_access_vm_record(
            Some(&record("a@b.pro", "assigned")),
            Some(&actor("admin@mitch.pro", true)),
            IS_ADMIN,
            false,
            None,
        ));
        // Admin denied on an admin-owned record.
        assert!(!can_access_vm_record(
            Some(&record("admin@mitch.pro", "assigned")),
            Some(&actor("admin2@mitch.pro", true)),
            IS_ADMIN,
            false,
            None,
        ));
        // requireAdminGrant: closure form.
        let grant = |_id: &str, _r: &Value| true;
        assert!(can_access_vm_record(
            Some(&record("a@b.pro", "assigned")),
            Some(&actor("admin@mitch.pro", true)),
            IS_ADMIN,
            true,
            Some(&grant),
        ));
        // requireAdminGrant: falls to record.adminAccessAllowed.
        let mut rec = record("a@b.pro", "assigned");
        rec["adminAccessAllowed"] = json!(true);
        assert!(can_access_vm_record(
            Some(&rec),
            Some(&actor("admin@mitch.pro", true)),
            IS_ADMIN,
            true,
            None,
        ));
    }

    #[test]
    fn session_ladder() {
        let session = json!({
            "sid": "s1", "actorEmail": "a@b.pro", "recordId": "vm-1",
            "vmid": 200.0, "node": "tartarus", "ownerEmail": "a@b.pro",
            "expiresAt": 5000.0, "used": false,
        });
        let actor_v = actor("a@b.pro", false);
        let rec = record("a@b.pro", "assigned");
        let (ok, status, code) = validate_desktop_session(
            Some(&session),
            Some(&actor_v),
            Some(&rec),
            1000.0,
            NOT_ADMIN,
            false,
            None,
        );
        assert!(ok && status == 101 && code == "ok");

        // Expired (and used) → 401 'expired'.
        let (ok, status, code) = validate_desktop_session(
            Some(&session),
            Some(&actor_v),
            Some(&rec),
            5000.0,
            NOT_ADMIN,
            false,
            None,
        );
        assert!(!ok && status == 401 && code == "expired");
        let mut used = session.clone();
        used["used"] = json!(true);
        let (ok, status, _) = validate_desktop_session(
            Some(&used),
            Some(&actor_v),
            Some(&rec),
            1000.0,
            NOT_ADMIN,
            false,
            None,
        );
        assert!(!ok && status == 401);

        // sid/email mismatch → 403.
        let (ok, status, code) = validate_desktop_session(
            Some(&session),
            Some(&actor("other@b.pro", false)),
            Some(&rec),
            1000.0,
            NOT_ADMIN,
            false,
            None,
        );
        assert!(!ok && status == 403 && code == "forbidden");

        // record/vmid/node mismatch → 403.
        let other_record = json!({ "id": "vm-2", "ownerEmail": "a@b.pro", "status": "assigned", "vmid": 201.0, "node": "tartarus" });
        let (ok, status, _) = validate_desktop_session(
            Some(&session),
            Some(&actor_v),
            Some(&other_record),
            1000.0,
            NOT_ADMIN,
            false,
            None,
        );
        assert!(!ok && status == 403);

        // ownerEmail present + differing → 403.
        let mut session2 = session.clone();
        session2["ownerEmail"] = json!("zz@b.pro");
        let (ok, status, _) = validate_desktop_session(
            Some(&session2),
            Some(&actor_v),
            Some(&rec),
            1000.0,
            NOT_ADMIN,
            false,
            None,
        );
        assert!(!ok && status == 403);

        // authSessionKey mismatch → 403.
        let mut session3 = session.clone();
        session3["authSessionKey"] = json!("k1");
        let mut actor3 = actor_v.clone();
        actor3["authSessionKey"] = json!("k2");
        let (ok, status, _) = validate_desktop_session(
            Some(&session3),
            Some(&actor3),
            Some(&rec),
            1000.0,
            NOT_ADMIN,
            false,
            None,
        );
        assert!(!ok && status == 403);
        // Matching key passes.
        actor3["authSessionKey"] = json!("k1");
        let (ok, _, _) = validate_desktop_session(
            Some(&session3),
            Some(&actor3),
            Some(&rec),
            1000.0,
            NOT_ADMIN,
            false,
            None,
        );
        assert!(ok);
    }

    #[test]
    fn gate_semantics() {
        let mut gate = VmOperationGate::new(5000);
        assert!(gate.acquire("vm-1", "start", 1000.0));
        assert!(
            !gate.acquire("vm-1", "start", 2000.0),
            "pending blocks re-acquire"
        );
        gate.release("vm-1", 3000.0);
        assert!(
            !gate.acquire("vm-1", "start", 7000.0),
            "cooldown 5000 not elapsed"
        );
        assert!(
            gate.acquire("vm-1", "start", 8000.0),
            "elapsed at 5000ms after release"
        );
        gate.release("vm-1", 9000.0);
        gate.cleanup(9_000.0);
        assert!(
            !gate.acquire("vm-1", "start", 9_100.0),
            "release+cooldown still held"
        );
        gate.cleanup(15_000.0);
        assert!(
            gate.acquire("vm-1", "start", 15_000.0),
            "cleanup dropped the entry"
        );
    }

    #[test]
    fn catalog_shape() {
        let catalog = vm_upgrade_catalog();
        assert_eq!(catalog["cpu"].as_array().unwrap().len(), 3);
        assert_eq!(catalog["cpu"][1]["label"], json!("4 Cores"));
        assert_eq!(catalog["cpu"][1]["cost"], json!(400.0));
        assert_eq!(catalog["ram"][3]["value"], json!(16384.0));
        assert_eq!(catalog["disk"][4]["cost"], json!(1200.0));
        assert_eq!(catalog["session"][0]["duration"], json!("Permanent"));
        assert_eq!(catalog["session"][0].get("durationDays"), None);
        assert_eq!(catalog["session"][4]["cost"], json!(1800.0));
        assert_eq!(catalog["session"][4]["durationDays"], json!(30.0));
    }

    #[test]
    fn remaining_seconds() {
        assert_eq!(
            get_remaining_daily_vm_seconds(0.0, false, 21600.0),
            Some(21600.0)
        );
        assert_eq!(
            get_remaining_daily_vm_seconds(1000.0, false, 21600.0),
            Some(20600.0)
        );
        assert_eq!(
            get_remaining_daily_vm_seconds(99999.0, false, 21600.0),
            Some(0.0)
        );
        assert_eq!(get_remaining_daily_vm_seconds(0.0, true, 21600.0), None);
        assert_eq!(get_remaining_daily_vm_seconds(0.0, false, 86400.0), None);
        assert_eq!(
            get_remaining_daily_vm_seconds(5.9, false, 21600.0),
            Some(21595.0),
            "floors used"
        );
    }

    #[test]
    fn eligibility_rules() {
        assert!(is_eligible_for_free_vm("x@student.rjuhsd.us", false, false));
        assert!(is_eligible_for_free_vm("x@student.mitch.pro", false, false));
        assert!(is_eligible_for_free_vm("x@student.RJUHSD.us", false, false));
        assert!(is_eligible_for_free_vm("a@b.pro", true, false));
        assert!(is_eligible_for_free_vm("a@b.pro", false, true));
        assert!(!is_eligible_for_free_vm("a@b.pro", false, false));
        assert!(!is_eligible_for_free_vm("", false, true));
    }

    #[test]
    fn cooldown_and_extension() {
        assert!(can_user_extend(0.0, false, 1000.0));
        assert!(
            !can_user_extend(900.0, false, 1000.0),
            "inside 24h cooldown"
        );
        assert!(can_user_extend(0.0, true, 1000.0), "admins always can");
        assert_eq!(compute_cooldown_remaining(0.0, false, 1000.0), 0.0);
        assert_eq!(
            compute_cooldown_remaining(1500.0, false, 1000.0),
            1.0,
            "ceil"
        );
        assert_eq!(compute_cooldown_remaining(500.0, false, 1000.0), 0.0);
        assert_eq!(compute_cooldown_remaining(9999.0, true, 0.0), 0.0);
        assert!(
            !is_vm_inactive(0.0, 1_000_000.0, false),
            "lastSeen 0 → never inactive"
        );
        assert!(!is_vm_inactive(900_000.0, 1_000_000.0, false));
        assert!(is_vm_inactive(400_000.0, 1_000_000.0, false), "600s elapsed");
        assert!(!is_vm_inactive(1_000_000.0 - 29.0 * 60_000.0, 1_000_000.0, true));
        assert!(is_vm_inactive(1_000_000.0 - 30.0 * 60_000.0, 1_000_000.0, true));
    }

    #[test]
    fn formatting() {
        assert_eq!(format_uptime_duration(3661.0), "1h 1m");
        assert_eq!(format_uptime_duration(59.0), "0m");
        assert_eq!(format_uptime_duration(60.0), "1m");
        assert_eq!(format_uptime_duration(-5.0), "0m");
        let (title, message, priority) =
            format_capacity_full_alert("a@b.pro", "use a computer", 18.0);
        assert_eq!(title, "VM Capacity Alert");
        assert_eq!(
            message,
            "VM capacity full (18/18): a@b.pro attempted to use a computer."
        );
        assert_eq!(priority, "high");
        let (t2, m2) =
            format_admin_usage_notice("admin@mitch.pro", "My Computer", "opened desktop session");
        assert_eq!(t2, "Admin Used Your Computer");
        assert_eq!(m2, "Administrator admin@mitch.pro accessed your computer \"My Computer\" (opened desktop session).");
        let (t3, m3) = format_admin_access_request("admin@mitch.pro", "My Computer");
        assert_eq!(t3, "Admin Access Request");
        assert_eq!(m3, "Administrator admin@mitch.pro requested access to your computer \"My Computer\". You can allow or revoke access in your Computer settings.");
    }

    #[test]
    fn notify_throttle_map() {
        let mut map = HashMap::new();
        assert!(should_notify_capacity_alert(
            "A@B.pro",
            1_000_000.0,
            &mut map,
            60_000.0
        ));
        assert!(!should_notify_capacity_alert(
            "a@b.pro",
            1_001_000.0,
            &mut map,
            60_000.0
        ));
        assert!(should_notify_capacity_alert(
            "a@b.pro",
            1_061_000.0,
            &mut map,
            60_000.0
        ));
    }
}
