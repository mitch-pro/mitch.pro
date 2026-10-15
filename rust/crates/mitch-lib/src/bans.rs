//! Account and IP ban data layer — real `blacklist`/`banned_ips` tables
//! instead of the old `blacklist.json`/`banned_ips.json` blobs.
//!
//! Both blobs are migrated once on startup
//! (`DataStore::backfill_blacklist_table_from_json`/
//! `backfill_banned_ips_table_from_json`) and no longer written. Lookups by
//! email try both the normalized form and the raw lowercased form, since
//! legacy blob entries were keyed inconsistently (some normalized, some
//! not) and existing rows are carried over as-is rather than rekeyed.

use crate::data::DataStore;
use serde_json::{json, Value};

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Bans (or re-bans, updating reason/by) one account.
pub fn blacklist_insert(
    store: &DataStore,
    email: &str,
    reason: &str,
    banned_by: &str,
) -> rusqlite::Result<()> {
    let conn = store.conn();
    conn.execute(
        "INSERT INTO blacklist (email, reason, banned_at, banned_by) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(email) DO UPDATE SET reason = ?2, banned_at = ?3, banned_by = ?4",
        rusqlite::params![
            crate::auth::normalize_email(email),
            reason,
            now_millis(),
            banned_by
        ],
    )?;
    Ok(())
}

fn matching_keys(conn: &rusqlite::Connection, email: &str) -> rusqlite::Result<Vec<String>> {
    let canonical = crate::auth::normalize_email(email);
    let mut stmt = conn.prepare("SELECT email FROM blacklist")?;
    let keys = stmt.query_map([], |r| r.get::<_, String>(0))?;
    keys.filter_map(|key| match key {
        Ok(key) if crate::auth::normalize_email(&key) == canonical => Some(Ok(key)),
        Ok(_) => None,
        Err(err) => Some(Err(err)),
    })
    .collect()
}

/// Remove all legacy spellings of the same account in one transaction.
pub fn blacklist_remove(
    store: &DataStore,
    norm_email: &str,
    _raw_email: &str,
) -> rusqlite::Result<bool> {
    let conn = store.conn();
    let keys = matching_keys(&conn, norm_email)?;
    let tx = conn.unchecked_transaction()?;
    for key in &keys {
        tx.execute("DELETE FROM blacklist WHERE email = ?1", [key])?;
    }
    tx.commit()?;
    Ok(!keys.is_empty())
}

/// `{ "reason": ..., "by": ... }` for the given email, or `None` if not
/// banned. Tries the normalized form first, then the raw lowercased form.
pub fn blacklist_get(store: &DataStore, norm_email: &str, raw_email: &str) -> Option<Value> {
    let conn = store.conn();
    let lookup = |key: &str| -> Option<Value> {
        conn.query_row(
            "SELECT reason, banned_by, banned_at FROM blacklist WHERE email = ?1",
            rusqlite::params![key],
            |r| {
                Ok(json!({
                    "reason": r.get::<_, String>(0)?,
                    "by": r.get::<_, String>(1)?,
                    "banned_at": r.get::<_, i64>(2)?,
                }))
            },
        )
        .ok()
    };
    lookup(norm_email)
        .or_else(|| lookup(raw_email))
        .or_else(|| {
            matching_keys(&conn, norm_email)
                .ok()?
                .iter()
                .find_map(|key| lookup(key))
        })
}

/// One row per banned account, for the admin dashboard's full listing.
pub fn blacklist_all(store: &DataStore) -> Vec<Value> {
    let conn = store.conn();
    let mut stmt = match conn.prepare(
        "SELECT email, reason, banned_at, banned_by FROM blacklist ORDER BY banned_at DESC",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map([], |r| {
        Ok(json!({
            "email": r.get::<_, String>(0)?,
            "reason": r.get::<_, String>(1)?,
            "bannedAt": r.get::<_, i64>(2)?,
            "by": r.get::<_, String>(3)?,
        }))
    });
    match rows {
        Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Bans (or re-bans) one IP, optionally associated with the account whose
/// ban triggered it.
pub fn banned_ips_insert(store: &DataStore, ip: &str, reason: &str, banned_by: &str, email: &str) {
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO banned_ips (ip, reason, banned_at, banned_by, email) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(ip) DO UPDATE SET reason = ?2, banned_at = ?3, banned_by = ?4, email = ?5",
        rusqlite::params![ip, reason, now_millis(), banned_by, email],
    );
}

pub fn banned_ips_remove(store: &DataStore, ip: &str) -> bool {
    let conn = store.conn();
    conn.execute(
        "DELETE FROM banned_ips WHERE ip = ?1",
        rusqlite::params![ip],
    )
    .unwrap_or(0)
        > 0
}

/// Remove account-linked restrictions while preserving independent network bans.
pub fn banned_ips_remove_for_account(
    store: &DataStore,
    norm_email: &str,
    raw_email: &str,
    _last_known_ip: Option<&str>,
) -> Vec<String> {
    let conn = store.conn();
    let mut stmt = match conn.prepare("SELECT ip, email FROM banned_ips") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows: Vec<(String, String)> =
        match stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => return Vec::new(),
        };
    let to_remove: Vec<String> = rows
        .into_iter()
        .filter(|(_, email)| {
            !email.is_empty()
                && (crate::auth::normalize_email(email) == crate::auth::normalize_email(norm_email)
                    || email == raw_email)
        })
        .map(|(ip, _)| ip)
        .collect();
    for ip in &to_remove {
        let _ = conn.execute(
            "DELETE FROM banned_ips WHERE ip = ?1",
            rusqlite::params![ip],
        );
    }
    to_remove
}

/// `{ "reason": ..., "by": ... }` for the given IP, or `None` if not banned.
pub fn banned_ips_get(store: &DataStore, ip: &str) -> Option<Value> {
    let conn = store.conn();
    conn.query_row(
        "SELECT reason, banned_by, email FROM banned_ips WHERE ip = ?1",
        rusqlite::params![ip],
        |r| {
            Ok(json!({
                "reason": r.get::<_, String>(0)?,
                "by": r.get::<_, String>(1)?,
                "email": r.get::<_, String>(2)?,
            }))
        },
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::DataStore;

    fn temp_store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-bans-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn blacklist_insert_then_get_then_remove() {
        let store = temp_store("basic");
        assert!(blacklist_get(&store, "person@gmail.com", "person@gmail.com").is_none());
        blacklist_insert(&store, "person@gmail.com", "spamming", "admin@mitch.pro").unwrap();
        let entry = blacklist_get(&store, "person@gmail.com", "person@gmail.com").unwrap();
        assert_eq!(entry.get("reason").unwrap(), "spamming");
        assert_eq!(entry.get("by").unwrap(), "admin@mitch.pro");
        assert!(blacklist_remove(&store, "person@gmail.com", "person@gmail.com").unwrap());
        assert!(blacklist_get(&store, "person@gmail.com", "person@gmail.com").is_none());
    }

    #[test]
    fn blacklist_get_falls_back_to_raw_email() {
        let store = temp_store("raw-fallback");
        // Simulates a legacy row keyed by the raw (non-normalized) address.
        store.conn().execute("INSERT INTO blacklist (email, reason, banned_at, banned_by) VALUES (?1, 'legacy entry', 1, 'admin')", ["Person@Gmail.com"]).unwrap();
        let entry = blacklist_get(&store, "person@gmail.com", "Person@Gmail.com").unwrap();
        assert_eq!(entry.get("reason").unwrap(), "legacy entry");
    }

    #[test]
    fn legacy_aliases_cannot_bypass_a_ban_or_survive_an_unban() {
        let store = temp_store("aliases");
        for email in [
            "First.Last+old@student.mitch.pro",
            "FIRST.LAST@student.rjuhsd.us",
        ] {
            store.conn().execute("INSERT INTO blacklist (email, reason, banned_at, banned_by) VALUES (?1, 'legacy', 1, 'admin')", [email]).unwrap();
        }
        blacklist_insert(&store, "unrelated@example.com", "keep", "admin").unwrap();
        assert!(
            crate::auth::banned_info_for_email(&store, "firstlast@student.rjuhsd.us").is_some()
        );
        assert!(blacklist_remove(
            &store,
            "firstlast@student.rjuhsd.us",
            "firstlast@student.rjuhsd.us"
        )
        .unwrap());
        assert!(
            crate::auth::banned_info_for_email(&store, "First.Last@student.mitch.pro").is_none()
        );
        assert_eq!(blacklist_all(&store).len(), 1);
        assert!(crate::auth::banned_info_for_email(&store, "unrelated@example.com").is_some());
    }

    #[test]
    fn account_linked_ips_do_not_block_other_people_on_the_network() {
        let store = temp_store("shared-network");
        banned_ips_insert(
            &store,
            "192.0.2.1",
            "old automatic ban",
            "admin",
            "person@example.com",
        );
        assert!(crate::auth::banned_info_for_ip(&store, "192.0.2.1").is_none());
        banned_ips_insert(&store, "192.0.2.2", "explicit network ban", "admin", "");
        assert!(crate::auth::banned_info_for_ip(&store, "192.0.2.2").is_some());
    }

    #[test]
    fn failed_database_writes_are_reported() {
        let store = temp_store("write-failure");
        store.conn().execute("DROP TABLE blacklist", []).unwrap();
        assert!(blacklist_insert(&store, "person@example.com", "reason", "admin").is_err());
        assert!(blacklist_remove(&store, "person@example.com", "person@example.com").is_err());
    }

    #[test]
    fn banned_ips_insert_then_get_then_remove() {
        let store = temp_store("ip-basic");
        banned_ips_insert(
            &store,
            "1.2.3.4",
            "abuse",
            "admin@mitch.pro",
            "person@gmail.com",
        );
        let entry = banned_ips_get(&store, "1.2.3.4").unwrap();
        assert_eq!(entry.get("reason").unwrap(), "abuse");
        assert!(banned_ips_remove(&store, "1.2.3.4"));
        assert!(banned_ips_get(&store, "1.2.3.4").is_none());
    }

    #[test]
    fn account_restore_preserves_unrelated_network_bans() {
        let store = temp_store("ip-sweep");
        banned_ips_insert(&store, "1.1.1.1", "r1", "admin", "person@gmail.com");
        banned_ips_insert(&store, "2.2.2.2", "r2", "admin", "");
        banned_ips_insert(&store, "3.3.3.3", "r3", "admin", "other@gmail.com");
        let removed = banned_ips_remove_for_account(
            &store,
            "person@gmail.com",
            "person@gmail.com",
            Some("2.2.2.2"),
        );
        assert_eq!(removed.len(), 1);
        assert!(banned_ips_get(&store, "1.1.1.1").is_none());
        assert!(banned_ips_get(&store, "2.2.2.2").is_some());
        assert!(banned_ips_get(&store, "3.3.3.3").is_some());
    }
}
