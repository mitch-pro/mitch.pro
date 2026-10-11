//! Web Push subscription registry — real `push_subs` table (one row per
//! normalized email) instead of the old push_subs.json blob.
//!
//! Every write path already normalizes the email before saving; the only
//! reason callers used to also try the raw (non-normalized) key was to
//! find legacy rows written before that was consistent. The backfill
//! normalizes those once (see `DataStore::backfill_push_subs_from_json`),
//! so every lookup here is a single normalized-key read.

use crate::data::DataStore;
use serde_json::Value;

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn push_sub_get(store: &DataStore, norm_email: &str) -> Option<Value> {
    let conn = store.conn();
    let raw: Option<String> = conn
        .query_row(
            "SELECT data FROM push_subs WHERE email = ?1",
            rusqlite::params![norm_email],
            |r| r.get(0),
        )
        .ok();
    raw.and_then(|s| serde_json::from_str(&s).ok())
}

pub fn push_sub_set(store: &DataStore, norm_email: &str, data: &Value) {
    let data_str = serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string());
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO push_subs (email, data, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(email) DO UPDATE SET data = ?2, updated_at = ?3",
        rusqlite::params![norm_email, data_str, now_millis()],
    );
}

/// Removes the subscription for an email (tolerates being handed a raw,
/// non-normalized email — normalizes before deleting).
pub fn push_sub_remove(store: &DataStore, email: &str) -> bool {
    let norm = crate::auth::normalize_email(email);
    let conn = store.conn();
    conn.execute(
        "DELETE FROM push_subs WHERE email = ?1",
        rusqlite::params![norm],
    )
    .unwrap_or(0)
        > 0
}

pub fn rename_push_sub_email(store: &DataStore, old_norm: &str, new_norm: &str) {
    let conn = store.conn();
    let _ = conn.execute(
        "DELETE FROM push_subs WHERE email = ?1",
        rusqlite::params![new_norm],
    );
    let _ = conn.execute(
        "UPDATE push_subs SET email = ?1 WHERE email = ?2",
        rusqlite::params![new_norm, old_norm],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-push-subs-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn get_set_remove_round_trip() {
        let store = temp_store("round-trip");
        assert_eq!(push_sub_get(&store, "a@gmail.com"), None);
        let sub =
            json!({"endpoint": "https://example.com/x", "keys": {"p256dh": "a", "auth": "b"}});
        push_sub_set(&store, "a@gmail.com", &sub);
        assert_eq!(push_sub_get(&store, "a@gmail.com"), Some(sub));
        assert!(push_sub_remove(&store, "a@gmail.com"));
        assert_eq!(push_sub_get(&store, "a@gmail.com"), None);
    }

    #[test]
    fn remove_normalizes_a_raw_email() {
        let store = temp_store("remove-raw");
        push_sub_set(&store, "ab@student.rjuhsd.us", &json!({"endpoint": "x"}));
        assert!(push_sub_remove(&store, "A.B+x@mitch.pro"));
        assert_eq!(push_sub_get(&store, "ab@student.rjuhsd.us"), None);
    }

    #[test]
    fn rename_moves_the_row() {
        let store = temp_store("rename");
        push_sub_set(&store, "old@gmail.com", &json!({"endpoint": "x"}));
        rename_push_sub_email(&store, "old@gmail.com", "new@gmail.com");
        assert_eq!(push_sub_get(&store, "old@gmail.com"), None);
        assert_eq!(
            push_sub_get(&store, "new@gmail.com"),
            Some(json!({"endpoint": "x"}))
        );
    }
}
