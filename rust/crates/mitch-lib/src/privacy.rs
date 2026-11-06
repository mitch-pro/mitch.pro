//! Startup cleanup of personal data the site no longer keeps.
//!
//! When code stops recording something, data written by older versions has to
//! go too — otherwise "we don't store that" is only true for new users. Every
//! step in [`run_startup_cleanup`] is idempotent: on a database that is already
//! clean it changes nothing, so it simply runs on every server start (and so
//! on every deploy). See `docs/privacy.md` for what is kept and why.
//!
//! What it removes:
//! - `sessions.json` — the per-ping log of session ids and timestamps (older
//!   rows also had the page and IP). Nothing writes it any more.
//! - `last_known_ips.json` — each account's most recent IP address.
//! - `search_intent.json` — retired search log (query + account).
//! - `banned_ips.json` — legacy blob, already imported into `banned_ips`.
//! - `cheat_logs.json` — moved to the `cheat_logs` table without IPs.
//! - IP address, user agent and last-used time in `auth_sessions.json`.
//! - Raw IPs in `banned_ips` (hashed) and account-linked rows (deleted).
//! - Sub-day precision in `user_stats.last_active_at`.
//! - Staff IP address and user agent in `admin_actions.json` details.
//! - `enroll_ip` in `tokens`, and `ip` in `app_logs` details/messages.
//! - Support-ticket IP hashes older than a day (`support::prune_old`).

use crate::data::DataStore;
use serde_json::Value;
use std::path::Path;

/// JSON documents that are deleted outright, relative to the data dir.
pub const RETIRED_DOCUMENTS: &[&str] = &[
    "sessions.json",
    "last_known_ips.json",
    "search_intent.json",
    "banned_ips.json",
];

/// Deletes one JSON document from both places the store may hold it: the
/// `json_documents` table and the file on disk.
pub fn delete_document(store: &DataStore, file: &Path) {
    let key = store.relative_key(file);
    {
        let conn = store.conn();
        let _ = conn.execute(
            "DELETE FROM json_documents WHERE path = ?1",
            rusqlite::params![key],
        );
    }
    let _ = std::fs::remove_file(file);
}

/// Removes `keys` from every object in `record` (one level), returning
/// whether anything changed.
fn strip_keys(record: &mut Value, keys: &[&str]) -> bool {
    let Some(obj) = record.as_object_mut() else {
        return false;
    };
    let mut changed = false;
    for key in keys {
        changed |= obj.remove(*key).is_some();
    }
    changed
}

/// Drops `ip`, `userAgent` and `lastSeen` from every stored login session.
fn scrub_auth_sessions(store: &DataStore) -> usize {
    let file = store.base_dir.join("data/auth_sessions.json");
    let mut sessions = store.read_document(&file, Value::Null);
    let Some(map) = sessions.as_object_mut() else {
        return 0;
    };
    let mut changed = 0;
    for record in map.values_mut() {
        if strip_keys(record, &["ip", "userAgent", "lastSeen"]) {
            changed += 1;
        }
    }
    if changed > 0 {
        let _ = store.write_document(&file, &sessions);
    }
    changed
}

/// Drops staff `ip` / `userAgent` from admin action log details.
fn scrub_admin_actions(store: &DataStore) -> usize {
    let file = store.data_dir().join("admin_actions.json");
    let mut actions = store.read_document(&file, Value::Null);
    let Some(list) = actions.as_array_mut() else {
        return 0;
    };
    let mut changed = 0;
    for entry in list.iter_mut() {
        if let Some(details) = entry.get_mut("details") {
            if strip_keys(details, &["ip", "userAgent"]) {
                changed += 1;
            }
        }
    }
    if changed > 0 {
        let _ = store.write_document(&file, &actions);
    }
    changed
}

/// SQL-side scrubs: token enrollment IPs and IPs in application logs.
fn scrub_sql(store: &DataStore) -> usize {
    let conn = store.conn();
    let mut changed = 0;
    changed += conn
        .execute(
            "UPDATE tokens SET data = json_remove(data, '$.enroll_ip')
              WHERE json_valid(data) AND json_type(data, '$.enroll_ip') IS NOT NULL",
            [],
        )
        .unwrap_or(0);
    changed += conn
        .execute(
            "UPDATE app_logs SET details = json_remove(details, '$.ip')
              WHERE json_valid(details) AND json_type(details, '$.ip') IS NOT NULL",
            [],
        )
        .unwrap_or(0);
    changed += conn
        .execute(
            "UPDATE app_logs SET message = '[recaptcha] Blocked: empty token received'
              WHERE message LIKE '[recaptcha] Blocked: empty token received (from IP:%'",
            [],
        )
        .unwrap_or(0);
    changed
}

/// What one cleanup run changed, for the startup log line.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CleanupReport {
    pub documents_deleted: usize,
    pub auth_sessions_scrubbed: usize,
    pub banned_ips_deleted: usize,
    pub banned_ips_hashed: usize,
    pub user_stats_floored: usize,
    pub cheat_logs_imported: usize,
    pub admin_actions_scrubbed: usize,
    pub sql_rows_scrubbed: usize,
}

impl CleanupReport {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Runs every cleanup step. `id_secret` keys the IP-ban hash.
pub fn run_startup_cleanup(store: &DataStore, id_secret: &[u8]) -> CleanupReport {
    // Cheat logs first: the import reads the JSON before deleting it.
    let mut report = CleanupReport {
        cheat_logs_imported: crate::admin::migrate_cheat_logs_from_json(store),
        ..Default::default()
    };
    for name in RETIRED_DOCUMENTS {
        let mut paths = vec![store.data_dir().join(name)];
        let legacy = store.base_dir.join("data").join(name);
        if legacy != paths[0] {
            paths.push(legacy);
        }
        for file in paths {
            let key = store.relative_key(&file);
            let in_db: bool = store
                .conn()
                .query_row(
                    "SELECT 1 FROM json_documents WHERE path = ?1",
                    rusqlite::params![key],
                    |_| Ok(()),
                )
                .is_ok();
            if in_db || file.exists() {
                delete_document(store, &file);
                report.documents_deleted += 1;
            }
        }
    }
    report.auth_sessions_scrubbed = scrub_auth_sessions(store);
    let (deleted, hashed) = crate::bans::hash_legacy_banned_ips(store, id_secret);
    report.banned_ips_deleted = deleted;
    report.banned_ips_hashed = hashed;
    report.user_stats_floored = crate::user_stats::floor_last_active_to_day(store);
    report.admin_actions_scrubbed = scrub_admin_actions(store);
    report.sql_rows_scrubbed = scrub_sql(store);
    // Support tickets are otherwise pruned only when a new one is opened.
    crate::support::prune_old(
        store,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
    );
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_store(tag: &str) -> (std::path::PathBuf, DataStore) {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-privacy-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        let store = DataStore::open(&base, &base.join("data")).unwrap();
        (base, store)
    }

    #[test]
    fn removes_retired_documents_and_scrubs_records() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let (base, store) = temp_store("cleanup");
        let data = base.join("data");
        store
            .write_document(
                &data.join("sessions.json"),
                &json!([{"id": "sid", "page": "/games/x", "ip": "198.51.100.7"}]),
            )
            .unwrap();
        store
            .write_document(
                &data.join("last_known_ips.json"),
                &json!({"a@b.c": "198.51.100.7"}),
            )
            .unwrap();
        std::fs::write(data.join("search_intent.json"), "[]").unwrap();
        store
            .write_document(
                &data.join("auth_sessions.json"),
                &json!({"k": {"email": "a@b.c", "gen": 0, "expiresAt": 9, "ip": "198.51.100.7", "userAgent": "UA", "lastSeen": 5}}),
            )
            .unwrap();
        store
            .write_document(
                &data.join("admin_actions.json"),
                &json!([{"action": "x", "details": {"role": "admin", "ip": "198.51.100.7", "userAgent": "UA"}}]),
            )
            .unwrap();
        store
            .write_document(
                &data.join("cheat_logs.json"),
                &json!([{"email": "a@b.c", "game": "g", "details": "d", "ts": now, "ip": "198.51.100.7"}]),
            )
            .unwrap();
        crate::user_stats::user_stats_set(
            &store,
            "a@b.c",
            &json!({"last_active_at": 86_400_000_i64 * 3 + 12_345}),
        );

        let report = run_startup_cleanup(&store, b"secret");
        assert_eq!(report.documents_deleted, 3);
        assert_eq!(report.auth_sessions_scrubbed, 1);
        assert_eq!(report.cheat_logs_imported, 1);
        assert_eq!(report.admin_actions_scrubbed, 1);
        assert_eq!(report.user_stats_floored, 1);

        for name in [
            "sessions.json",
            "last_known_ips.json",
            "search_intent.json",
            "cheat_logs.json",
        ] {
            assert!(
                store.read_document(&data.join(name), Value::Null).is_null(),
                "{name}"
            );
            assert!(!data.join(name).exists(), "{name}");
        }
        let sessions = store.read_document(&data.join("auth_sessions.json"), Value::Null);
        assert_eq!(
            sessions["k"],
            json!({"email": "a@b.c", "gen": 0, "expiresAt": 9})
        );
        let actions = store.read_document(&data.join("admin_actions.json"), Value::Null);
        assert_eq!(actions[0]["details"], json!({"role": "admin"}));
        let (cheats, total) = crate::admin::cheat_logs_recent(&store, 10);
        assert_eq!(total, 1);
        assert!(cheats[0].get("ip").is_none());
        assert_eq!(
            crate::user_stats::user_stats_get(&store, "a@b.c")["last_active_at"],
            json!(86_400_000_i64 * 3)
        );

        // A second run finds nothing left to do.
        assert!(run_startup_cleanup(&store, b"secret").is_empty());
        std::fs::remove_dir_all(base).ok();
    }
}
