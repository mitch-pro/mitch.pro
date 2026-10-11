//! Per-user game/activity stats — real `user_stats` table (one row per
//! normalized email) instead of the old user_stats.json blob.
//!
//! The stat keys inside each entry are dynamic (per-game counters, VIP/
//! happy-hour expiry timestamps, playtime, premium bookkeeping, ...), so
//! unlike `achievements` this keeps the whole per-user record as a JSON
//! blob in `data` — same shape as `cosmetics`/`push_subs`. The win here
//! isn't a schema redesign, it's replacing "read the whole file's blob,
//! mutate one user's key, write the whole file back" with a single-row
//! read/write.

use crate::data::DataStore;
use serde_json::{json, Value};

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The stored stats object for a user, or `{}` if they have none yet —
/// matches the old blob's `stats[norm] || {}` access pattern.
pub fn user_stats_get(store: &DataStore, norm_email: &str) -> Value {
    let conn = store.conn();
    let raw: Option<String> = conn
        .query_row(
            "SELECT data FROM user_stats WHERE email = ?1",
            rusqlite::params![norm_email],
            |r| r.get(0),
        )
        .ok();
    raw.and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}))
}

/// Overwrites the stats object for a user.
pub fn user_stats_set(store: &DataStore, norm_email: &str, data: &Value) {
    let data_str = serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string());
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO user_stats (email, data, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(email) DO UPDATE SET data = ?2, updated_at = ?3",
        rusqlite::params![norm_email, data_str, now_millis()],
    );
}

/// The full `{email: statsObject}` map, for admin/dashboard listings that
/// used to read the whole user_stats.json blob at once.
pub fn user_stats_all(store: &DataStore) -> Value {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare("SELECT email, data FROM user_stats") else {
        return json!({});
    };
    let Ok(rows) = stmt.query_map([], |r| {
        let email: String = r.get(0)?;
        let data: String = r.get(1)?;
        Ok((email, data))
    }) else {
        return json!({});
    };
    let mut map = serde_json::Map::new();
    for (email, data) in rows.filter_map(|r| r.ok()) {
        let value = serde_json::from_str(&data).unwrap_or_else(|_| json!({}));
        map.insert(email, value);
    }
    Value::Object(map)
}

/// Every email with a stats row.
pub fn user_stats_all_emails(store: &DataStore) -> Vec<String> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare("SELECT email FROM user_stats") else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) else {
        return Vec::new();
    };
    rows.filter_map(|r| r.ok()).collect()
}

pub fn rename_user_stats_email(store: &DataStore, old_norm: &str, new_norm: &str) {
    let conn = store.conn();
    let _ = conn.execute(
        "DELETE FROM user_stats WHERE email = ?1",
        rusqlite::params![new_norm],
    );
    let _ = conn.execute(
        "UPDATE user_stats SET email = ?1 WHERE email = ?2",
        rusqlite::params![new_norm, old_norm],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-user-stats-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn get_defaults_to_empty_object() {
        let store = temp_store("default");
        assert_eq!(user_stats_get(&store, "a@gmail.com"), json!({}));
    }

    #[test]
    fn get_set_round_trip() {
        let store = temp_store("round-trip");
        let data = json!({"pixels": 42, "last_active_at": 1000});
        user_stats_set(&store, "a@gmail.com", &data);
        assert_eq!(user_stats_get(&store, "a@gmail.com"), data);
    }

    #[test]
    fn all_lists_every_row() {
        let store = temp_store("all");
        user_stats_set(&store, "a@gmail.com", &json!({"pixels": 1}));
        user_stats_set(&store, "b@gmail.com", &json!({"pixels": 2}));
        let all = user_stats_all(&store);
        assert_eq!(all.get("a@gmail.com"), Some(&json!({"pixels": 1})));
        assert_eq!(all.get("b@gmail.com"), Some(&json!({"pixels": 2})));
    }

    #[test]
    fn all_emails_lists_every_row() {
        let store = temp_store("all-emails");
        user_stats_set(&store, "a@gmail.com", &json!({}));
        user_stats_set(&store, "b@gmail.com", &json!({}));
        let mut emails = user_stats_all_emails(&store);
        emails.sort();
        assert_eq!(
            emails,
            vec!["a@gmail.com".to_string(), "b@gmail.com".to_string()]
        );
    }

    #[test]
    fn rename_moves_the_row() {
        let store = temp_store("rename");
        user_stats_set(&store, "old@gmail.com", &json!({"pixels": 7}));
        rename_user_stats_email(&store, "old@gmail.com", "new@gmail.com");
        assert_eq!(user_stats_get(&store, "old@gmail.com"), json!({}));
        assert_eq!(
            user_stats_get(&store, "new@gmail.com"),
            json!({"pixels": 7})
        );
    }
}
