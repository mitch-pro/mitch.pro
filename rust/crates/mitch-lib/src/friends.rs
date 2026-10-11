//! Friends-list storage — real `friends` table (one row per normalized
//! email, value is a JSON array of friend emails) instead of the old
//! friends.json blob.

use crate::data::DataStore;
use serde_json::{json, Value};

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The stored friend-email array for a user, or `[]` if they have none —
/// matches the old blob's `friends[norm] || []` access pattern.
pub fn friends_get(store: &DataStore, norm_email: &str) -> Value {
    let conn = store.conn();
    let raw: Option<String> = conn
        .query_row(
            "SELECT data FROM friend_lists WHERE email = ?1",
            rusqlite::params![norm_email],
            |r| r.get(0),
        )
        .ok();
    raw.and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!([]))
}

/// Overwrites the friend-email array for a user.
pub fn friends_set(store: &DataStore, norm_email: &str, data: &Value) {
    let data_str = serde_json::to_string(data).unwrap_or_else(|_| "[]".to_string());
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO friend_lists (email, data, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(email) DO UPDATE SET data = ?2, updated_at = ?3",
        rusqlite::params![norm_email, data_str, now_millis()],
    );
}

/// The full `{email: [friendEmail, ...]}` map, for call sites that used
/// to load the whole friends.json blob once and index into it many
/// times (the members/friends list builders, the `are_friends` checks).
pub fn friends_all(store: &DataStore) -> Value {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare("SELECT email, data FROM friend_lists") else {
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
        let value = serde_json::from_str(&data).unwrap_or_else(|_| json!([]));
        map.insert(email, value);
    }
    Value::Object(map)
}

/// `friends[key] ||= []; if (!includes(x)) push(x)` (server.js:16110-16115).
pub fn friends_add(store: &DataStore, norm_email: &str, friend_email: &str) {
    let mut list = friends_get(store, norm_email);
    let Some(arr) = list.as_array_mut() else {
        return;
    };
    if !arr.iter().any(|v| v.as_str() == Some(friend_email)) {
        arr.push(json!(friend_email));
    }
    friends_set(store, norm_email, &list);
}

/// Removes one entry (by exact string match) from a user's friend list.
pub fn friends_remove_one(store: &DataStore, norm_email: &str, friend_email: &str) {
    let mut list = friends_get(store, norm_email);
    let Some(arr) = list.as_array_mut() else {
        return;
    };
    arr.retain(|v| v.as_str() != Some(friend_email));
    friends_set(store, norm_email, &list);
}

/// Moves a user's own friend-list row to their new email, then rewrites
/// every OTHER user's list, replacing any entry that normalizes to the
/// old email with the new one — the old whole-blob rename did both
/// (server.js's renameEmailReferences moved the key, then remapped every
/// list entry pointing at the renamed account).
pub fn rename_friend_references(store: &DataStore, old_norm: &str, new_norm: &str) {
    let list = friends_get(store, old_norm);
    if list != json!([]) {
        friends_set(store, new_norm, &list);
        let conn = store.conn();
        let _ = conn.execute(
            "DELETE FROM friend_lists WHERE email = ?1",
            rusqlite::params![old_norm],
        );
    }
    let all = friends_all(store);
    let Some(map) = all.as_object() else { return };
    for (email, value) in map {
        if email == new_norm {
            continue;
        }
        let Some(arr) = value.as_array() else {
            continue;
        };
        if !arr.iter().any(|f| {
            f.as_str()
                .is_some_and(|s| crate::auth::normalize_email(s) == old_norm)
        }) {
            continue;
        }
        let mut updated = value.clone();
        if let Some(arr) = updated.as_array_mut() {
            for f in arr.iter_mut() {
                if let Some(s) = f.as_str() {
                    if crate::auth::normalize_email(s) == old_norm {
                        *f = json!(new_norm);
                    }
                }
            }
        }
        friends_set(store, email, &updated);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-friends-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn get_defaults_to_empty_array() {
        let store = temp_store("default");
        assert_eq!(friends_get(&store, "a@gmail.com"), json!([]));
    }

    #[test]
    fn add_is_idempotent() {
        let store = temp_store("add");
        friends_add(&store, "a@gmail.com", "b@gmail.com");
        friends_add(&store, "a@gmail.com", "b@gmail.com");
        assert_eq!(friends_get(&store, "a@gmail.com"), json!(["b@gmail.com"]));
    }

    #[test]
    fn remove_one_drops_just_that_entry() {
        let store = temp_store("remove");
        friends_add(&store, "a@gmail.com", "b@gmail.com");
        friends_add(&store, "a@gmail.com", "c@gmail.com");
        friends_remove_one(&store, "a@gmail.com", "b@gmail.com");
        assert_eq!(friends_get(&store, "a@gmail.com"), json!(["c@gmail.com"]));
    }

    #[test]
    fn all_lists_every_row() {
        let store = temp_store("all");
        friends_add(&store, "a@gmail.com", "b@gmail.com");
        friends_add(&store, "b@gmail.com", "a@gmail.com");
        let all = friends_all(&store);
        assert_eq!(all.get("a@gmail.com"), Some(&json!(["b@gmail.com"])));
        assert_eq!(all.get("b@gmail.com"), Some(&json!(["a@gmail.com"])));
    }

    #[test]
    fn rename_moves_row_and_remaps_other_lists() {
        let store = temp_store("rename");
        friends_add(&store, "old@gmail.com", "b@gmail.com");
        friends_add(&store, "b@gmail.com", "old@gmail.com");
        rename_friend_references(&store, "old@gmail.com", "new@gmail.com");
        assert_eq!(friends_get(&store, "old@gmail.com"), json!([]));
        assert_eq!(friends_get(&store, "new@gmail.com"), json!(["b@gmail.com"]));
        assert_eq!(friends_get(&store, "b@gmail.com"), json!(["new@gmail.com"]));
    }
}
