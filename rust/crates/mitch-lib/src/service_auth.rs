//! Short-lived, one-use service authorization codes and scoped token revocations.
//! SQLite consumes codes atomically, including across blue/green processes.
use crate::data::{DataError, DataStore};
use serde_json::{json, Value};

fn prune(store: &DataStore, now: i64) -> Result<(), DataError> {
    store.conn().execute(
        "DELETE FROM json_documents WHERE path LIKE 'woodcreek-auth/%' AND json_extract(content, '$.expires') <= ?1",
        [now],
    )?;
    Ok(())
}

pub fn save_code(
    store: &DataStore,
    hash: &str,
    challenge: &str,
    token: &str,
    expires: i64,
    now: i64,
) -> Result<(), DataError> {
    prune(store, now)?;
    store.conn().execute(
        "INSERT INTO json_documents(path,content,updated_at) VALUES(?1,?2,?3)",
        rusqlite::params![
            format!("woodcreek-auth/code/{hash}"),
            json!({"challenge":challenge,"token":token,"expires":expires}).to_string(),
            now
        ],
    )?;
    Ok(())
}

pub fn take_code(store: &DataStore, hash: &str, challenge: &str, now: i64) -> Option<String> {
    let raw: String = store.conn().query_row(
        "DELETE FROM json_documents WHERE path=?1 AND json_extract(content,'$.challenge')=?2 AND json_extract(content,'$.expires')>?3 RETURNING content",
        rusqlite::params![format!("woodcreek-auth/code/{hash}"), challenge, now], |row| row.get(0),
    ).ok()?;
    serde_json::from_str::<Value>(&raw)
        .ok()?
        .get("token")?
        .as_str()
        .map(str::to_owned)
}

pub fn revoke(store: &DataStore, hash: &str, expires: i64, now: i64) -> Result<(), DataError> {
    prune(store, now)?;
    store.conn().execute(
        "INSERT OR REPLACE INTO json_documents(path,content,updated_at) VALUES(?1,?2,?3)",
        rusqlite::params![
            format!("woodcreek-auth/revoked/{hash}"),
            json!({"expires":expires}).to_string(),
            now
        ],
    )?;
    Ok(())
}

pub fn is_revoked(store: &DataStore, hash: &str, now: i64) -> bool {
    store.conn().query_row(
        "SELECT EXISTS(SELECT 1 FROM json_documents WHERE path=?1 AND json_extract(content,'$.expires')>?2)",
        rusqlite::params![format!("woodcreek-auth/revoked/{hash}"), now], |row| row.get::<_, bool>(0),
    ).unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codes_are_one_use_expire_and_survive_reopening() {
        let base = std::env::temp_dir().join(format!(
            "mitch-service-auth-{}",
            crate::crypto::random_bytes_hex(8)
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        let store = DataStore::open(&base, &base.join("data")).unwrap();
        save_code(&store, "code", "challenge", "grant", 200, 100).unwrap();
        assert!(take_code(&store, "code", "wrong", 101).is_none());
        drop(store);
        let store = DataStore::open(&base, &base.join("data")).unwrap();
        assert_eq!(
            take_code(&store, "code", "challenge", 101).as_deref(),
            Some("grant")
        );
        assert!(take_code(&store, "code", "challenge", 101).is_none());
        save_code(&store, "expired", "challenge", "grant", 200, 100).unwrap();
        assert!(take_code(&store, "expired", "challenge", 200).is_none());
        assert!(!is_revoked(&store, "grant", 100));
        revoke(&store, "grant", 300, 100).unwrap();
        assert!(is_revoked(&store, "grant", 101));
        assert!(!is_revoked(&store, "grant", 300));
        drop(store);
        std::fs::remove_dir_all(base).unwrap();
    }
}
