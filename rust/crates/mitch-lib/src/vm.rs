//! The SQL-backed VM data layer — port of lib/data_store.js's
//! `vmRowToRecord` + the 16 virtual_machines/vm_audit_logs/vm_usage_samples
//! functions (data_store.js:205-478).
//!
//! Compatibility contract: identical SQL against the shared `mitchpro.db`,
//! identical camelCase record shapes, identical clamping/ordering/fallbacks.
//! The bun process writes these same tables with the same statements, so
//! both processes stay interoperable during the overlap window.

use crate::data::DataStore;
use serde_json::{json, Map, Value};

/// A virtual machine record — the JS object shape returned by
/// `vmRowToRecord` (data_store.js:205-225). Serde keeps field order for
/// response bodies.
#[derive(Debug, Clone, PartialEq)]
pub struct VmRecord {
    pub id: String,
    pub owner_email: String,
    pub owner_user_id: String,
    pub vmid: f64,
    pub node: String,
    pub guest_type: String,
    pub friendly_name: String,
    pub hostname: String,
    pub operating_system: String,
    pub template_vmid: Option<f64>,
    pub cpu_cores: f64,
    pub memory_mb: f64,
    pub disk_gb: f64,
    pub ip_address: String,
    pub status: String,
    pub created_at: f64,
    pub updated_at: f64,
}

impl VmRecord {
    /// The camelCase JSON object every caller spreads into responses.
    pub fn to_json(&self) -> Value {
        let mut obj = Map::new();
        obj.insert("id".into(), json!(self.id));
        obj.insert("ownerEmail".into(), json!(self.owner_email));
        obj.insert("ownerUserId".into(), json!(self.owner_user_id));
        obj.insert("vmid".into(), json!(self.vmid));
        obj.insert("node".into(), json!(self.node));
        obj.insert("guestType".into(), json!(self.guest_type));
        obj.insert("friendlyName".into(), json!(self.friendly_name));
        obj.insert("hostname".into(), json!(self.hostname));
        obj.insert("operatingSystem".into(), json!(self.operating_system));
        obj.insert(
            "templateVmid".into(),
            self.template_vmid.map(|v| json!(v)).unwrap_or(Value::Null),
        );
        obj.insert("cpuCores".into(), json!(self.cpu_cores));
        obj.insert("memoryMb".into(), json!(self.memory_mb));
        obj.insert("diskGb".into(), json!(self.disk_gb));
        obj.insert("ipAddress".into(), json!(self.ip_address));
        obj.insert("status".into(), json!(self.status));
        obj.insert("createdAt".into(), json!(self.created_at));
        obj.insert("updatedAt".into(), json!(self.updated_at));
        Value::Object(obj)
    }
}

/// `vmRowToRecord` (data_store.js:205-225).
fn vm_row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<VmRecord> {
    Ok(VmRecord {
        id: row.get("id")?,
        owner_email: row.get("owner_email")?,
        owner_user_id: row
            .get::<_, Option<String>>("owner_user_id")?
            .unwrap_or_default(),
        vmid: row.get::<_, f64>("proxmox_vmid")?,
        node: row.get("proxmox_node")?,
        guest_type: row.get("guest_type")?,
        friendly_name: row.get("friendly_name")?,
        hostname: row.get("hostname")?,
        operating_system: row.get("operating_system")?,
        template_vmid: row.get::<_, Option<f64>>("template_vmid")?,
        cpu_cores: row.get("cpu_cores")?,
        memory_mb: row.get("memory_mb")?,
        disk_gb: row.get("disk_gb")?,
        ip_address: row
            .get::<_, Option<String>>("ip_address")?
            .unwrap_or_default(),
        status: row.get("status")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

/// `listVirtualMachines({includeUnassigned})` (data_store.js:231-236).
pub fn list_virtual_machines(store: &DataStore, include_unassigned: bool) -> Vec<VmRecord> {
    let sql = if include_unassigned {
        "SELECT * FROM virtual_machines ORDER BY created_at DESC"
    } else {
        "SELECT * FROM virtual_machines WHERE status != 'unassigned' ORDER BY created_at DESC"
    };
    let conn = store.conn();
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map([], vm_row_to_record);
    match rows {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// `getVirtualMachinesForOwner(ownerEmail)` (data_store.js:238-243).
pub fn get_virtual_machines_for_owner(store: &DataStore, owner_email: &str) -> Vec<VmRecord> {
    let conn = store.conn();
    let mut stmt = match conn.prepare(
        "SELECT * FROM virtual_machines WHERE lower(owner_email) = lower(?1) AND status != 'unassigned' ORDER BY created_at DESC",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map([owner_email], vm_row_to_record);
    match rows {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// `getVirtualMachineById(id)` (data_store.js:245-247).
pub fn get_virtual_machine_by_id(store: &DataStore, id: &str) -> Option<VmRecord> {
    let conn = store.conn();
    let mut stmt = conn
        .prepare("SELECT * FROM virtual_machines WHERE id = ?1")
        .ok()?;
    stmt.query_row([id], vm_row_to_record).ok()
}

/// `getVirtualMachineByVmid(vmid)` (data_store.js:249-251).
pub fn get_virtual_machine_by_vmid(store: &DataStore, vmid: f64) -> Option<VmRecord> {
    let conn = store.conn();
    let mut stmt = conn
        .prepare("SELECT * FROM virtual_machines WHERE proxmox_vmid = ?1")
        .ok()?;
    stmt.query_row([vmid], vm_row_to_record).ok()
}

/// `reserveVirtualMachine(record)` (data_store.js:253-259) — immediate
/// transaction: null when the vmid is already taken, else the upsert. The
/// check + insert run inside one BEGIN IMMEDIATE (cross-process safe on the
/// shared WAL database, like the JS `transaction(...).immediate()`).
pub fn reserve_virtual_machine(store: &DataStore, record: &Value) -> Option<VmRecord> {
    let vmid = record.get("vmid").and_then(Value::as_f64)?;
    {
        let conn = store.conn();
        if let Err(e) = conn.execute_batch("BEGIN IMMEDIATE") {
            tracing::warn!("[reserve_virtual_machine] BEGIN IMMEDIATE failed for vmid {vmid}: {e}");
            return None;
        }
        let blocker: Option<(String, String, String)> = conn
            .query_row(
                "SELECT id, owner_email, status FROM virtual_machines WHERE proxmox_vmid = ?1",
                rusqlite::params![vmid],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .ok();
        let exists = blocker.is_some();
        let outcome = if exists {
            None
        } else {
            match upsert_values(record) {
                Some(values) => match write_upsert(&conn, &values) {
                    Ok(_) => Some(()),
                    Err(e) => {
                        tracing::warn!(
                            "[reserve_virtual_machine] write_upsert failed for vmid {vmid}: {e}"
                        );
                        None
                    }
                },
                None => {
                    tracing::warn!("[reserve_virtual_machine] upsert_values rejected the record for vmid {vmid}");
                    None
                }
            }
        };
        if let Err(e) = conn.execute_batch("COMMIT") {
            tracing::warn!("[reserve_virtual_machine] COMMIT failed for vmid {vmid}: {e}");
        }
        if outcome.is_none() {
            if let Some((id, owner_email, status)) = blocker {
                tracing::warn!(
                    "[reserve_virtual_machine] vmid {vmid} already occupied by id={id} owner={owner_email} status={status}"
                );
            }
            return None;
        }
    }
    let readback = get_virtual_machine_by_vmid(store, vmid);
    if readback.is_none() {
        tracing::warn!(
            "[reserve_virtual_machine] insert for vmid {vmid} committed but the readback found nothing"
        );
    }
    readback
}

/// `upsertVirtualMachine(record)` (data_store.js:261-296) — INSERT … ON
/// CONFLICT(proxmox_vmid) DO UPDATE, then read back through the vmid lookup.
pub fn upsert_virtual_machine(store: &DataStore, record: &Value) -> Option<VmRecord> {
    let values = upsert_values(record)?;
    {
        let conn = store.conn();
        write_upsert(&conn, &values).ok()?;
    }
    let vmid = record.get("vmid").and_then(Value::as_f64)?;
    get_virtual_machine_by_vmid(store, vmid)
}

/// The INSERT … ON CONFLICT statement, executed with `values` as binds.
fn write_upsert(conn: &rusqlite::Connection, values: &[String]) -> Result<usize, rusqlite::Error> {
    conn.execute(
        "INSERT INTO virtual_machines (
            id, owner_email, owner_user_id, proxmox_vmid, proxmox_node, guest_type,
            friendly_name, hostname, operating_system, template_vmid, cpu_cores,
            memory_mb, disk_gb, ip_address, status, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
        ON CONFLICT(proxmox_vmid) DO UPDATE SET
            id=excluded.id, owner_email=excluded.owner_email, owner_user_id=excluded.owner_user_id,
            proxmox_node=excluded.proxmox_node, guest_type=excluded.guest_type,
            friendly_name=excluded.friendly_name, hostname=excluded.hostname,
            operating_system=excluded.operating_system, template_vmid=excluded.template_vmid,
            cpu_cores=excluded.cpu_cores, memory_mb=excluded.memory_mb, disk_gb=excluded.disk_gb,
            ip_address=excluded.ip_address, status=excluded.status, updated_at=excluded.updated_at",
        rusqlite::params_from_iter(values.iter()),
    )
}

/// The 17-column bind list of `upsertVirtualMachine` (data_store.js:262-285):
/// `String(x || fallback)` / `Number(x) || fallback` coercions per field.
fn upsert_values(record: &Value) -> Option<Vec<String>> {
    let now = now_millis();
    let created_at = record
        .get("createdAt")
        .and_then(Value::as_f64)
        .filter(|v| *v != 0.0)
        .unwrap_or(now as f64);
    let str_or = |key: &str, fallback: &str| -> String {
        match record.get(key) {
            Some(Value::String(s)) if !s.is_empty() => s.clone(),
            Some(Value::Bool(b)) => b.to_string(),
            Some(Value::Number(n)) => js_number_string(n.as_f64().unwrap_or(f64::NAN)),
            _ => fallback.to_string(),
        }
    };
    let vmid = record.get("vmid").and_then(Value::as_f64)?;
    Some(vec![
        str_or("id", ""),
        str_or("ownerEmail", "").to_lowercase(),
        str_or("ownerUserId", ""),
        js_number_string(vmid),
        str_or("node", ""),
        str_or("guestType", "qemu"),
        str_or("friendlyName", "My Computer"),
        str_or("hostname", &format!("computer-{}", js_number_string(vmid))),
        str_or("operatingSystem", "Linux Desktop"),
        record
            .get("templateVmid")
            .and_then(Value::as_f64)
            .filter(|v| !v.is_nan())
            .map(js_number_string)
            .unwrap_or_default(),
        js_number_or(record.get("cpuCores"), 2.0).to_string(),
        js_number_or(record.get("memoryMb"), 4096.0).to_string(),
        js_number_or(record.get("diskGb"), 64.0).to_string(),
        str_or("ipAddress", ""),
        str_or("status", "assigned"),
        js_number_string(created_at),
        now.to_string(),
    ])
}

/// `unassignVirtualMachine(id)` (data_store.js:298-304).
pub fn unassign_virtual_machine(store: &DataStore, id: &str) -> bool {
    let conn = store.conn();
    let result = conn.execute(
        "UPDATE virtual_machines SET owner_email = '', owner_user_id = '', status = 'unassigned', updated_at = ?1 WHERE id = ?2",
        rusqlite::params![now_millis(), id],
    );
    result.map(|n| n > 0).unwrap_or(false)
}

/// `deleteVirtualMachine(id)` (data_store.js:306-311) — deletes by id OR by
/// numeric vmid (`Number(id) || -1` when the id is not a number).
pub fn delete_virtual_machine(store: &DataStore, id: &str) -> bool {
    let vmid = js_string_to_number(id).unwrap_or(-1.0);
    let conn = store.conn();
    let result = conn.execute(
        "DELETE FROM virtual_machines WHERE id = ?1 OR proxmox_vmid = ?2",
        rusqlite::params![id, vmid],
    );
    result.map(|n| n > 0).unwrap_or(false)
}

/// `updateVirtualMachineRuntime(id, {ipAddress, status})` (data_store.js:313-323).
/// An undefined ipAddress keeps the current value; a defined falsy one clears
/// the stored string. Rust uses `Option<&str>` to model that distinction.
pub fn update_virtual_machine_runtime(
    store: &DataStore,
    id: &str,
    ip_address: Option<&str>,
    status: Option<&str>,
) -> Option<VmRecord> {
    let current = get_virtual_machine_by_id(store, id)?;
    let ip = match ip_address {
        None => current.ip_address.clone(),
        Some(v) => v.to_string(),
    };
    let st = status.unwrap_or(&current.status).to_string();
    {
        let conn = store.conn();
        let result = conn.execute(
            "UPDATE virtual_machines SET ip_address = ?1, status = ?2, updated_at = ?3 WHERE id = ?4",
            rusqlite::params![ip, st, now_millis(), id],
        );
        if result.is_err() {
            return None;
        }
    }
    get_virtual_machine_by_id(store, id)
}

/// `updateVirtualMachineSpecs(id, {cpuCores, memoryMb, diskGb})`
/// (data_store.js:325-343) — each field clamps to its server.js window.
pub fn update_virtual_machine_specs(
    store: &DataStore,
    id: &str,
    cpu_cores: Option<f64>,
    memory_mb: Option<f64>,
    disk_gb: Option<f64>,
) -> Option<VmRecord> {
    let current = get_virtual_machine_by_id(store, id)?;
    let cores = cpu_cores
        .filter(|v| !v.is_nan())
        .map(|v| v.round().clamp(2.0, 16.0))
        .unwrap_or(current.cpu_cores);
    let memory = memory_mb
        .filter(|v| !v.is_nan())
        .map(|v| v.round().clamp(2048.0, 65536.0))
        .unwrap_or(current.memory_mb);
    let disk = disk_gb
        .filter(|v| !v.is_nan())
        .map(|v| v.round().clamp(40.0, 256.0))
        .unwrap_or(current.disk_gb);
    {
        let conn = store.conn();
        let result = conn.execute(
            "UPDATE virtual_machines
            SET cpu_cores = ?1, memory_mb = ?2, disk_gb = ?3, updated_at = ?4
            WHERE id = ?5",
            rusqlite::params![cores as i64, memory as i64, disk as i64, now_millis(), id],
        );
        if result.is_err() {
            return None;
        }
    }
    get_virtual_machine_by_id(store, id)
}

/// `appendVmAuditLog(entry)` (data_store.js:345-360). `details` is a JSON
/// object here; the JS stringifies objects before `cleanLogText(…, 2000)`.
#[allow(clippy::too_many_arguments)] // mirrors the JS parameter object
pub fn append_vm_audit_log(
    store: &DataStore,
    ts: f64,
    actor_email: &str,
    owner_email: &str,
    vm_record_id: &str,
    vmid: Option<f64>,
    action: &str,
    success: bool,
    details: Option<&Value>,
) {
    let safe_details = match details {
        None => String::new(),
        Some(v) => crate::log::clean_log_text(&js_stringify(v), 2000),
    };
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO vm_audit_logs
        (ts, actor_email, owner_email, vm_record_id, proxmox_vmid, action, success, details)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            ts.max(1.0) as i64,
            if actor_email.is_empty() {
                "system"
            } else {
                actor_email
            },
            owner_email,
            vm_record_id,
            vmid,
            if action.is_empty() { "UNKNOWN" } else { action },
            if success { 1 } else { 0 },
            safe_details,
        ],
    );
}

/// One `listVmAuditLogs` row (data_store.js:362-373).
#[derive(Debug, Clone, PartialEq)]
pub struct VmAuditRow {
    pub id: i64,
    pub ts: i64,
    pub actor_email: String,
    pub owner_email: String,
    pub vm_record_id: String,
    pub vmid: Option<f64>,
    pub action: String,
    pub success: bool,
    pub details: String,
}

impl VmAuditRow {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "ts": self.ts,
            "actorEmail": self.actor_email,
            "ownerEmail": self.owner_email,
            "vmRecordId": self.vm_record_id,
            "vmid": self.vmid,
            "action": self.action,
            "success": self.success,
            "details": self.details,
        })
    }
}

/// `listVmAuditLogs(limit)` (data_store.js:362-373) — limit clamped 1..500
/// with JS `|| 150` semantics (0/NaN → 150), newest first (ts DESC, then id
/// DESC for same-ts rows).
pub fn list_vm_audit_logs(store: &DataStore, limit: f64) -> Vec<VmAuditRow> {
    let limit = js_limit_or(limit, 150.0).clamp(1.0, 500.0) as i64;
    let conn = store.conn();
    let mut stmt = match conn.prepare(
        "SELECT id, ts, actor_email, owner_email, vm_record_id, proxmox_vmid, action, success, details FROM vm_audit_logs ORDER BY ts DESC, id DESC LIMIT ?1",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map([limit], |r| {
        Ok(VmAuditRow {
            id: r.get(0)?,
            ts: r.get(1)?,
            actor_email: r.get(2)?,
            owner_email: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
            vm_record_id: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
            vmid: r.get::<_, Option<f64>>(5)?,
            action: r.get(6)?,
            success: r.get::<_, i64>(7)? != 0,
            details: r.get::<_, Option<String>>(8)?.unwrap_or_default(),
        })
    });
    match rows {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// One `listVmUsageSamples` row (data_store.js:401-424).
#[derive(Debug, Clone, PartialEq)]
pub struct VmUsageSample {
    pub id: i64,
    pub ts: i64,
    pub day_key: String,
    pub hour: i64,
    pub minute: i64,
    pub owner_email: String,
    pub vm_record_id: String,
    pub vmid: Option<f64>,
    pub vm_name: String,
    pub uptime_seconds: i64,
    pub active_users: Vec<String>,
    pub is_running: bool,
}

/// `recordVmUsageSample(sample)` (data_store.js:375-399) — server-local
/// hour/minute resolution (`d.getHours()`), lowercased/trimmed owner.
#[allow(clippy::too_many_arguments)] // mirrors the JS parameter object
pub fn record_vm_usage_sample(
    store: &DataStore,
    ts: Option<f64>,
    day_key: &str,
    hour: Option<i64>,
    minute: Option<i64>,
    owner_email: &str,
    vm_record_id: &str,
    vmid: Option<f64>,
    vm_name: &str,
    uptime_seconds: f64,
    active_users: &str,
    is_running: bool,
) {
    if owner_email.is_empty() {
        return;
    }
    let timestamp = ts
        .filter(|v| *v != 0.0 && v.is_finite())
        .unwrap_or(now_millis() as f64);
    let (resolved_day, resolved_hour, resolved_minute) = match (hour, minute) {
        (Some(h), Some(m)) => (day_key.to_string(), h, m),
        _ => {
            // JS fallback: `new Date(ts)` → toISOString().slice(0,10) for the
            // day, getHours()/getMinutes() (server-local zone) for the bucket.
            let iso_day = crate::coins::js_iso_date_from(timestamp as i64);
            let day = if day_key.is_empty() {
                iso_day
            } else {
                day_key.to_string()
            };
            let (h, m) = local_hour_minute(timestamp as i64);
            (day, h, m)
        }
    };
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO vm_usage_samples
        (ts, day_key, hour, minute, owner_email, vm_record_id, vmid, vm_name, uptime_seconds, active_users, is_running)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params![
            timestamp as i64,
            resolved_day,
            resolved_hour,
            resolved_minute,
            owner_email.trim().to_lowercase(),
            vm_record_id,
            vmid,
            vm_name,
            uptime_seconds.max(0.0).floor() as i64,
            active_users,
            if is_running { 1 } else { 0 },
        ],
    );
}

/// `listVmUsageSamples(dayKey, limit)` (data_store.js:401-421) — day-key
/// filter, ts ASC, limit clamped 1..10000, activeUsers split on ','.
pub fn list_vm_usage_samples(store: &DataStore, day_key: &str, limit: f64) -> Vec<VmUsageSample> {
    let query_day = if day_key.is_empty() {
        crate::coins::js_iso_date()
    } else {
        day_key.to_string()
    };
    let limit = js_limit_or(limit, 1000.0).clamp(1.0, 10_000.0) as i64;
    let conn = store.conn();
    let mut stmt = match conn.prepare(
        "SELECT id, ts, day_key, hour, minute, owner_email, vm_record_id, vmid, vm_name, uptime_seconds, active_users, is_running FROM vm_usage_samples WHERE day_key = ?1 ORDER BY ts ASC LIMIT ?2",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map(rusqlite::params![query_day, limit], |r| {
        let active: Option<String> = r.get(10)?;
        Ok(VmUsageSample {
            id: r.get(0)?,
            ts: r.get(1)?,
            day_key: r.get(2)?,
            hour: r.get(3)?,
            minute: r.get(4)?,
            owner_email: r.get(5)?,
            vm_record_id: r.get(6)?,
            vmid: r.get::<_, Option<f64>>(7)?,
            vm_name: r.get::<_, Option<String>>(8)?.unwrap_or_default(),
            uptime_seconds: r.get::<_, Option<i64>>(9)?.unwrap_or(0),
            active_users: active
                .unwrap_or_default()
                .split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
            is_running: r.get::<_, i64>(11)? != 0,
        })
    });
    match rows {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// One 24-bucket hour entry of `getVmUsageTimeline` (data_store.js:423-469).
#[derive(Debug, Clone, PartialEq)]
pub struct VmTimelineHour {
    pub hour: i64,
    pub label: String,
    pub full_label: String,
    pub peak_running: i64,
    pub active_sessions_peak: i64,
    pub users: Vec<Value>,
    pub samples_count: i64,
}

impl VmTimelineHour {
    pub fn to_json(&self) -> Value {
        json!({
            "hour": self.hour,
            "label": self.label,
            "fullLabel": self.full_label,
            "peakRunning": self.peak_running,
            "activeSessionsPeak": self.active_sessions_peak,
            "users": self.users,
            "samplesCount": self.samples_count,
        })
    }
}

/// `getVmUsageTimeline(dayKey)` (data_store.js:423-469) — per-hour buckets
/// with ts-snapshot peaks, per-user max uptime rollups, and the fleet peaks.
pub fn get_vm_usage_timeline(store: &DataStore, day_key: &str) -> Value {
    let query_day = if day_key.is_empty() {
        crate::coins::js_iso_date()
    } else {
        day_key.to_string()
    };
    let samples = list_vm_usage_samples(store, &query_day, 5000.0);

    let label_for = |h: usize| -> (String, String) {
        let period = if h < 12 { "AM" } else { "PM" };
        let display = if h == 0 {
            12
        } else if h > 12 {
            h - 12
        } else {
            h
        };
        (format!("{display} {period}"), format!("{:02}:00", h))
    };

    let mut hours: Vec<VmTimelineHour> = (0..24)
        .map(|h| {
            let (label, full_label) = label_for(h);
            VmTimelineHour {
                hour: h as i64,
                label,
                full_label,
                peak_running: 0,
                active_sessions_peak: 0,
                users: Vec::new(),
                samples_count: 0,
            }
        })
        .collect();

    let mut peak_concurrent_today = 0i64;

    for (h, bucket) in hours.iter_mut().enumerate() {
        let hour_samples: Vec<&VmUsageSample> =
            samples.iter().filter(|s| s.hour == h as i64).collect();
        bucket.samples_count = hour_samples.len() as i64;

        // byTs snapshots (running/active counts per ts), keyed by ts.
        let mut by_ts: Vec<(i64, i64, i64)> = Vec::new(); // (ts, running, active)
        let mut user_map: Vec<(String, Value)> = Vec::new();

        for s in &hour_samples {
            if let Some(snap) = by_ts.iter_mut().find(|(ts, _, _)| *ts == s.ts) {
                if s.is_running {
                    snap.1 += 1;
                }
                if !s.active_users.is_empty() {
                    snap.2 += 1;
                }
            } else {
                by_ts.push((
                    s.ts,
                    if s.is_running { 1 } else { 0 },
                    if s.active_users.is_empty() { 0 } else { 1 },
                ));
            }

            let existing = user_map
                .iter_mut()
                .find(|(email, _)| *email == s.owner_email)
                .map(|(_, u)| u);
            match existing {
                Some(u) => {
                    if let Some(obj) = u.as_object_mut() {
                        obj["samples"] = json!(obj["samples"].as_i64().unwrap_or(0) + 1);
                        if s.uptime_seconds > obj["maxUptimeSeconds"].as_i64().unwrap_or(0) {
                            obj["maxUptimeSeconds"] = json!(s.uptime_seconds);
                        }
                        if !s.active_users.is_empty() {
                            obj["hadActiveSession"] = json!(true);
                        }
                    }
                }
                None => user_map.push((
                    s.owner_email.clone(),
                    json!({
                        "email": s.owner_email,
                        "vmName": if s.vm_name.is_empty() { "Computer" } else { &s.vm_name },
                        "vmRecordId": s.vm_record_id,
                        "maxUptimeSeconds": s.uptime_seconds,
                        "hadActiveSession": !s.active_users.is_empty(),
                        "samples": 1,
                    }),
                )),
            }
        }

        let mut hour_peak_running = 0i64;
        let mut hour_peak_active = 0i64;
        for (_, running, active) in &by_ts {
            if *running > hour_peak_running {
                hour_peak_running = *running;
            }
            if *active > hour_peak_active {
                hour_peak_active = *active;
            }
        }
        if hour_peak_running == 0 && !user_map.is_empty() {
            hour_peak_running = user_map.len() as i64;
        }

        bucket.peak_running = hour_peak_running;
        bucket.active_sessions_peak = hour_peak_active;
        bucket.users = user_map.into_iter().map(|(_, u)| u).collect();
        if hour_peak_running > peak_concurrent_today {
            peak_concurrent_today = hour_peak_running;
        }
    }

    json!({
        "dayKey": query_day,
        "hours": hours.iter().map(VmTimelineHour::to_json).collect::<Vec<_>>(),
        "peakConcurrentToday": peak_concurrent_today,
        "totalSamplesToday": samples.len() as i64,
    })
}

/// `pruneOldVmUsageSamples(daysToKeep)` (data_store.js:471-475).
pub fn prune_old_vm_usage_samples(store: &DataStore, days_to_keep: f64) {
    let days = if days_to_keep.is_finite() && days_to_keep >= 1.0 {
        days_to_keep
    } else {
        30.0
    };
    let cutoff = now_millis() as f64 - (days * 86_400_000.0);
    let conn = store.conn();
    let _ = conn.execute(
        "DELETE FROM vm_usage_samples WHERE ts < ?1",
        rusqlite::params![cutoff as i64],
    );
}

// ── JS number/string coercion helpers ────────────────────────────────────────

/// Server-local hour/minute (`Date.getHours()/getMinutes()`): resolves the
/// system TZ offset the same way the mitch-server admin economy module does —
/// `date +%z`, which consults the live IANA zone (DST-correct).
fn local_hour_minute(millis: i64) -> (i64, i64) {
    let offset = local_tz_offset_secs();
    let secs = millis.div_euclid(1000) + offset;
    let day_secs = secs.rem_euclid(86_400);
    (day_secs / 3600, (day_secs % 3600) / 60)
}

fn local_tz_offset_secs() -> i64 {
    if let Ok(out) = std::process::Command::new("date").arg("+%z").output() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let sign = if s.starts_with('-') { -1 } else { 1 };
        let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() == 4 {
            let hours: i64 = digits[..2].parse().unwrap_or(0);
            let mins: i64 = digits[2..].parse().unwrap_or(0);
            return sign * (hours * 3600 + mins * 60);
        }
    }
    0
}

fn js_number_or(value: Option<&Value>, fallback: f64) -> f64 {
    // JS `Number(x) || fallback`: 0 and NaN fall to the fallback.
    value
        .and_then(Value::as_f64)
        .filter(|v| !v.is_nan() && *v != 0.0)
        .unwrap_or(fallback)
}

/// The `Number(limit) || fallback` coercion for the list limits (0 and NaN
/// fall to the fallback before the clamp).
fn js_limit_or(limit: f64, fallback: f64) -> f64 {
    if limit.is_nan() || limit == 0.0 {
        fallback
    } else {
        limit
    }
}

/// `String(Number(v))` for the SQL bind of a numeric vmid from a string id.
fn js_string_to_number(s: &str) -> Option<f64> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Some(0.0);
    }
    trimmed.parse::<f64>().ok()
}

/// JS `Number(v)` stringification for the hostname fallback template.
fn js_number_string(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        v.to_string()
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Re-export of the shared `cleanLogText` used by the audit details cap
/// (lib/data_store.js:555 strips ANSI escapes and caps length).
fn js_stringify(value: &Value) -> String {
    crate::data::js_stringify(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::DataStore;

    fn test_store() -> (std::path::PathBuf, DataStore) {
        let dir = std::env::temp_dir().join(format!(
            "mitch-lib-vm-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap_or_default();
        let store =
            DataStore::open(&dir, &dir.join("data")).unwrap_or_else(|e| panic!("store: {e}"));
        (dir, store)
    }

    fn sample(id: &str, vmid: f64) -> Value {
        json!({
            "id": id,
            "ownerEmail": "Owner@MITCH.pro",
            "ownerUserId": "uid-1",
            "vmid": vmid,
            "node": "tartarus",
            "guestType": "qemu",
            "friendlyName": "My Computer",
            "hostname": format!("student-{}", vmid),
            "operatingSystem": "Linux Desktop",
            "templateVmid": 9010.0,
            "cpuCores": 2.0,
            "memoryMb": 4096.0,
            "diskGb": 64.0,
            "ipAddress": "10.0.0.5",
            "status": "assigned",
            "createdAt": 1_700_000_000_000.0,
        })
    }

    #[test]
    fn upsert_and_lookup() {
        let (_dir, store) = test_store();
        let rec = upsert_virtual_machine(&store, &sample("vm-1", 200.0)).unwrap();
        assert_eq!(rec.id, "vm-1");
        // Email is lowercased on write.
        assert_eq!(rec.owner_email, "owner@mitch.pro");
        assert_eq!(rec.template_vmid, Some(9010.0));
        assert_eq!(rec.vmid, 200.0);

        assert!(get_virtual_machine_by_id(&store, "vm-1").is_some());
        assert!(get_virtual_machine_by_vmid(&store, 200.0).is_some());
        assert!(get_virtual_machine_by_id(&store, "vm-nope").is_none());

        // Upserting the same vmid updates (ON CONFLICT) rather than erroring.
        let mut second = sample("vm-1", 200.0);
        second["status"] = json!("provisioning");
        let rec2 = upsert_virtual_machine(&store, &second).unwrap();
        assert_eq!(rec2.status, "provisioning");
        assert_eq!(
            list_virtual_machines(&store, true).len(),
            1,
            "ON CONFLICT updates, never duplicates"
        );
    }

    #[test]
    fn reserve_conflict_returns_none() {
        let (_dir, store) = test_store();
        assert!(reserve_virtual_machine(&store, &sample("vm-1", 201.0)).is_some());
        assert!(
            reserve_virtual_machine(&store, &sample("vm-2", 201.0)).is_none(),
            "vmid already reserved → null"
        );
    }

    #[test]
    fn owner_filter_excludes_unassigned() {
        let (_dir, store) = test_store();
        upsert_virtual_machine(&store, &sample("vm-1", 200.0));
        unassign_virtual_machine(&store, "vm-1");
        assert!(get_virtual_machines_for_owner(&store, "owner@mitch.pro").is_empty());
        assert_eq!(
            get_virtual_machine_by_id(&store, "vm-1").unwrap().status,
            "unassigned"
        );
        // owner_user_id cleared too.
        assert_eq!(
            get_virtual_machine_by_id(&store, "vm-1")
                .unwrap()
                .owner_user_id,
            ""
        );
    }

    #[test]
    fn delete_by_id_or_vmid() {
        let (_dir, store) = test_store();
        upsert_virtual_machine(&store, &sample("vm-1", 200.0));
        // Number('200') → 200 matches the vmid even though id is 'vm-1'.
        assert!(delete_virtual_machine(&store, "200"));
        assert!(get_virtual_machine_by_id(&store, "vm-1").is_none());

        upsert_virtual_machine(&store, &sample("vm-2", 201.0));
        assert!(delete_virtual_machine(&store, "vm-2"));
        assert!(get_virtual_machine_by_id(&store, "vm-2").is_none());
        // Non-numeric, non-matching id → Number(id)||-1 = -1 → no rows.
        upsert_virtual_machine(&store, &sample("vm-3", 202.0));
        assert!(!delete_virtual_machine(&store, "zzz"));
        assert!(get_virtual_machine_by_id(&store, "vm-3").is_some());
    }

    #[test]
    fn runtime_and_spec_updates() {
        let (_dir, store) = test_store();
        upsert_virtual_machine(&store, &sample("vm-1", 200.0));
        // Undefined IP keeps the current value.
        let rec = update_virtual_machine_runtime(&store, "vm-1", None, Some("provisioning-failed"))
            .unwrap();
        assert_eq!(rec.ip_address, "10.0.0.5");
        assert_eq!(rec.status, "provisioning-failed");
        // Defined falsy IP clears it.
        let rec = update_virtual_machine_runtime(&store, "vm-1", Some(""), None).unwrap();
        assert_eq!(rec.ip_address, "");
        assert_eq!(rec.status, "provisioning-failed");

        let rec =
            update_virtual_machine_specs(&store, "vm-1", Some(8.0), Some(9999.0), Some(400.0))
                .unwrap();
        assert_eq!(rec.cpu_cores, 8.0);
        assert_eq!(rec.memory_mb, 9999.0);
        assert_eq!(rec.disk_gb, 256.0, "disk clamps to 256");
        let rec =
            update_virtual_machine_specs(&store, "vm-1", Some(1.0), Some(1.0), Some(1.0)).unwrap();
        assert_eq!(rec.cpu_cores, 2.0, "cores clamp low to 2");
        assert_eq!(rec.memory_mb, 2048.0, "memory clamps low to 2048");
        assert_eq!(rec.disk_gb, 40.0, "disk clamps low to 40");
    }

    #[test]
    fn audit_round_trip() {
        let (_dir, store) = test_store();
        append_vm_audit_log(
            &store,
            1_700_000_000_000.0,
            "actor@x.pro",
            "owner@x.pro",
            "vm-1",
            Some(200.0),
            "VM_STARTED",
            true,
            Some(&json!({"code": "OK"})),
        );
        append_vm_audit_log(
            &store,
            1_700_000_001_000.0,
            "",
            "",
            "",
            None,
            "",
            false,
            None,
        );
        let rows = list_vm_audit_logs(&store, 150.0);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].ts, 1_700_000_001_000);
        assert!(!rows[0].success);
        assert_eq!(rows[0].action, "UNKNOWN", "empty action → UNKNOWN");
        assert_eq!(rows[0].actor_email, "system", "empty actor → system");
        assert_eq!(rows[1].details, r#"{"code":"OK"}"#);
        assert_eq!(rows[1].vmid, Some(200.0));
        // Limit clamps to 1..500 with `|| 150` semantics (0/NaN → 150).
        assert_eq!(list_vm_audit_logs(&store, 0.0).len(), 2);
        assert_eq!(list_vm_audit_logs(&store, f64::NAN).len(), 2);
        assert_eq!(list_vm_audit_logs(&store, 10_000.0).len(), 2);
    }

    #[test]
    fn usage_samples_and_timeline() {
        let (_dir, store) = test_store();
        // Fixed server-local hour/minute for determinism.
        record_vm_usage_sample(
            &store,
            Some(1_700_000_000_000.0),
            "2023-11-14",
            Some(9),
            Some(30),
            "Owner@x.pro",
            "vm-1",
            Some(200.0),
            "My Computer",
            3600.0,
            "a@x.pro,b@x.pro",
            true,
        );
        record_vm_usage_sample(
            &store,
            Some(1_700_000_060_000.0),
            "2023-11-14",
            Some(9),
            Some(31),
            "other@x.pro",
            "vm-2",
            Some(201.0),
            "Other",
            60.0,
            "",
            true,
        );
        let samples = list_vm_usage_samples(&store, "2023-11-14", 1000.0);
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].owner_email, "owner@x.pro", "owner lowercased");
        assert_eq!(
            samples[0].active_users,
            vec!["a@x.pro".to_string(), "b@x.pro".to_string()]
        );
        assert!(samples[0].is_running);
        assert_eq!(samples[0].uptime_seconds, 3600);
        // Limit 0/NaN falls to the 1000 default before clamping.
        assert_eq!(list_vm_usage_samples(&store, "2023-11-14", 0.0).len(), 2);
        assert_eq!(
            list_vm_usage_samples(&store, "2023-11-14", f64::NAN).len(),
            2
        );

        let timeline = get_vm_usage_timeline(&store, "2023-11-14");
        assert_eq!(timeline["dayKey"], json!("2023-11-14"));
        assert_eq!(timeline["totalSamplesToday"], json!(2));
        let hours = timeline["hours"].as_array().unwrap();
        assert_eq!(hours.len(), 24);
        // Label mapping: 0 → "12 AM", 13 → "1 PM", 12 → "12 PM".
        assert_eq!(hours[0]["label"], json!("12 AM"));
        assert_eq!(hours[12]["label"], json!("12 PM"));
        assert_eq!(hours[13]["label"], json!("1 PM"));
        assert_eq!(hours[13]["fullLabel"], json!("13:00"));
        let hour9 = &hours[9];
        assert_eq!(hour9["samplesCount"], json!(2));
        // Per-ts snapshots: each ts holds exactly one running sample.
        assert_eq!(hour9["peakRunning"], json!(1));
        assert_eq!(
            hour9["activeSessionsPeak"],
            json!(1),
            "one snapshot with active users"
        );
        assert_eq!(hour9["users"].as_array().unwrap().len(), 2);
        let owner = hour9["users"]
            .as_array()
            .unwrap()
            .iter()
            .find(|u| u["email"] == json!("owner@x.pro"))
            .unwrap();
        assert_eq!(owner["maxUptimeSeconds"], json!(3600));
        assert!(owner["hadActiveSession"].as_bool().unwrap());
        assert_eq!(timeline["peakConcurrentToday"], json!(1));
    }

    #[test]
    fn timeline_fallback_peak_from_user_count() {
        let (_dir, store) = test_store();
        // All samples share one ts → running snapshot count is 1 each, but
        // two distinct users in the hour → peakRunning falls back to 2.
        record_vm_usage_sample(
            &store,
            Some(1_700_000_000_000.0),
            "2023-11-14",
            Some(9),
            Some(30),
            "a@x.pro",
            "vm-1",
            Some(200.0),
            "",
            10.0,
            "",
            false,
        );
        record_vm_usage_sample(
            &store,
            Some(1_700_000_000_000.0),
            "2023-11-14",
            Some(9),
            Some(30),
            "b@x.pro",
            "vm-2",
            Some(201.0),
            "",
            10.0,
            "",
            false,
        );
        let timeline = get_vm_usage_timeline(&store, "2023-11-14");
        let hours = timeline["hours"].as_array().unwrap();
        assert_eq!(hours[9]["peakRunning"], json!(2));
        assert_eq!(hours[9]["activeSessionsPeak"], json!(0));
    }

    #[test]
    fn prune_old_samples() {
        let (_dir, store) = test_store();
        let old = (now_millis() as f64) - 40.0 * 86_400_000.0;
        let fresh = now_millis() as f64;
        record_vm_usage_sample(
            &store,
            Some(old),
            "2024-01-01",
            Some(1),
            Some(0),
            "a@x.pro",
            "vm-1",
            None,
            "",
            1.0,
            "",
            true,
        );
        record_vm_usage_sample(
            &store,
            Some(fresh),
            "2024-01-02",
            Some(1),
            Some(0),
            "a@x.pro",
            "vm-1",
            None,
            "",
            1.0,
            "",
            true,
        );
        prune_old_vm_usage_samples(&store, 30.0);
        assert!(
            list_vm_usage_samples(&store, "2024-01-01", 1000.0).is_empty(),
            "40d-old sample pruned"
        );
        assert_eq!(
            list_vm_usage_samples(&store, "2024-01-02", 1000.0).len(),
            1,
            "fresh sample kept"
        );
        // daysToKeep below 1 clamps to 1 → cutoff = now - 1d; the fresh
        // sample (ts = now) still survives.
        prune_old_vm_usage_samples(&store, 0.0);
        assert_eq!(list_vm_usage_samples(&store, "2024-01-02", 1000.0).len(), 1);
    }

    #[test]
    fn record_without_owner_is_ignored() {
        let (_dir, store) = test_store();
        record_vm_usage_sample(
            &store, None, "", None, None, "", "vm-1", None, "", 1.0, "", true,
        );
        assert!(list_vm_usage_samples(&store, "", 1000.0).is_empty());
    }
}
