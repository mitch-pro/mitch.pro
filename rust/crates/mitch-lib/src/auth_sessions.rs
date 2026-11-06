//! Login sessions — the `auth_sessions` table (formerly the
//! `auth_sessions.json` blob). The `mitch_session` cookie holds a random
//! token; only `sha256(token)` hex is stored, as `token_hash`.
//!
//! A row holds exactly what validating a session needs: the account, the
//! derived sid, the session generation (bumped to log an account out
//! everywhere), creation and expiry. No IP, user agent or last-used time
//! (docs/privacy.md). Expired rows are deleted lazily on lookup and in bulk
//! whenever a session is created.

use crate::data::DataStore;
use rusqlite::OptionalExtension;
use serde_json::Value;

/// One stored session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub token_hash: String,
    pub email: String,
    pub norm_email: String,
    pub sid: String,
    pub gen: i64,
    pub created_at: i64,
    pub expires_at: i64,
    pub dev_superuser: bool,
}

pub fn insert(store: &DataStore, row: &SessionRow) {
    let conn = store.conn();
    let _ = conn.execute(
        "DELETE FROM auth_sessions WHERE expires_at < ?1",
        rusqlite::params![row.created_at],
    );
    let _ = conn.execute(
        "INSERT OR REPLACE INTO auth_sessions
            (token_hash, email, norm_email, sid, gen, created_at, expires_at, dev_superuser)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            row.token_hash,
            row.email,
            row.norm_email,
            row.sid,
            row.gen,
            row.created_at,
            row.expires_at,
            row.dev_superuser as i64
        ],
    );
}

pub fn get(store: &DataStore, token_hash: &str) -> Option<SessionRow> {
    let conn = store.conn();
    conn.query_row(
        "SELECT token_hash, email, norm_email, sid, gen, created_at, expires_at, dev_superuser
           FROM auth_sessions WHERE token_hash = ?1",
        rusqlite::params![token_hash],
        |r| {
            Ok(SessionRow {
                token_hash: r.get(0)?,
                email: r.get(1)?,
                norm_email: r.get(2)?,
                sid: r.get(3)?,
                gen: r.get(4)?,
                created_at: r.get(5)?,
                expires_at: r.get(6)?,
                dev_superuser: r.get::<_, i64>(7)? != 0,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

pub fn delete(store: &DataStore, token_hash: &str) -> bool {
    let conn = store.conn();
    conn.execute(
        "DELETE FROM auth_sessions WHERE token_hash = ?1",
        rusqlite::params![token_hash],
    )
    .unwrap_or(0)
        > 0
}

pub fn set_sid(store: &DataStore, token_hash: &str, sid: &str) {
    let conn = store.conn();
    let _ = conn.execute(
        "UPDATE auth_sessions SET sid = ?2 WHERE token_hash = ?1",
        rusqlite::params![token_hash, sid],
    );
}

/// Deletes every session for `norm_email` except `keep_hash`.
pub fn delete_for_email(store: &DataStore, norm_email: &str, keep_hash: &str) -> usize {
    let conn = store.conn();
    conn.execute(
        "DELETE FROM auth_sessions WHERE norm_email = ?1 AND token_hash != ?2",
        rusqlite::params![norm_email, keep_hash],
    )
    .unwrap_or(0)
}

/// Startup migration: imports `auth_sessions.json` (keyed by token hash) into
/// the table, keeping only the fields above, then deletes the document.
/// Expired entries are dropped. Idempotent. Returns the number imported.
pub fn migrate_from_json(store: &DataStore) -> usize {
    let file = store.base_dir.join("data/auth_sessions.json");
    let legacy = store.read_document(&file, Value::Null);
    let mut imported = 0;
    if let Some(map) = legacy.as_object() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let conn = store.conn();
        for (key, rec) in map {
            let text = |k: &str| rec.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let num = |k: &str| rec.get(k).and_then(Value::as_i64).unwrap_or(0);
            let expires_at = num("expiresAt");
            let email = text("email");
            let norm = {
                let n = text("normEmail");
                crate::auth::normalize_email(if n.is_empty() { &email } else { &n })
            };
            if key.is_empty() || norm.is_empty() || expires_at <= now {
                continue;
            }
            if conn
                .execute(
                    "INSERT OR IGNORE INTO auth_sessions
                        (token_hash, email, norm_email, sid, gen, created_at, expires_at, dev_superuser)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![
                        key,
                        email,
                        norm,
                        text("sid"),
                        num("gen"),
                        num("createdAt"),
                        expires_at,
                        rec.get("devSuperuser").and_then(Value::as_bool).unwrap_or(false) as i64
                    ],
                )
                .unwrap_or(0)
                > 0
            {
                imported += 1;
            }
        }
    }
    crate::privacy::delete_document(store, &file);
    imported
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_store(tag: &str) -> (std::path::PathBuf, DataStore) {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-auth-sessions-{tag}-{}",
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
    fn json_sessions_move_to_the_table_without_extra_fields() {
        let (base, store) = temp_store("migrate");
        let far = 9_999_999_999_999_i64;
        store
            .write_document(
                &base.join("data/auth_sessions.json"),
                &json!({
                    "live": {"email": "A@b.c", "normEmail": "a@b.c", "sid": "s", "gen": 2,
                             "createdAt": 1, "expiresAt": far, "ip": "198.51.100.1",
                             "userAgent": "UA", "lastSeen": 5},
                    "old": {"email": "a@b.c", "sid": "s", "gen": 0, "createdAt": 1, "expiresAt": 2}
                }),
            )
            .unwrap();
        assert_eq!(migrate_from_json(&store), 1);
        assert!(store
            .read_document(&base.join("data/auth_sessions.json"), Value::Null)
            .is_null());
        let row = get(&store, "live").unwrap();
        assert_eq!(row.norm_email, "a@b.c");
        assert_eq!(row.gen, 2);
        assert_eq!(row.expires_at, far);
        assert!(get(&store, "old").is_none());
        assert_eq!(migrate_from_json(&store), 0);
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn delete_for_email_keeps_the_current_session() {
        let (base, store) = temp_store("delete");
        for key in ["a", "b"] {
            insert(
                &store,
                &SessionRow {
                    token_hash: key.into(),
                    email: "x@y.z".into(),
                    norm_email: "x@y.z".into(),
                    sid: "s".into(),
                    gen: 0,
                    created_at: 1,
                    expires_at: i64::MAX,
                    dev_superuser: false,
                },
            );
        }
        assert_eq!(delete_for_email(&store, "x@y.z", "a"), 1);
        assert!(get(&store, "a").is_some());
        assert!(get(&store, "b").is_none());
        std::fs::remove_dir_all(base).ok();
    }
}
