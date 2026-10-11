//! Profile storage — real `profiles` table (one row per normalized email)
//! instead of the old profiles.json blob.
//!
//! Profile fields (username, displayName, bio, twofa settings, ...) are
//! read/written as a flexible per-user object, same shape as
//! `cosmetics`/`user_stats`/`push_subs` — see `profile.rs` for the actual
//! business logic built on top of this storage layer.

use crate::data::DataStore;
use serde_json::{json, Value};

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The stored profile object for a user, or `None` if they have none yet —
/// matches the old blob's `profiles[norm]` (an `Option`, not `|| {}`) so
/// callers can tell "no row" apart from "an empty object was stored".
pub fn profile_get_opt(store: &DataStore, norm_email: &str) -> Option<Value> {
    let conn = store.conn();
    let raw: Option<String> = conn
        .query_row(
            "SELECT data FROM profiles WHERE email = ?1",
            rusqlite::params![norm_email],
            |r| r.get(0),
        )
        .ok();
    raw.and_then(|s| serde_json::from_str(&s).ok())
}

/// The stored profile object for a user, or `{}` if they have none yet —
/// matches the old blob's `profiles[norm] || {}` access pattern.
pub fn profile_get(store: &DataStore, norm_email: &str) -> Value {
    profile_get_opt(store, norm_email).unwrap_or_else(|| json!({}))
}

/// Overwrites the profile object for a user.
pub fn profile_set(store: &DataStore, norm_email: &str, data: &Value) {
    let data_str = serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string());
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO profiles (email, data, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(email) DO UPDATE SET data = ?2, updated_at = ?3",
        rusqlite::params![norm_email, data_str, now_millis()],
    );
}

/// The full `{email: profileObject}` map, for call sites that used to load
/// the whole profiles.json blob once and index into it many times.
pub fn profile_all(store: &DataStore) -> Value {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare("SELECT email, data FROM profiles") else {
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

/// Every email with a profile row.
pub fn profile_all_emails(store: &DataStore) -> Vec<String> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare("SELECT email FROM profiles") else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) else {
        return Vec::new();
    };
    rows.filter_map(|r| r.ok()).collect()
}

/// Moves a profile row to a new email key, also updating the embedded
/// `email` field (the old whole-blob rename did the same thing only for
/// profiles.json, since it's the one record that carries its own address).
pub fn rename_profile_email(store: &DataStore, old_norm: &str, new_norm: &str, new_email: &str) {
    let mut entry = profile_get(store, old_norm);
    if entry == json!({}) {
        return;
    }
    if let Some(obj) = entry.as_object_mut() {
        obj.insert("email".to_string(), json!(new_email));
    }
    profile_set(store, new_norm, &entry);
    let conn = store.conn();
    let _ = conn.execute(
        "DELETE FROM profiles WHERE email = ?1",
        rusqlite::params![old_norm],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-profiles-test-{tag}-{}",
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
        assert_eq!(profile_get(&store, "a@gmail.com"), json!({}));
    }

    #[test]
    fn get_set_round_trip() {
        let store = temp_store("round-trip");
        let data = json!({"username": "a", "displayName": "A"});
        profile_set(&store, "a@gmail.com", &data);
        assert_eq!(profile_get(&store, "a@gmail.com"), data);
    }

    #[test]
    fn all_lists_every_row() {
        let store = temp_store("all");
        profile_set(&store, "a@gmail.com", &json!({"username": "a"}));
        profile_set(&store, "b@gmail.com", &json!({"username": "b"}));
        let all = profile_all(&store);
        assert_eq!(all.get("a@gmail.com"), Some(&json!({"username": "a"})));
        assert_eq!(all.get("b@gmail.com"), Some(&json!({"username": "b"})));
    }

    #[test]
    fn rename_moves_the_row_and_updates_embedded_email() {
        let store = temp_store("rename");
        profile_set(
            &store,
            "old@gmail.com",
            &json!({"username": "a", "email": "old@gmail.com"}),
        );
        rename_profile_email(&store, "old@gmail.com", "new@gmail.com", "new@gmail.com");
        assert_eq!(profile_get(&store, "old@gmail.com"), json!({}));
        assert_eq!(
            profile_get(&store, "new@gmail.com"),
            json!({"username": "a", "email": "new@gmail.com"})
        );
    }

    #[test]
    fn rename_is_a_noop_when_no_profile_exists() {
        let store = temp_store("rename-noop");
        rename_profile_email(&store, "old@gmail.com", "new@gmail.com", "new@gmail.com");
        assert_eq!(profile_get(&store, "new@gmail.com"), json!({}));
    }
}
