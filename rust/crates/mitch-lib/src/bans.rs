//! Account and IP ban data layer — real `blacklist`/`banned_ips` tables
//! instead of the old `blacklist.json`/`banned_ips.json` blobs.
//!
//! Both blobs are migrated once on startup
//! (`DataStore::backfill_blacklist_table_from_json`/
//! `backfill_banned_ips_table_from_json`) and no longer written. Lookups by
//! email try both the normalized form and the raw lowercased form, since
//! legacy blob entries were keyed inconsistently (some normalized, some
//! not) and existing rows are carried over as-is rather than rekeyed.
//!
//! IP bans are explicit network bans only. The `banned_ips.ip` column holds
//! [`ip_ban_key`] — a keyed HMAC of the address, never the raw IP — so the
//! table can answer "is this address banned?" without keeping a list of
//! addresses. Rows that older code wrote alongside an account ban (non-empty
//! `email`) were never enforced and are deleted on startup by
//! `crate::privacy::run_startup_cleanup`, which also rehashes any raw rows.

use crate::data::DataStore;
use rusqlite::OptionalExtension;
use serde_json::{json, Value};

/// Signed identity marker used only to deny access, never to authenticate.
pub fn signed_ban_marker(email: &str, secret: &[u8]) -> String {
    use base64::Engine;
    let email = crate::auth::normalize_email(email);
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(email);
    let signature = crate::crypto::hmac_sha256(secret, format!("account-ban:{payload}").as_bytes());
    format!(
        "{payload}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature)
    )
}

pub fn verify_ban_marker(value: &str, secret: &[u8]) -> Option<String> {
    use base64::Engine;
    use hmac::{Hmac, Mac};
    let (payload, signature) = value.split_once('.')?;
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(signature)
        .ok()?;
    let mut mac = <Hmac<sha2::Sha256> as Mac>::new_from_slice(secret).ok()?;
    mac.update(format!("account-ban:{payload}").as_bytes());
    mac.verify_slice(&signature).ok()?;
    let email = String::from_utf8(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .ok()?,
    )
    .ok()?;
    Some(crate::auth::normalize_email(&email))
}

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
    let snapshots = crate::chat_cleanup::snapshots(store);
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
    crate::chat_cleanup::purge_transaction(&tx, email, now_millis(), snapshots)?;
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

/// The stored form of an IP in `banned_ips.ip`: `h:` + HMAC-SHA256 hex of the
/// address under the server's ID secret. Equality lookups still work; the
/// raw address cannot be read back out of the table.
pub fn ip_ban_key(secret: &[u8], ip: &str) -> String {
    format!(
        "h:{}",
        crate::crypto::hmac_sha256_hex(secret, format!("banned-ip:{}", ip.trim()).as_bytes())
    )
}

/// Bans (or re-bans) one network address.
pub fn banned_ips_insert(
    store: &DataStore,
    secret: &[u8],
    ip: &str,
    reason: &str,
    banned_by: &str,
) {
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO banned_ips (ip, reason, banned_at, banned_by, email) VALUES (?1, ?2, ?3, ?4, '')
         ON CONFLICT(ip) DO UPDATE SET reason = ?2, banned_at = ?3, banned_by = ?4, email = ''",
        rusqlite::params![ip_ban_key(secret, ip), reason, now_millis(), banned_by],
    );
}

pub fn banned_ips_remove(store: &DataStore, secret: &[u8], ip: &str) -> bool {
    let conn = store.conn();
    conn.execute(
        "DELETE FROM banned_ips WHERE ip = ?1",
        rusqlite::params![ip_ban_key(secret, ip)],
    )
    .unwrap_or(0)
        > 0
}

/// `{ "reason": ..., "by": ... }` for the given IP, or `None` if not banned.
pub fn banned_ips_get(store: &DataStore, secret: &[u8], ip: &str) -> Option<Value> {
    let conn = store.conn();
    conn.query_row(
        "SELECT reason, banned_by FROM banned_ips WHERE ip = ?1 AND email = ''",
        rusqlite::params![ip_ban_key(secret, ip)],
        |r| {
            Ok(json!({
                "reason": r.get::<_, String>(0)?,
                "by": r.get::<_, String>(1)?,
            }))
        },
    )
    .ok()
}

/// Startup migration: deletes account-linked rows (never enforced) and
/// replaces any raw-IP keys with [`ip_ban_key`]. Idempotent. Returns
/// `(deleted, rehashed)`.
pub fn hash_legacy_banned_ips(store: &DataStore, secret: &[u8]) -> (usize, usize) {
    let conn = store.conn();
    let deleted = conn
        .execute("DELETE FROM banned_ips WHERE email != ''", [])
        .unwrap_or(0);
    let raw: Vec<String> = conn
        .prepare("SELECT ip FROM banned_ips WHERE ip NOT LIKE 'h:%'")
        .and_then(|mut stmt| {
            stmt.query_map([], |r| r.get::<_, String>(0))
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
        })
        .unwrap_or_default();
    let mut rehashed = 0;
    for ip in raw {
        let key = ip_ban_key(secret, &ip);
        // Keep an already-hashed row for the same address; drop the raw one.
        let _ = conn.execute(
            "UPDATE OR IGNORE banned_ips SET ip = ?1 WHERE ip = ?2",
            rusqlite::params![key, ip],
        );
        let _ = conn.execute(
            "DELETE FROM banned_ips WHERE ip = ?1",
            rusqlite::params![ip],
        );
        rehashed += 1;
    }
    (deleted, rehashed)
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
    fn ban_markers_reject_forged_identity_or_signature() {
        let marker = signed_ban_marker("First.Last+alias@student.mitch.pro", b"secret");
        assert_eq!(
            verify_ban_marker(&marker, b"secret").as_deref(),
            Some("firstlast@student.rjuhsd.us")
        );
        assert!(verify_ban_marker(&marker, b"different secret").is_none());
        let signature = marker.split_once('.').unwrap().1;
        assert!(
            verify_ban_marker(&format!("Zm9yZ2VkQGV4YW1wbGUuY29t.{signature}"), b"secret")
                .is_none()
        );
        assert!(verify_ban_marker("invalid", b"secret").is_none());
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

    const SECRET: &[u8] = b"test-secret";

    #[test]
    fn legacy_ip_rows_are_hashed_and_account_linked_rows_deleted() {
        let store = temp_store("ip-migrate");
        {
            let conn = store.conn();
            conn.execute(
                "INSERT INTO banned_ips (ip, reason, banned_at, banned_by, email) VALUES
                 ('192.0.2.1', 'old automatic ban', 1, 'admin', 'person@example.com'),
                 ('192.0.2.2', 'explicit network ban', 1, 'admin', '')",
                [],
            )
            .unwrap();
        }
        assert_eq!(hash_legacy_banned_ips(&store, SECRET), (1, 1));
        assert_eq!(hash_legacy_banned_ips(&store, SECRET), (0, 0));
        let keys: Vec<String> = store
            .conn()
            .prepare("SELECT ip FROM banned_ips")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(keys, vec![ip_ban_key(SECRET, "192.0.2.2")]);
        assert!(!keys[0].contains("192.0.2"));
        assert!(crate::auth::banned_info_for_ip(&store, SECRET, "192.0.2.2").is_some());
        assert!(crate::auth::banned_info_for_ip(&store, SECRET, "192.0.2.1").is_none());
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
        banned_ips_insert(&store, SECRET, "1.2.3.4", "abuse", "admin@mitch.pro");
        let entry = banned_ips_get(&store, SECRET, "1.2.3.4").unwrap();
        assert_eq!(entry.get("reason").unwrap(), "abuse");
        let stored: String = store
            .conn()
            .query_row("SELECT ip FROM banned_ips", [], |r| r.get(0))
            .unwrap();
        assert!(!stored.contains("1.2.3.4"));
        assert!(banned_ips_get(&store, b"other-secret", "1.2.3.4").is_none());
        assert!(banned_ips_remove(&store, SECRET, "1.2.3.4"));
        assert!(banned_ips_get(&store, SECRET, "1.2.3.4").is_none());
    }
}
