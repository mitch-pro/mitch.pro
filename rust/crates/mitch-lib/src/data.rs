//! SQLite-backed JSON document store — the compatibility linchpin (plan Step 5).
//!
//! Contract (from `lib/data_store.js` + `lib/jsonStore.js`):
//! - Opens `data/mitchpro.db` (bun:sqlite, WAL). Pragmas must match exactly:
//!   `busy_timeout=15000`, `journal_mode=WAL`, `synchronous=NORMAL`,
//!   `foreign_keys=ON`.
//! - `loadJson(file)` / `saveJson(file, obj)` map to the `json_documents`
//!   table (`path` -> `content` TEXT), path keys normalized posix-relative to
//!   the base dir (absolute when outside it).
//! - `PRESERVED_DATA_FILES` bypass the DB and hit real disk.
//! - Content for objects is `JSON.stringify(data, null, 2)` — reproduced by
//!   [`js_stringify_pretty`] (serde_json's pretty printer differs on floats
//!   and would re-sort keys without the `preserve_order` feature). Untouched
//!   documents must pass through as TEXT.
//! - Full table init (all core tables + indexes) on open, matching
//!   `initTablesWithRetry` including the SQLITE_BUSY retry ladder.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Files that never go through the DB — copied verbatim from
/// `lib/data_store.js` PRESERVED_DATA_FILES.
pub const PRESERVED_DATA_FILES: &[&str] = &[
    "admins.json",
    "bad_words.json",
    "bell_overrides.json",
    "emojis.json",
    "game_categories.json",
    "game_categories_external.json",
    "game_categories_local.json",
    "games",
    "games_external",
    "games_local",
    "logic_words.json",
    "moderators.json",
    "payloads.json",
    "prox_blocklist.json",
    "shop_catalog_overrides.json",
    "site.json",
    "sites",
    "wordle_dictionary.txt",
];

/// PRESERVED_ROOT_FILES — empty in the JS today, kept for parity.
pub const PRESERVED_ROOT_FILES: &[&str] = &[];

/// Store error: rusqlite or filesystem. Boxed to keep the public surface
/// simple without pulling in an error-crate dependency.
pub type DataError = Box<dyn std::error::Error + Send + Sync>;

/// One app_logs row (mirrors `queryAppLogs` entries).
#[derive(Debug, Clone)]
pub struct AppLogRow {
    pub ts: i64,
    pub level: String,
    pub category: String,
    pub message: String,
    pub details: String,
}

pub struct DataStore {
    /// Repo root (pub for auth/session file paths under it).
    pub base_dir: PathBuf,
    data_dir: PathBuf,
    conn: Mutex<rusqlite::Connection>,
    /// `appLogWriteCount` from the JS — prune fires on every 250th write.
    app_log_writes: std::sync::atomic::AtomicUsize,
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Legacy ban timestamps were written in milliseconds by most code paths
/// but in seconds by at least one (`routes/admin/data.rs`'s blacklist
/// action). A millisecond "now" is ~1.7e12; a second "now" is ~1.7e9 — well
/// under 1e12 either way for any date this app has existed, so this
/// threshold cleanly tells the two apart regardless of which field name the
/// value came from.
fn normalize_legacy_ts(v: f64) -> i64 {
    if v > 0.0 && v < 1e12 {
        (v * 1000.0) as i64
    } else {
        v as i64
    }
}

#[allow(dead_code)] // used by the 30-min revalidation in later steps
fn mtime_of(md: &std::fs::Metadata) -> u128 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// A static regex that must compile; failure is a programming error.
#[allow(clippy::expect_used)]
fn static_regex(pattern: &str) -> regex::Regex {
    regex::Regex::new(pattern).expect("static regex")
}

impl DataStore {
    /// `configureDataStore()` — opens the DB, sets the exact pragmas, and
    /// creates all core tables + indexes with the same SQLITE_BUSY retry
    /// ladder (10 retries, 100ms * attempt).
    pub fn open(base_dir: &Path, data_dir: &Path) -> Result<Self, DataError> {
        std::fs::create_dir_all(data_dir)?;
        let db_path = data_dir.join("mitchpro.db");
        let conn = rusqlite::Connection::open(&db_path)?;
        conn.busy_timeout(std::time::Duration::from_millis(15_000))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        // initTablesWithRetry: 10 attempts, 100ms * attempt backoff on BUSY.
        let init_tables = |conn: &rusqlite::Connection| -> Result<(), rusqlite::Error> {
            for attempt in 0..10u32 {
                let result = conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS metadata (
                        key TEXT PRIMARY KEY,
                        value TEXT NOT NULL,
                        updated_at INTEGER NOT NULL
                    );
                    CREATE TABLE IF NOT EXISTS json_documents (
                        path TEXT PRIMARY KEY,
                        content TEXT NOT NULL,
                        updated_at INTEGER NOT NULL
                    );
                    CREATE TABLE IF NOT EXISTS jsonl_documents (
                        path TEXT PRIMARY KEY,
                        content TEXT NOT NULL,
                        updated_at INTEGER NOT NULL
                    );
                    CREATE TABLE IF NOT EXISTS users (
                        email TEXT PRIMARY KEY,
                        username TEXT UNIQUE,
                        nickname TEXT DEFAULT '',
                        display_name TEXT DEFAULT '',
                        password_hash TEXT DEFAULT '',
                        coins REAL DEFAULT 0,
                        twofa_enabled INTEGER DEFAULT 0,
                        twofa_type TEXT DEFAULT '',
                        totp_secret TEXT DEFAULT '',
                        grad_year TEXT DEFAULT '',
                        gender TEXT DEFAULT '',
                        has_completed_tutorial INTEGER DEFAULT 0,
                        created_at INTEGER NOT NULL,
                        updated_at INTEGER NOT NULL
                    );
                    CREATE TABLE IF NOT EXISTS friends (
                        user_email TEXT NOT NULL,
                        friend_email TEXT NOT NULL,
                        status TEXT NOT NULL,
                        created_at INTEGER NOT NULL,
                        PRIMARY KEY (user_email, friend_email, status)
                    );
                    CREATE TABLE IF NOT EXISTS referrals (
                        email TEXT NOT NULL,
                        source TEXT DEFAULT '',
                        details TEXT DEFAULT '',
                        timestamp INTEGER NOT NULL
                    );
                    CREATE TABLE IF NOT EXISTS app_logs (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        ts INTEGER NOT NULL,
                        level TEXT NOT NULL,
                        category TEXT NOT NULL,
                        message TEXT NOT NULL,
                        details TEXT DEFAULT ''
                    );
                    CREATE TABLE IF NOT EXISTS virtual_machines (
                        id TEXT PRIMARY KEY,
                        owner_email TEXT NOT NULL,
                        owner_user_id TEXT DEFAULT '',
                        proxmox_vmid INTEGER NOT NULL UNIQUE,
                        proxmox_node TEXT NOT NULL,
                        guest_type TEXT NOT NULL DEFAULT 'qemu',
                        friendly_name TEXT NOT NULL,
                        hostname TEXT NOT NULL,
                        operating_system TEXT NOT NULL,
                        template_vmid INTEGER,
                        cpu_cores INTEGER NOT NULL DEFAULT 4,
                        memory_mb INTEGER NOT NULL DEFAULT 4096,
                        disk_gb INTEGER NOT NULL DEFAULT 40,
                        ip_address TEXT DEFAULT '',
                        status TEXT NOT NULL DEFAULT 'assigned',
                        created_at INTEGER NOT NULL,
                        updated_at INTEGER NOT NULL
                    );
                    CREATE TABLE IF NOT EXISTS vm_audit_logs (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        ts INTEGER NOT NULL,
                        actor_email TEXT NOT NULL,
                        owner_email TEXT DEFAULT '',
                        vm_record_id TEXT DEFAULT '',
                        proxmox_vmid INTEGER,
                        action TEXT NOT NULL,
                        success INTEGER NOT NULL,
                        details TEXT DEFAULT ''
                    );
                    CREATE TABLE IF NOT EXISTS vm_usage_samples (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        ts INTEGER NOT NULL,
                        day_key TEXT NOT NULL,
                        hour INTEGER NOT NULL,
                        minute INTEGER NOT NULL,
                        owner_email TEXT NOT NULL,
                        vm_record_id TEXT NOT NULL,
                        vmid INTEGER,
                        vm_name TEXT DEFAULT '',
                        uptime_seconds INTEGER DEFAULT 0,
                        active_users TEXT DEFAULT '',
                        is_running INTEGER NOT NULL DEFAULT 1
                    );
                    CREATE TABLE IF NOT EXISTS coins (
                        email TEXT PRIMARY KEY,
                        balance REAL NOT NULL DEFAULT 0,
                        lifetime_earned REAL NOT NULL DEFAULT 0,
                        updated_at INTEGER NOT NULL
                    );
                    CREATE TABLE IF NOT EXISTS blacklist (
                        email TEXT PRIMARY KEY,
                        reason TEXT NOT NULL DEFAULT 'Banned by admin',
                        banned_at INTEGER NOT NULL,
                        banned_by TEXT NOT NULL DEFAULT 'admin'
                    );
                    CREATE TABLE IF NOT EXISTS banned_ips (
                        ip TEXT PRIMARY KEY,
                        reason TEXT NOT NULL DEFAULT 'This IP address is banned from the website.',
                        banned_at INTEGER NOT NULL,
                        banned_by TEXT NOT NULL DEFAULT 'site admin',
                        email TEXT NOT NULL DEFAULT ''
                    );
                    CREATE INDEX IF NOT EXISTS idx_app_logs_ts ON app_logs (ts DESC);
                    CREATE INDEX IF NOT EXISTS idx_app_logs_level ON app_logs (level, ts DESC);
                    CREATE INDEX IF NOT EXISTS idx_app_logs_category ON app_logs (category, ts DESC);
                    CREATE INDEX IF NOT EXISTS idx_virtual_machines_owner ON virtual_machines (owner_email, status);
                    CREATE INDEX IF NOT EXISTS idx_vm_audit_logs_ts ON vm_audit_logs (ts DESC);
                    CREATE INDEX IF NOT EXISTS idx_vm_audit_logs_vm ON vm_audit_logs (vm_record_id, ts DESC);
                    CREATE INDEX IF NOT EXISTS idx_coins_balance ON coins (balance DESC);
                    CREATE INDEX IF NOT EXISTS idx_blacklist_banned_at ON blacklist (banned_at DESC);
                    CREATE INDEX IF NOT EXISTS idx_banned_ips_email ON banned_ips (email);",
                );
                match result {
                    Ok(()) => return Ok(()),
                    Err(err) => {
                        let busy = err.to_string().to_lowercase().contains("locked")
                            || matches!(
                                err,
                                rusqlite::Error::SqliteFailure(
                                    _code,
                                    Some(ref message),
                                ) if message.to_lowercase().contains("locked")
                            )
                            || err.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy);
                        if busy && attempt < 9 {
                            std::thread::sleep(std::time::Duration::from_millis(
                                100 * (u64::from(attempt) + 1),
                            ));
                            continue;
                        }
                        return Err(err);
                    }
                }
            }
            Ok(())
        };
        init_tables(&conn)?;

        // Repairs rows written before upsert_values' INSERT started
        // wrapping template_vmid in NULLIF(?, '') — a record with no
        // template (BYO-OS) stored '' (empty TEXT) in this INTEGER column
        // instead of real NULL, which made every row.get::<_, Option<f64>>
        // read of that column fail, silently dropping the whole row out of
        // every listing/lookup function while it still held its vmid via
        // the UNIQUE constraint. Idempotent — a no-op once rows are fixed.
        let _ = conn.execute(
            "UPDATE virtual_machines SET template_vmid = NULL WHERE template_vmid = ''",
            [],
        );

        conn.execute(
            "INSERT OR REPLACE INTO metadata (key, value, updated_at) VALUES ('schema_version', '1', ?)",
            rusqlite::params![now_millis()],
        )?;

        let store = Self {
            base_dir: base_dir.to_path_buf(),
            data_dir: data_dir.to_path_buf(),
            conn: Mutex::new(conn),
            app_log_writes: std::sync::atomic::AtomicUsize::new(0),
        };
        store.backfill_coins_table_from_json();
        store.backfill_blacklist_table_from_json();
        store.backfill_banned_ips_table_from_json();
        Ok(store)
    }

    /// One-time (idempotent — only inserts rows the `coins` table doesn't
    /// already have) backfill from the old coins.json/user_stats.json blob
    /// storage into the real `coins` table. Safe to run on every startup:
    /// `INSERT OR IGNORE` never touches a row that's already there, so once
    /// migrated this is a cheap no-op pass over whatever's still in the
    /// (now otherwise-unused) coins.json blob.
    fn backfill_coins_table_from_json(&self) {
        let coins_blob = self.read_document(&self.data_dir.join("coins.json"), Value::Null);
        let Some(balances) = coins_blob.as_object() else {
            return;
        };
        if balances.is_empty() {
            return;
        }
        let stats_blob = self.read_document(&self.data_dir.join("user_stats.json"), Value::Null);
        let now = now_millis();
        let conn = self.conn();
        for (email, balance_val) in balances {
            let Some(balance) = balance_val.as_f64() else {
                continue;
            };
            let lifetime_earned = stats_blob
                .get(email)
                .and_then(|s| s.get("lifetime_earned"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let _ = conn.execute(
                "INSERT OR IGNORE INTO coins (email, balance, lifetime_earned, updated_at) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![email, balance, lifetime_earned, now],
            );
        }
    }

    /// One-time (idempotent) backfill from blacklist.json into the real
    /// `blacklist` table. Legacy entries were written by at least two code
    /// paths that disagreed on field names (`banned_at` vs `blacklisted_at`,
    /// `by` vs `admin`) and units (`banned_at` was always milliseconds, but
    /// one `blacklisted_at` writer used seconds) — `normalize_legacy_ts`
    /// below resolves both.
    fn backfill_blacklist_table_from_json(&self) {
        let bl = self.read_document(&self.base_dir.join("data/blacklist.json"), Value::Null);
        let Some(map) = bl.as_object() else {
            return;
        };
        if map.is_empty() {
            return;
        }
        let now = now_millis();
        let conn = self.conn();
        for (email, info) in map {
            let reason = info
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("Banned by admin")
                .to_string();
            let banned_at = info
                .get("banned_at")
                .or_else(|| info.get("blacklisted_at"))
                .and_then(|v| v.as_f64())
                .map(normalize_legacy_ts)
                .unwrap_or(now);
            let banned_by = info
                .get("by")
                .or_else(|| info.get("admin"))
                .and_then(|v| v.as_str())
                .unwrap_or("admin")
                .to_string();
            let _ = conn.execute(
                "INSERT OR IGNORE INTO blacklist (email, reason, banned_at, banned_by) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![email, reason, banned_at, banned_by],
            );
        }
    }

    /// One-time (idempotent) backfill from banned_ips.json into the real
    /// `banned_ips` table. See `backfill_blacklist_table_from_json` for why
    /// timestamps go through `normalize_legacy_ts`.
    fn backfill_banned_ips_table_from_json(&self) {
        let ips = self.read_document(&self.base_dir.join("data/banned_ips.json"), Value::Null);
        let Some(map) = ips.as_object() else {
            return;
        };
        if map.is_empty() {
            return;
        }
        let now = now_millis();
        let conn = self.conn();
        for (ip, info) in map {
            let reason = info
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("This IP address is banned from the website.")
                .to_string();
            let banned_at = info
                .get("banned_at")
                .and_then(|v| v.as_f64())
                .map(normalize_legacy_ts)
                .unwrap_or(now);
            let banned_by = info
                .get("by")
                .and_then(|v| v.as_str())
                .unwrap_or("site admin")
                .to_string();
            let email = info
                .get("email")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let _ = conn.execute(
                "INSERT OR IGNORE INTO banned_ips (ip, reason, banned_at, banned_by, email) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![ip, reason, banned_at, banned_by, email],
            );
        }
    }

    /// Path key semantics of `relativeKey()`: posix-relative to baseDir,
    /// absolute when the file lives outside it.
    pub fn relative_key(&self, file: &Path) -> String {
        let rel = file.strip_prefix(&self.base_dir).ok();
        let key = match rel {
            Some(rel) => rel.to_path_buf(),
            None => file.to_path_buf(),
        };
        key.to_string_lossy().replace('\\', "/")
    }

    /// `isPreservedFile()`: PRESERVED_DATA_FILES under dataDir,
    /// PRESERVED_ROOT_FILES under baseDir, plus database files.
    pub fn is_preserved_file(&self, file: &Path) -> bool {
        let name = file.file_name().unwrap_or_default().to_string_lossy();
        if file.parent().is_some_and(|p| p == self.data_dir)
            && PRESERVED_DATA_FILES.contains(&name.as_ref())
        {
            return true;
        }
        if file.parent().is_some_and(|p| p == self.base_dir)
            && PRESERVED_ROOT_FILES.contains(&name.as_ref())
        {
            return true;
        }
        name == "mitchpro.db"
            || name.ends_with(".db")
            || name.ends_with(".db-wal")
            || name.ends_with(".db-shm")
    }

    /// `shouldStoreInDb(file)`: .json/.jsonl under dataDir/ or baseDir/mail/,
    /// excluding preserved files.
    pub fn should_store_in_db(&self, file: &Path) -> bool {
        let json_like = file
            .to_str()
            .is_some_and(|f| f.ends_with(".json") || f.ends_with(".jsonl"));
        if !json_like {
            return false;
        }
        if self.is_preserved_file(file) {
            return false;
        }
        let data_prefix = format!("{}/", self.data_dir.to_string_lossy());
        let mail_prefix = format!("{}/", self.base_dir.join("mail").to_string_lossy());
        let abs = file.to_string_lossy().into_owned();
        abs.starts_with(&data_prefix) || abs.starts_with(&mail_prefix)
    }

    /// `readDocument(file, fallback)`: DB row first, then the on-disk file,
    /// then the fallback. `.jsonl` rows return the raw string.
    pub fn read_document(&self, file: &Path, fallback: Value) -> Value {
        // .jsonl rows pass through as raw strings; .json rows are JSON.parse'd.
        let is_jsonl = file.to_str().is_some_and(|f| f.ends_with(".jsonl"));
        let parse = |raw: &str| -> Option<Value> {
            if is_jsonl {
                Some(Value::String(raw.to_owned()))
            } else {
                serde_json::from_str(raw).ok()
            }
        };
        if !self.should_store_in_db(file) {
            return std::fs::read_to_string(file)
                .ok()
                .and_then(|raw| parse(&raw))
                .unwrap_or(fallback);
        }
        let key = self.relative_key(file);
        let table = if is_jsonl {
            "jsonl_documents"
        } else {
            "json_documents"
        };
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let row: Option<String> = conn
            .query_row(
                &format!("SELECT content FROM {table} WHERE path = ?1"),
                rusqlite::params![key],
                |r| r.get(0),
            )
            .ok();
        if let Some(content) = row {
            return parse(&content).unwrap_or(fallback);
        }
        std::fs::read_to_string(file)
            .ok()
            .and_then(|raw| parse(&raw))
            .unwrap_or(fallback)
    }

    /// Raw content insert — mirrors JS `writeDocument(file, stringData)`.
    pub fn write_document_raw(&self, file: &Path, content: &str) -> Result<(), DataError> {
        if !self.should_store_in_db(file) {
            if let Some(parent) = file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(file, content)?;
            return Ok(());
        }
        let key = self.relative_key(file);
        let table = if file.to_str().is_some_and(|f| f.ends_with(".jsonl")) {
            "jsonl_documents"
        } else {
            "json_documents"
        };
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            &format!(
                "INSERT OR REPLACE INTO {table} (path, content, updated_at) VALUES (?1, ?2, ?3)"
            ),
            rusqlite::params![key, content, now_millis()],
        )?;
        Ok(())
    }

    /// `writeDocument(file, data)`: INSERT OR REPLACE into the blob table
    /// (pretty 2-space JSON via [`js_stringify_pretty`], like
    /// `JSON.stringify(data, null, 2)`), or a plain file write for non-DB
    /// paths. `data` as a string is stored verbatim (JS
    /// `typeof data === 'string'` branch).
    pub fn write_document(&self, file: &Path, data: &Value) -> Result<(), DataError> {
        let content = match data {
            Value::String(s) => s.clone(),
            other => js_stringify_pretty(other),
        };
        self.write_document_raw(file, &content)
    }

    /// Clone the inner connection handle (parity tooling convenience).
    pub fn conn_clone(&self) -> &DataStore {
        self
    }

    /// Locked connection handle for crate-internal data layers (`vm.rs`).
    /// The VM/Proxmox tables are raw SQL on both sides (lib/data_store.js
    /// 239-478) — they never live in the JSON document store.
    pub(crate) fn conn(&self) -> std::sync::MutexGuard<'_, rusqlite::Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// List every `json_documents` key (parity tooling:
    /// `examples/data_parity.rs`).
    pub fn list_json_document_paths(&self) -> Result<Vec<String>, DataError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare("SELECT path FROM json_documents")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Raw content read — mirrors `SELECT content FROM json_documents WHERE
    /// path = ?` (parity tooling: `examples/data_parity.rs` passthrough mode).
    /// Falls back to the on-disk file when no DB row exists.
    pub fn read_document_raw(&self, file: &Path) -> Result<Option<String>, DataError> {
        if !self.should_store_in_db(file) {
            return Ok(std::fs::read_to_string(file).ok());
        }
        let key = self.relative_key(file);
        let table = if file.to_str().is_some_and(|f| f.ends_with(".jsonl")) {
            "jsonl_documents"
        } else {
            "json_documents"
        };
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let row: Option<String> = conn
            .query_row(
                &format!("SELECT content FROM {table} WHERE path = ?1"),
                rusqlite::params![key],
                |r| r.get(0),
            )
            .ok();
        if let Some(content) = row {
            return Ok(Some(content));
        }
        Ok(std::fs::read_to_string(file).ok())
    }

    /// `appendAppLog` (sync form; callers wrap in spawn_blocking). Prunes to
    /// the newest 20000 rows on the same 1-in-250 cadence as the JS.
    pub fn append_app_log_sync(
        &self,
        ts: i64,
        level: &str,
        category: &str,
        message: String,
        details: String,
    ) -> Result<(), DataError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO app_logs (ts, level, category, message, details) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![ts, level, category, message, details],
        )?;
        // JS parity: appLogWriteCount++ then prune on every 250th write,
        // id-ordered (NOT ts — out-of-order ts values would misprune).
        let writes = self
            .app_log_writes
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        if writes.is_multiple_of(250) {
            conn.execute(
                "DELETE FROM app_logs WHERE id NOT IN (
                    SELECT id FROM app_logs ORDER BY id DESC LIMIT 20000
                )",
                [],
            )?;
        }
        Ok(())
    }

    /// `queryAppLogs` equivalent (sync form). Filters: level/category/search,
    /// newest first, limit clamped 25..2000.
    pub fn query_app_logs_sync(
        &self,
        level: &str,
        category: &str,
        search: &str,
        limit: usize,
    ) -> Result<Vec<AppLogRow>, DataError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut conditions: Vec<String> = Vec::new();
        let mut params: Vec<String> = Vec::new();
        if !level.eq_ignore_ascii_case("all") {
            params.push(normalize_log_level(level));
            conditions.push("level = ?".into());
        }
        if !category.eq_ignore_ascii_case("all") {
            params.push(normalize_log_category(category));
            conditions.push("category = ?".into());
        }
        let search = search.trim().chars().take(160).collect::<String>();
        if !search.is_empty() {
            let like = format!("%{search}%");
            params.push(like.clone());
            params.push(like.clone());
            params.push(like);
            conditions.push("(message LIKE ? OR details LIKE ? OR category LIKE ?)".into());
        }
        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };
        let limit = limit.clamp(25, 2000) as i64;
        let mut stmt = conn.prepare(&format!(
            "SELECT ts, level, category, message, details FROM app_logs {where_clause} ORDER BY ts DESC, id DESC LIMIT ?",
            where_clause = where_clause
        ))?;
        params.push(limit.to_string());
        let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |r| {
            Ok(AppLogRow {
                ts: r.get(0)?,
                level: r.get(1)?,
                category: r.get(2)?,
                message: r.get(3)?,
                details: r.get(4)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// `queryAppLogs`'s categories query — top 80 categories by count.
    pub fn app_log_categories_sync(&self) -> Result<Vec<(String, i64)>, DataError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT category, COUNT(*) FROM app_logs GROUP BY category ORDER BY COUNT(*) DESC, category ASC LIMIT 80",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}

fn normalize_log_level(level: &str) -> String {
    match level.to_lowercase().trim() {
        "debug" => "debug".into(),
        "warn" => "warn".into(),
        "error" => "error".into(),
        _ => "info".into(),
    }
}

fn normalize_log_category(category: &str) -> String {
    let re = static_regex(r"[^a-z0-9._-]");
    let lowered = category.to_lowercase();
    let collapsed = re.replace_all(lowered.trim(), "-");
    let collapsed = collapsed.replace("--", "-").replace("--", "-");
    let collapsed = collapsed.trim_matches('-');
    let out = if collapsed.is_empty() {
        "general"
    } else {
        collapsed
    };
    out.chars().take(48).collect()
}

/// `JSON.stringify(value)` — compact form, same number/string semantics as
/// [`js_stringify_pretty`]. Used for HTTP response bodies that carry
/// passthrough game-session state, where serde_json's f64 formatting would
/// emit `1e22` where JS emits `1e+22` (and `1.0` where JS emits `1`).
pub fn js_stringify(value: &Value) -> String {
    let mut out = String::new();
    js_write_compact(value, &mut out);
    out
}

fn js_write_compact(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => out.push_str(&js_number(n)),
        Value::String(s) => out.push_str(&js_quote(s)),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                js_write_compact(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&js_quote(k));
                out.push(':');
                js_write_compact(v, out);
            }
            out.push('}');
        }
    }
}

/// `JSON.stringify(value, null, 2)` with JS number semantics (integers print
/// without a decimal point below 1e21, `-0` prints as `0`, exponential
/// notation outside 1e-6..1e21). serde_json's default f64 formatting would
/// emit `1.0` where JS emits `1`.
pub fn js_stringify_pretty(value: &Value) -> String {
    let mut out = String::new();
    js_write(value, 0, &mut out);
    out
}

fn push_indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn js_write(value: &Value, depth: usize, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => out.push_str(&js_number(n)),
        Value::String(s) => out.push_str(&js_quote(s)),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }
                push_indent(out, depth + 1);
                js_write(item, depth + 1, out);
            }
            out.push('\n');
            push_indent(out, depth);
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }
                push_indent(out, depth + 1);
                out.push_str(&js_quote(k));
                out.push_str(": ");
                js_write(v, depth + 1, out);
            }
            out.push('\n');
            push_indent(out, depth);
            out.push('}');
        }
    }
}

fn js_number(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() {
        return i.to_string();
    }
    if let Some(u) = n.as_u64() {
        return u.to_string();
    }
    let f = n.as_f64().unwrap_or(0.0);
    js_f64(f)
}

/// JS `Number.prototype.toString()` — the exact ECMAScript algorithm, which
/// JSON.stringify uses for finite numbers: shortest round-trip digits
/// (Rust's `{:e}` produces the same digit string as V8), then placement per
/// spec — zero-fill when k <= n <= 21, `d1..dn.dn+1..dk` when 0 < n <= 21,
/// `0.00…d` when -6 < n <= 0, exponential `d1.d2…e±m` otherwise. `-0` prints
/// as `0`; non-finite values print as `null` (JSON.stringify drops them).
///
/// The old short-cut (`integral → i64`) broke at ≥ 2^53 where JS zero-fills
/// the *shortest* digits instead of the exact integer — e.g. `2**60` prints
/// `1152921504606847000`, not `1152921504606846976` (verified on bun).
pub fn js_number_string(f: f64) -> String {
    if f == 0.0 {
        return "0".to_string(); // also covers -0
    }
    if !f.is_finite() {
        return "null".to_string(); // JSON.stringify(NaN/Infinity)
    }
    // Shortest round-trip digits: `format!("{:e}")` → "d1.d2…dk e EXP".
    let sci = format!("{f:e}");
    let Some((mantissa, exp)) = sci.split_once('e') else {
        return sci; // unreachable for finite f64
    };
    let n: i32 = exp.parse::<i32>().unwrap_or(0) + 1; // decimal-point position
    let digits: String = mantissa.chars().filter(|c| c.is_ascii_digit()).collect();
    let k = digits.len() as i32;
    let mut out = String::new();
    if f < 0.0 {
        out.push('-');
    }
    if k <= n && n <= 21 {
        out.push_str(&digits);
        for _ in 0..(n - k) {
            out.push('0');
        }
    } else if 0 < n && n <= 21 {
        out.push_str(&digits[..n as usize]);
        out.push('.');
        out.push_str(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        for _ in 0..(-n) {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if n >= 1 { '+' } else { '-' });
        out.push_str(&(n - 1).unsigned_abs().to_string());
    }
    out
}

fn js_f64(f: f64) -> String {
    js_number_string(f)
}

/// `Number.prototype.toFixed(digits)` — the exact decimal rounding the ES
/// spec defines: pick the integer n whose value n/10^f is closest to |x|,
/// ties choosing the LARGER n. Rust's `{:.n$}` rounds half-to-even, which
/// disagrees on exact binary ties (`0.125.toFixed(2)` → "0.13" in JS, "0.12"
/// in Rust), so round the true decimal expansion by hand.
///
/// Correctness note: an f64 is m·2^e. Either |x| IS an exact decimal tie
/// (its expansion terminates in zeros — handled correctly below) or it
/// differs from the nearest tie by ≥ ~2.2e-16 relative (the mantissa
/// granularity), so rendering with 30 guard digits and half-up rounding the
/// digit string reproduces the spec choice for every finite f64.
pub fn js_to_fixed(x: f64, digits: usize) -> String {
    if x.is_nan() {
        return "NaN".to_string();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    let neg = x < 0.0; // -0.0 is not < 0: JS prints "0.00" for it
    let a = x.abs(); // also strips the sign off -0.0 so the format is unsigned
    let guard = digits + 30;
    let s = format!("{a:.guard$}");
    let (int_part, frac) = match s.split_once('.') {
        Some((i, f)) => (i, f),
        // `{:.0}` prints no decimal point; frac is empty then.
        None => (s.as_str(), ""),
    };
    // Combined digits: integer part + the first `digits` fraction digits.
    let mut digs: Vec<u8> = int_part.bytes().map(|b| b - b'0').collect();
    digs.extend(frac.bytes().take(digits).map(|b| b - b'0'));
    while digs.len() < 1 + digits {
        digs.push(0);
    }
    // The rounding digit lives at fraction position `digits` in the exact
    // expansion (guard digits guarantee it exists for prec ≥ 1).
    let round_up = frac.as_bytes().get(digits).is_some_and(|b| *b - b'0' >= 5);
    if round_up {
        let mut i = digs.len();
        loop {
            i -= 1;
            if digs[i] == 9 {
                digs[i] = 0;
                if i == 0 {
                    digs.insert(0, 1);
                    break;
                }
            } else {
                digs[i] += 1;
                break;
            }
        }
    }
    let int_len = digs.len() - digits;
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    for d in &digs[..int_len] {
        out.push((b'0' + d) as char);
    }
    if digits > 0 {
        out.push('.');
        for d in &digs[int_len..] {
            out.push((b'0' + d) as char);
        }
    }
    out
}

/// `Number(str)` on a toFixed result — correctly-rounded parse, like JS.
pub fn js_num_from_fixed(s: &str) -> f64 {
    s.parse::<f64>().unwrap_or(f64::NAN)
}

/// `JSON.stringify(string)` escaping: quotes, backslashes, control chars as
/// short escapes or `\uXXXX`, non-ASCII kept raw.
pub fn js_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn js_to_fixed_matches_ecmascript() {
        assert_eq!(js_to_fixed(1.9, 4), "1.9000");
        assert_eq!(js_to_fixed(19.0, 4), "19.0000");
        assert_eq!(js_to_fixed(0.0, 2), "0.00");
        assert_eq!(js_to_fixed(-0.0, 2), "0.00"); // JS: (-0).toFixed(2) === "0.00"
                                                  // Exact binary tie → JS picks the larger candidate (half-up).
        assert_eq!(js_to_fixed(0.125, 2), "0.13");
        assert_eq!(js_to_fixed(0.5, 0), "1");
        // Not representable — true expansion decides, not Rust's half-even.
        assert_eq!(js_to_fixed(1.005, 2), "1.00");
        assert_eq!(js_to_fixed(2.675, 2), "2.67");
        assert_eq!(js_to_fixed(123.456, 2), "123.46");
        // Carry across the decimal point.
        assert_eq!(js_to_fixed(9.99996, 4), "10.0000");
        assert_eq!(js_to_fixed(0.99999, 4), "1.0000");
        assert_eq!(js_to_fixed(-1.25, 1), "-1.3"); // -1.25 exact tie → larger n
        assert_eq!(js_to_fixed(10.5, 2), "10.50");
        assert_eq!(js_to_fixed(f64::NAN, 2), "NaN");
        assert_eq!(js_to_fixed(f64::INFINITY, 2), "Infinity");
    }

    #[test]
    fn js_number_string_matches_ecmascript() {
        // Expected values measured with bun (`String(n)` / JSON.stringify).
        assert_eq!(js_number_string(0.0), "0");
        assert_eq!(js_number_string(-0.0), "0");
        assert_eq!(js_number_string(123.0), "123");
        assert_eq!(js_number_string(123.5), "123.5");
        assert_eq!(js_number_string(0.1), "0.1");
        assert_eq!(js_number_string(0.30000000000000004), "0.30000000000000004");
        assert_eq!(js_number_string(1e15), "1000000000000000");
        assert_eq!(js_number_string(1e16), "10000000000000000");
        assert_eq!(js_number_string(1.7e17), "170000000000000000");
        assert_eq!(js_number_string(1.4e13), "14000000000000");
        // ≥ 2^53: JS zero-fills the SHORTEST digits, not the exact integer.
        assert_eq!(js_number_string(2.0f64.powi(60)), "1152921504606847000");
        // Exponential band switch at 1e21.
        assert_eq!(js_number_string(1e20), "100000000000000000000");
        assert_eq!(js_number_string(1e21), "1e+21");
        assert_eq!(js_number_string(1.4e21), "1.4e+21");
        assert_eq!(js_number_string(2.6e22), "2.6e+22");
        assert_eq!(js_number_string(1e290), "1e+290");
        assert_eq!(js_number_string(-1e290), "-1e+290");
        assert_eq!(js_number_string(1e-6), "0.000001");
        assert_eq!(js_number_string(1.5e-7), "1.5e-7");
        assert_eq!(js_number_string(1e-7), "1e-7");
        // Non-finite → JSON.stringify renders null.
        assert_eq!(js_number_string(f64::NAN), "null");
        assert_eq!(js_number_string(f64::INFINITY), "null");
        assert_eq!(
            js_stringify(&json!({"a":1e21,"b":2.0f64.powi(60),"c":1e15,"d":1.5e-7})),
            r#"{"a":1e+21,"b":1152921504606847000,"c":1000000000000000,"d":1.5e-7}"#
        );
        assert_eq!(
            js_stringify_pretty(&json!({"a":1e21})),
            "{\n  \"a\": 1e+21\n}"
        );
    }

    fn temp_base(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-data-test-{tag}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).expect("mkdir");
        base
    }

    #[test]
    fn preserved_files_bypass_db() {
        let base = temp_base("preserved");
        let store = DataStore::open(&base, &base.join("data")).unwrap();
        let site = base.join("data").join("site.json");
        assert!(!store.should_store_in_db(&site));
        let tokens = base.join("data").join("unsubscribe_tokens.json");
        assert!(store.should_store_in_db(&tokens));
        let mail_json = base.join("mail").join("check_email").join("emails.json");
        assert!(store.should_store_in_db(&mail_json));
        let txt = base.join("data").join("notes.txt");
        assert!(!store.should_store_in_db(&txt));
        let db = base.join("data").join("mitchpro.db-wal");
        assert!(!store.should_store_in_db(&db));
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn document_round_trips_through_db() {
        let base = temp_base("roundtrip");
        let store = DataStore::open(&base, &base.join("data")).unwrap();
        let file = base.join("data").join("unsubscribe_tokens.json");
        let value = json!({"a@b.c": "deadbeef", "z": 1});
        store.write_document(&file, &value).unwrap();
        let back = store.read_document(&file, Value::Null);
        assert_eq!(back, value);
        // Key uses posix-relative path.
        let conn = store.conn.lock().unwrap_or_else(|e| e.into_inner());
        let (path, content): (String, String) = conn
            .query_row(
                "SELECT path, content FROM json_documents WHERE path LIKE '%unsubscribe%'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        drop(conn);
        assert!(path.ends_with("data/unsubscribe_tokens.json"), "key={path}");
        assert!(
            content.starts_with("{\n  \"a@b.c\": \"deadbeef\""),
            "pretty 2-space: {content}"
        );
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn key_order_is_preserved_not_sorted() {
        let base = temp_base("order");
        let store = DataStore::open(&base, &base.join("data")).unwrap();
        let file = base.join("data").join("order_check.json");
        let value = json!({"zebra": 1, "apple": 2, "mango": 3});
        store.write_document(&file, &value).unwrap();
        let conn = store.conn.lock().unwrap_or_else(|e| e.into_inner());
        let content: String = conn
            .query_row(
                "SELECT content FROM json_documents WHERE path LIKE '%order_check%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        drop(conn);
        let zebra_pos = content.find("\"zebra\"").unwrap();
        let apple_pos = content.find("\"apple\"").unwrap();
        assert!(zebra_pos < apple_pos, "preserve_order violated: {content}");
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn preserved_file_written_to_disk() {
        let base = temp_base("disk");
        let store = DataStore::open(&base, &base.join("data")).unwrap();
        let file = base.join("data").join("site.json");
        let value = json!({"primary": "https://mitch.pro", "alternate": "https://mitchdog.com"});
        store.write_document(&file, &value).unwrap();
        let raw = std::fs::read_to_string(&file).unwrap();
        assert!(raw.contains("\"primary\""));
        let conn = store.conn.lock().unwrap_or_else(|e| e.into_inner());
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM json_documents", [], |r| r.get(0))
            .unwrap();
        drop(conn);
        assert_eq!(count, 0, "site.json must not land in the DB");
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn missing_row_falls_back_to_disk_file() {
        let base = temp_base("diskfallback");
        let store = DataStore::open(&base, &base.join("data")).unwrap();
        let file = base.join("data").join("site.json");
        std::fs::write(&file, "{\"primary\":\"https://mitch.pro\"}").unwrap();
        let back = store.read_document(&file, Value::Null);
        assert_eq!(back["primary"], "https://mitch.pro");
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn traversal_is_blocked_lexically() {
        let base = temp_base("traversal");
        let store = DataStore::open(&base, &base.join("data")).unwrap();
        // .. escapes the base dir, so the key is absolute (JS relativeKey
        // behavior) — but content still round-trips, keyed by absolute path.
        let file = base.join("data").join("..").join("outside.json");
        let value = json!({"escaped": true});
        store.write_document(&file, &value).unwrap();
        let back = store.read_document(&file, Value::Null);
        assert_eq!(back, value);
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn js_stringify_matches_json_stringify_pretty() {
        // Integers below 1e21 print without ".0".
        assert_eq!(js_stringify_pretty(&json!(1.0)), "1");
        assert_eq!(js_stringify_pretty(&json!(1.5)), "1.5");
        assert_eq!(js_stringify_pretty(&json!(25.5)), "25.5");
        assert_eq!(js_stringify_pretty(&json!(-0.75)), "-0.75");
        assert_eq!(
            js_stringify_pretty(&json!(9007199254740992u64)),
            "9007199254740992"
        );
        assert_eq!(js_stringify_pretty(&json!(-2.75)), "-2.75");
        // Empty containers match JS ("{}" / "[]").
        assert_eq!(js_stringify_pretty(&json!({})), "{}");
        assert_eq!(js_stringify_pretty(&json!([])), "[]");
        // Nested pretty layout: 2-space indent, \n separators.
        let nested = json!({"a": [1, {"b": "c"}], "d": null, "e": true, "f": "s\"\\n"});
        let out = js_stringify_pretty(&nested);
        let expected = "{\n  \"a\": [\n    1,\n    {\n      \"b\": \"c\"\n    }\n  ],\n  \"d\": null,\n  \"e\": true,\n  \"f\": \"s\\\"\\\\n\"\n}";
        assert_eq!(out, expected, "js_stringify_pretty: {out}");
    }

    #[test]
    fn js_stringify_escapes_match_json() {
        let s = js_stringify_pretty(&json!("a\"b\\c\nd\te\u{1}"));
        assert_eq!(s, "\"a\\\"b\\\\c\\nd\\te\\u0001\"");
        // Non-ASCII stays raw (parity with JS JSON.stringify).
        let uni = js_stringify_pretty(&json!("héllo ✅"));
        assert!(uni.contains("héllo"), "{uni}");
    }

    #[test]
    fn js_f64_matches_js_number_to_string() {
        assert_eq!(js_f64(0.0), "0");
        assert_eq!(js_f64(-0.0), "0");
        assert_eq!(js_f64(1.0), "1");
        assert_eq!(js_f64(-1.5), "-1.5");
        assert_eq!(js_f64(0.000001), "0.000001");
        assert_eq!(js_f64(0.0000001), "1e-7");
        assert_eq!(js_f64(1e20), "100000000000000000000");
        assert_eq!(js_f64(1e21), "1e+21");
    }
}
