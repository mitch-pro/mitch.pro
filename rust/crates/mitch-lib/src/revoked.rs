//! Revoked session-id storage — real `revoked_ids` table (one row per
//! sid) instead of the old revoked.json blob. Values are stored as-is:
//! some entries are a plain `true`, others an object carrying an `email`
//! field (from the admin-dashboard revoke flow) — every read site here
//! only ever cared about key presence or that `email` field, never a
//! fixed schema, so the blob-per-row shape preserves that exactly.

use crate::data::DataStore;
use serde_json::{json, Value};

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `isRevoked(id)` (server.js:2927-2930) — key presence only, regardless
/// of the stored value.
pub fn revoked_id_exists(store: &DataStore, sid: &str) -> bool {
    if sid.is_empty() {
        return false;
    }
    let conn = store.conn();
    conn.query_row(
        "SELECT 1 FROM revoked_ids WHERE sid = ?1",
        rusqlite::params![sid],
        |_| Ok(()),
    )
    .is_ok()
}

/// The stored value for a sid, or `None` if it isn't revoked.
pub fn revoked_id_get(store: &DataStore, sid: &str) -> Option<Value> {
    let conn = store.conn();
    let raw: Option<String> = conn
        .query_row(
            "SELECT data FROM revoked_ids WHERE sid = ?1",
            rusqlite::params![sid],
            |r| r.get(0),
        )
        .ok();
    raw.map(|s| serde_json::from_str(&s).unwrap_or(Value::Null))
}

pub fn revoked_id_set(store: &DataStore, sid: &str, data: &Value) {
    let data_str = serde_json::to_string(data).unwrap_or_else(|_| "null".to_string());
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO revoked_ids (sid, data, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(sid) DO UPDATE SET data = ?2, updated_at = ?3",
        rusqlite::params![sid, data_str, now_millis()],
    );
}

pub fn revoked_id_remove(store: &DataStore, sid: &str) -> bool {
    let conn = store.conn();
    conn.execute(
        "DELETE FROM revoked_ids WHERE sid = ?1",
        rusqlite::params![sid],
    )
    .unwrap_or(0)
        > 0
}

/// The full `{sid: value}` map, for call sites that used to load the
/// whole revoked.json blob once and scan it (e.g. the appeal-approve
/// cleanup, which removes every entry matching an email).
pub fn revoked_all(store: &DataStore) -> Value {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare("SELECT sid, data FROM revoked_ids") else {
        return json!({});
    };
    let Ok(rows) = stmt.query_map([], |r| {
        let sid: String = r.get(0)?;
        let data: String = r.get(1)?;
        Ok((sid, data))
    }) else {
        return json!({});
    };
    let mut map = serde_json::Map::new();
    for (sid, data) in rows.filter_map(|r| r.ok()) {
        let value = serde_json::from_str(&data).unwrap_or(Value::Null);
        map.insert(sid, value);
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-revoked-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn empty_sid_never_revoked() {
        let store = temp_store("empty");
        assert!(!revoked_id_exists(&store, ""));
    }

    #[test]
    fn exists_and_get_round_trip() {
        let store = temp_store("round-trip");
        assert!(!revoked_id_exists(&store, "sid1"));
        assert_eq!(revoked_id_get(&store, "sid1"), None);
        revoked_id_set(&store, "sid1", &json!(true));
        assert!(revoked_id_exists(&store, "sid1"));
        assert_eq!(revoked_id_get(&store, "sid1"), Some(json!(true)));
    }

    #[test]
    fn object_values_round_trip() {
        let store = temp_store("object");
        revoked_id_set(&store, "sid1", &json!({"email": "a@gmail.com"}));
        assert!(revoked_id_exists(&store, "sid1"));
        assert_eq!(
            revoked_id_get(&store, "sid1"),
            Some(json!({"email": "a@gmail.com"}))
        );
    }

    #[test]
    fn remove_deletes_the_row() {
        let store = temp_store("remove");
        revoked_id_set(&store, "sid1", &json!(true));
        assert!(revoked_id_remove(&store, "sid1"));
        assert!(!revoked_id_exists(&store, "sid1"));
        assert!(!revoked_id_remove(&store, "sid1"));
    }

    #[test]
    fn all_lists_every_row() {
        let store = temp_store("all");
        revoked_id_set(&store, "sid1", &json!(true));
        revoked_id_set(&store, "sid2", &json!({"email": "a@gmail.com"}));
        let all = revoked_all(&store);
        assert_eq!(all.get("sid1"), Some(&json!(true)));
        assert_eq!(all.get("sid2"), Some(&json!({"email": "a@gmail.com"})));
    }
}
