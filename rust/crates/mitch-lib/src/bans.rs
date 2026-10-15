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
use rusqlite::OptionalExtension;
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
    blacklist_insert_until(store, email, reason, banned_by, None)
}

/// Replace an account's ban, including legacy aliases. None means permanent.
pub fn blacklist_insert_until(
    store: &DataStore,
    email: &str,
    reason: &str,
    banned_by: &str,
    expires_at: Option<i64>,
) -> rusqlite::Result<()> {
    let conn = store.conn();
    let keys = matching_keys(&conn, email)?;
    let tx = conn.unchecked_transaction()?;
    for key in keys {
        tx.execute("DELETE FROM blacklist WHERE email = ?1", [key])?;
    }
    tx.execute(
        "INSERT INTO blacklist (email, reason, banned_at, banned_by, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![crate::auth::normalize_email(email), reason, now_millis(), banned_by, expires_at],
    )?;
    tx.commit()
}

/// Missing duration keeps older admin clients' permanent-ban behavior.
pub fn duration_millis(value: Option<&Value>) -> Result<Option<i64>, &'static str> {
    let duration = match value {
        None => "permanent",
        Some(value) => value.as_str().ok_or("Choose a valid ban duration.")?,
    };
    let hours = match duration {
        "1h" => 1,
        "1d" => 24,
        "3d" => 72,
        "7d" => 168,
        "14d" => 336,
        "permanent" => return Ok(None),
        _ => return Err("Choose a valid ban duration."),
    };
    Ok(Some(hours * 60 * 60 * 1000))
}

pub fn required_reason(reason: &str) -> Result<String, &'static str> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err("Enter a reason for the ban.");
    }
    if reason.chars().count() > 200 {
        return Err("Keep the ban reason to 200 characters or fewer.");
    }
    Ok(reason.to_string())
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
            "SELECT reason, banned_by, banned_at, expires_at FROM blacklist WHERE email = ?1 AND (expires_at IS NULL OR expires_at > ?2)",
            rusqlite::params![key, now_millis()],
            |r| {
                Ok(json!({
                    "reason": r.get::<_, String>(0)?,
                    "by": r.get::<_, String>(1)?,
                    "banned_at": r.get::<_, i64>(2)?,
                    "expires_at": r.get::<_, Option<i64>>(3)?,
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
        "SELECT email, reason, banned_at, banned_by, expires_at FROM blacklist WHERE expires_at IS NULL OR expires_at > ?1 ORDER BY banned_at DESC",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map([now_millis()], |r| {
        Ok(json!({
            "email": r.get::<_, String>(0)?,
            "reason": r.get::<_, String>(1)?,
            "bannedAt": r.get::<_, i64>(2)?,
            "by": r.get::<_, String>(3)?,
            "expiresAt": r.get::<_, Option<i64>>(4)?,
        }))
    });
    match rows {
        Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Keep one pending appeal per account, with atomic updates across requests.
pub fn submit_appeal(
    store: &DataStore,
    data_dir: &std::path::Path,
    email: &str,
    reason: &str,
) -> rusqlite::Result<()> {
    let path = data_dir.join("appeals.json");
    let key = store.relative_key(&path);
    let fallback = store.read_document(&path, json!([]));
    let conn = store.conn();
    let tx = conn.unchecked_transaction()?;
    let content: Option<String> = tx
        .query_row(
            "SELECT content FROM json_documents WHERE path = ?1",
            [&key],
            |row| row.get(0),
        )
        .optional()?;
    let mut appeals = content
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or(fallback)
        .as_array()
        .cloned()
        .unwrap_or_default();
    let email = crate::auth::normalize_email(email);
    if !appeals.iter().any(|appeal| {
        appeal
            .get("email")
            .and_then(Value::as_str)
            .is_some_and(|value| crate::auth::normalize_email(value) == email)
    }) {
        let now = now_millis();
        appeals.push(json!({ "email":email, "reason":reason, "submitted_at":now }));
        tx.execute("INSERT INTO json_documents (path, content, updated_at) VALUES (?1, ?2, ?3)
            ON CONFLICT(path) DO UPDATE SET content = excluded.content, updated_at = excluded.updated_at",
            rusqlite::params![key, json!(appeals).to_string(), now])?;
    }
    tx.commit()
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
    fn temporary_bans_expire_and_replace_legacy_permanent_aliases() {
        let store = temp_store("expiry");
        let email = "firstlast@student.rjuhsd.us";
        store.conn().execute("INSERT INTO blacklist (email, reason, banned_at, banned_by) VALUES ('First.Last@student.mitch.pro', 'legacy', 1, 'admin')", []).unwrap();
        let expiry = now_millis() + 86_400_000;
        blacklist_insert_until(&store, email, "Spam", "admin", Some(expiry)).unwrap();
        let info = crate::auth::banned_info_for_email(&store, "First.Last+test@student.mitch.pro")
            .unwrap();
        assert_eq!(info["expires_at"], expiry);
        assert_eq!(blacklist_all(&store).len(), 1);
        store
            .conn()
            .execute("UPDATE blacklist SET expires_at = ?1", [now_millis() - 1])
            .unwrap();
        assert!(crate::auth::banned_info_for_email(&store, email).is_none());
        assert!(blacklist_all(&store).is_empty());
        blacklist_insert(&store, email, "Permanent", "admin").unwrap();
        assert!(crate::auth::banned_info_for_email(&store, email).unwrap()["expires_at"].is_null());
    }

    #[test]
    fn legacy_document_does_not_reinstate_expired_or_removed_bans_on_restart() {
        let store = temp_store("legacy-restart");
        let base = store.base_dir.clone();
        store
            .write_document(
                &base.join("data/blacklist.json"),
                &json!({
                    "First.Last@student.mitch.pro":{"reason":"Legacy ban"},
                    "removed@example.com":{"reason":"Legacy ban"}
                }),
            )
            .unwrap();
        store.conn().execute("DROP TABLE blacklist", []).unwrap();
        drop(store);
        let store = DataStore::open(&base, &base.join("data")).unwrap();
        assert_eq!(blacklist_all(&store).len(), 2);
        blacklist_insert_until(
            &store,
            "firstlast@student.rjuhsd.us",
            "Temporary",
            "admin",
            Some(1),
        )
        .unwrap();
        blacklist_remove(&store, "removed@example.com", "removed@example.com").unwrap();
        drop(store);
        let store = DataStore::open(&base, &base.join("data")).unwrap();
        assert!(blacklist_all(&store).is_empty());
        assert!(
            crate::auth::banned_info_for_email(&store, "First.Last@student.mitch.pro").is_none()
        );
        assert!(crate::auth::banned_info_for_email(&store, "removed@example.com").is_none());
    }

    #[test]
    fn old_database_migration_keeps_permanent_bans() {
        let store = temp_store("migration");
        let base = store.base_dir.clone();
        blacklist_insert(&store, "student@example.com", "Existing ban", "admin").unwrap();
        store
            .conn()
            .execute("ALTER TABLE blacklist DROP COLUMN expires_at", [])
            .unwrap();
        drop(store);
        let store = DataStore::open(&base, &base.join("data")).unwrap();
        let info = crate::auth::banned_info_for_email(&store, "student@example.com").unwrap();
        assert_eq!(info["reason"], "Existing ban");
        assert!(info["expires_at"].is_null());
    }

    #[test]
    fn appeals_deduplicate_account_aliases_and_keep_other_accounts() {
        let store = temp_store("appeals");
        let data = store.base_dir.join("data");
        submit_appeal(
            &store,
            &data,
            "First.Last@student.mitch.pro",
            "Review my ban",
        )
        .unwrap();
        submit_appeal(
            &store,
            &data,
            "firstlast+alias@student.rjuhsd.us",
            "Duplicate",
        )
        .unwrap();
        submit_appeal(&store, &data, "other@example.com", "Another appeal").unwrap();
        let appeals = store.read_document(&data.join("appeals.json"), json!([]));
        assert_eq!(appeals.as_array().unwrap().len(), 2);
        assert_eq!(appeals[0]["reason"], "Review my ban");
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
