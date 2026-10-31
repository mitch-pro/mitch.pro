//! Usernames the owner has reserved so nobody can claim them.
//!
//! Stored in the `reserved_usernames` table (one row per normalized username).
//! A row may name a `holder_email`: that one account may use the name (so the
//! owner can reserve a name for themselves); with no holder, nobody can.
//!
//! Enforced wherever a username is chosen: signup (`routes/auth.rs`), profile
//! saves and default-username generation (`profile::ensure_profile_defaults`).
//! An account that already holds a name keeps it when the name is reserved
//! later; reserving only blocks new claims.

use crate::data::DataStore;

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// One reserved name, as listed in the admin panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservedUsername {
    pub username: String,
    /// Normalized email allowed to use the name, or empty for nobody.
    pub holder_email: String,
    pub reserved_by: String,
    pub reserved_at: i64,
}

/// Adds (or replaces) a reservation. `username` must already be valid and
/// normalized by the caller. Returns false on a database error.
pub fn reserve(store: &DataStore, username: &str, holder_email: &str, reserved_by: &str) -> bool {
    let holder = if holder_email.trim().is_empty() {
        String::new()
    } else {
        crate::auth::normalize_email(holder_email)
    };
    store
        .conn()
        .execute(
            "INSERT INTO reserved_usernames (username, holder_email, reserved_by, reserved_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(username) DO UPDATE SET holder_email = ?2, reserved_by = ?3, reserved_at = ?4",
            rusqlite::params![username, holder, reserved_by, now_millis()],
        )
        .is_ok()
}

/// Removes a reservation; true when one existed.
pub fn release(store: &DataStore, username: &str) -> bool {
    store
        .conn()
        .execute(
            "DELETE FROM reserved_usernames WHERE username = ?1",
            rusqlite::params![username],
        )
        .unwrap_or(0)
        > 0
}

/// All reservations, alphabetical.
pub fn list(store: &DataStore) -> Vec<ReservedUsername> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(
        "SELECT username, holder_email, reserved_by, reserved_at FROM reserved_usernames ORDER BY username",
    ) else {
        return Vec::new();
    };
    let rows = stmt.query_map([], |r| {
        Ok(ReservedUsername {
            username: r.get(0)?,
            holder_email: r.get(1)?,
            reserved_by: r.get(2)?,
            reserved_at: r.get(3)?,
        })
    });
    match rows {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// True when `username` is reserved and `email` isn't its holder. `email` is
/// the account trying to take the name.
pub fn is_blocked_for(store: &DataStore, username: &str, email: &str) -> bool {
    let holder: Option<String> = store
        .conn()
        .query_row(
            "SELECT holder_email FROM reserved_usernames WHERE username = ?1",
            rusqlite::params![username],
            |r| r.get(0),
        )
        .ok();
    match holder {
        None => false,
        Some(h) => h.is_empty() || h != crate::auth::normalize_email(email),
    }
}

/// Every reserved name, for callers that dedupe against a set (default
/// username generation). Holders are not considered here.
pub fn all_names(store: &DataStore) -> Vec<String> {
    list(store).into_iter().map(|r| r.username).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-reserved-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn reserved_names_block_everyone_except_the_holder() {
        let store = temp_store("block");
        assert!(!is_blocked_for(&store, "mitch", "a@x.test"));
        assert!(reserve(&store, "mitch", "", "owner@x.test"));
        assert!(is_blocked_for(&store, "mitch", "a@x.test"));
        assert!(reserve(&store, "mitch", "Owner@X.test", "owner@x.test"));
        assert!(!is_blocked_for(&store, "mitch", "owner@x.test"));
        assert!(is_blocked_for(&store, "mitch", "a@x.test"));
        assert_eq!(list(&store).len(), 1);
        assert!(release(&store, "mitch"));
        assert!(!release(&store, "mitch"));
        assert!(!is_blocked_for(&store, "mitch", "a@x.test"));
    }

    #[test]
    fn profile_defaults_skip_reserved_names_but_keep_current_holders() {
        use serde_json::json;
        let store = temp_store("profile");
        let dir = std::env::temp_dir();
        let claim = |email: &str, name: &str| {
            crate::profile::ensure_profile_defaults(
                &store,
                &dir,
                b"secret",
                email,
                email,
                &json!({ "username": name }),
            )["username"]
                .as_str()
                .unwrap_or("")
                .to_string()
        };
        // An existing holder, then the name gets reserved for someone else.
        assert_eq!(claim("holder@x.test", "taken"), "taken");
        assert!(reserve(&store, "taken", "owner@x.test", "owner@x.test"));
        assert!(reserve(&store, "reserved", "", "owner@x.test"));
        assert_eq!(claim("holder@x.test", "taken"), "taken");
        assert_ne!(claim("other@x.test", "reserved"), "reserved");
        assert_ne!(claim("third@x.test", "taken"), "taken");
        assert!(reserve(&store, "vip", "owner@x.test", "owner@x.test"));
        assert_eq!(claim("owner@x.test", "vip"), "vip");
    }
}
