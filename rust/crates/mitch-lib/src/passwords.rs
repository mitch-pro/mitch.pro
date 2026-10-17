//! Password hash storage — real `passwords` table (one row per email)
//! instead of the old passwords.json blob.

use crate::data::DataStore;

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The stored hash for an email, if any. This is an exact-match lookup,
/// same as the old blob's `Object.prototype.hasOwnProperty`-style access —
/// callers are responsible for normalizing the key first if that's what
/// they want to match against.
pub fn password_hash_get(store: &DataStore, email: &str) -> Option<String> {
    let conn = store.conn();
    conn.query_row(
        "SELECT hash FROM passwords WHERE email = ?1",
        rusqlite::params![email],
        |r| r.get(0),
    )
    .ok()
}

pub fn password_exists(store: &DataStore, email: &str) -> bool {
    password_hash_get(store, email).is_some()
}

pub fn password_set(store: &DataStore, email: &str, hash: &str) {
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO passwords (email, hash, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(email) DO UPDATE SET hash = ?2, updated_at = ?3",
        rusqlite::params![email, hash, now_millis()],
    );
}

/// Every email with a stored password hash.
pub fn password_all_emails(store: &DataStore) -> Vec<String> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare("SELECT email FROM passwords") else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) else {
        return Vec::new();
    };
    rows.filter_map(|r| r.ok()).collect()
}

pub fn password_remove(store: &DataStore, email: &str) -> bool {
    let conn = store.conn();
    conn.execute(
        "DELETE FROM passwords WHERE email = ?1",
        rusqlite::params![email],
    )
    .unwrap_or(0)
        > 0
}

pub fn rename_password_email(store: &DataStore, old_email: &str, new_email: &str) {
    let conn = store.conn();
    let _ = conn.execute(
        "DELETE FROM passwords WHERE email = ?1",
        rusqlite::params![new_email],
    );
    let _ = conn.execute(
        "UPDATE passwords SET email = ?1 WHERE email = ?2",
        rusqlite::params![new_email, old_email],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-passwords-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn get_set_round_trip() {
        let store = temp_store("round-trip");
        assert_eq!(password_hash_get(&store, "a@gmail.com"), None);
        assert!(!password_exists(&store, "a@gmail.com"));
        password_set(&store, "a@gmail.com", "hash1");
        assert_eq!(
            password_hash_get(&store, "a@gmail.com"),
            Some("hash1".to_string())
        );
        assert!(password_exists(&store, "a@gmail.com"));
    }

    #[test]
    fn set_overwrites_existing_hash() {
        let store = temp_store("overwrite");
        password_set(&store, "a@gmail.com", "hash1");
        password_set(&store, "a@gmail.com", "hash2");
        assert_eq!(
            password_hash_get(&store, "a@gmail.com"),
            Some("hash2".to_string())
        );
    }

    #[test]
    fn all_emails_lists_every_row() {
        let store = temp_store("all-emails");
        password_set(&store, "a@gmail.com", "hash1");
        password_set(&store, "b@gmail.com", "hash2");
        let mut emails = password_all_emails(&store);
        emails.sort();
        assert_eq!(
            emails,
            vec!["a@gmail.com".to_string(), "b@gmail.com".to_string()]
        );
    }

    #[test]
    fn rename_moves_the_row() {
        let store = temp_store("rename");
        password_set(&store, "old@gmail.com", "hash1");
        rename_password_email(&store, "old@gmail.com", "new@gmail.com");
        assert_eq!(password_hash_get(&store, "old@gmail.com"), None);
        assert_eq!(
            password_hash_get(&store, "new@gmail.com"),
            Some("hash1".to_string())
        );
    }
}
