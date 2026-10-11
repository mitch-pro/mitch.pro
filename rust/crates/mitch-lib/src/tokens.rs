//! Auth/invite token storage — real `tokens` table (one row per token)
//! instead of the old tokens.json blob. Covers signup-claim tokens,
//! password-reset tokens (+ their 6-digit OTP fallback), and email-change
//! confirmation tokens — all keyed by a random token string rather than
//! by email, unlike every other migrated store.

use crate::data::DataStore;
use serde_json::{json, Value};

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The stored record for a token, or `None` if it doesn't exist.
pub fn token_get(store: &DataStore, token: &str) -> Option<Value> {
    let conn = store.conn();
    let raw: Option<String> = conn
        .query_row(
            "SELECT data FROM tokens WHERE token = ?1",
            rusqlite::params![token],
            |r| r.get(0),
        )
        .ok();
    raw.and_then(|s| serde_json::from_str(&s).ok())
}

/// Inserts or overwrites a token's record.
pub fn token_set(store: &DataStore, token: &str, data: &Value) {
    let data_str = serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string());
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO tokens (token, data, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(token) DO UPDATE SET data = ?2, updated_at = ?3",
        rusqlite::params![token, data_str, now_millis()],
    );
}

pub fn token_remove(store: &DataStore, token: &str) -> bool {
    let conn = store.conn();
    conn.execute(
        "DELETE FROM tokens WHERE token = ?1",
        rusqlite::params![token],
    )
    .unwrap_or(0)
        > 0
}

/// The full `{token: record}` map, for call sites that used to load the
/// whole tokens.json blob once and scan it (e.g. the OTP fallback lookup,
/// enrolled-user listings, the email-rename sweep).
pub fn token_all(store: &DataStore) -> Value {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare("SELECT token, data FROM tokens") else {
        return json!({});
    };
    let Ok(rows) = stmt.query_map([], |r| {
        let token: String = r.get(0)?;
        let data: String = r.get(1)?;
        Ok((token, data))
    }) else {
        return json!({});
    };
    let mut map = serde_json::Map::new();
    for (token, data) in rows.filter_map(|r| r.ok()) {
        let value = serde_json::from_str(&data).unwrap_or_else(|_| json!({}));
        map.insert(token, value);
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-tokens-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn get_missing_is_none() {
        let store = temp_store("missing");
        assert_eq!(token_get(&store, "tok1"), None);
    }

    #[test]
    fn get_set_round_trip() {
        let store = temp_store("round-trip");
        let data = json!({"email": "a@gmail.com", "type": "reset"});
        token_set(&store, "tok1", &data);
        assert_eq!(token_get(&store, "tok1"), Some(data));
    }

    #[test]
    fn remove_deletes_the_row() {
        let store = temp_store("remove");
        token_set(&store, "tok1", &json!({"a": 1}));
        assert!(token_remove(&store, "tok1"));
        assert_eq!(token_get(&store, "tok1"), None);
        assert!(!token_remove(&store, "tok1"));
    }

    #[test]
    fn all_lists_every_row() {
        let store = temp_store("all");
        token_set(&store, "tok1", &json!({"a": 1}));
        token_set(&store, "tok2", &json!({"a": 2}));
        let all = token_all(&store);
        assert_eq!(all.get("tok1"), Some(&json!({"a": 1})));
        assert_eq!(all.get("tok2"), Some(&json!({"a": 2})));
    }
}
