//! Permanent message removal recorded atomically with each new account ban.
use crate::{auth::normalize_email, data::DataStore};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::HashMap;

const DOCUMENTS: &[(&str, bool)] = &[
    ("dms.json", true),
    ("pickle_dms.json", true),
    ("e2e_attachments.json", false),
    ("gif_favorites.json", false),
    ("matrix_users.json", false),
];

pub fn removals(conn: &Connection) -> rusqlite::Result<HashMap<String, i64>> {
    let mut stmt = conn.prepare("SELECT email, cutoff FROM account_chat_removals")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    rows.collect()
}

fn old_entry(entry: &Value, field: &str, cutoffs: &HashMap<String, i64>) -> bool {
    let author = normalize_email(entry.get(field).and_then(Value::as_str).unwrap_or(""));
    let ts = entry
        .get("ts")
        .or_else(|| entry.get("createdAt"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    cutoffs.get(&author).is_some_and(|cutoff| ts <= *cutoff)
}

pub fn filter_document(conn: &Connection, key: &str, value: &mut Value) {
    let name = key.rsplit('/').next().unwrap_or(key);
    if !DOCUMENTS.iter().any(|(doc, _)| *doc == name) {
        return;
    }
    let Ok(cutoffs) = removals(conn) else {
        return;
    };
    match name {
        "dms.json" | "pickle_dms.json" => {
            if let Some(list) = value.as_array_mut() {
                list.retain(|e| !old_entry(e, "from", &cutoffs));
            }
        }
        "e2e_attachments.json" => {
            if let Some(map) = value.as_object_mut() {
                map.retain(|_, e| !old_entry(e, "user", &cutoffs));
            }
        }
        "gif_favorites.json" => {
            if let Some(map) = value.as_object_mut() {
                map.retain(|email, _| !conn.query_row("SELECT EXISTS(SELECT 1 FROM blacklist WHERE email = ?1 AND (expires_at IS NULL OR expires_at > ?2))", rusqlite::params![normalize_email(email), crate::school::now_millis()], |r| r.get::<_, bool>(0)).unwrap_or(false));
            }
        }
        _ => {}
    }
}

pub fn snapshots(store: &DataStore) -> Vec<(String, Value)> {
    DOCUMENTS
        .iter()
        .map(|(name, list)| {
            let file = store.data_dir().join(name);
            (
                store.relative_key(&file),
                store.read_document(&file, if *list { json!([]) } else { json!({}) }),
            )
        })
        .collect()
}

pub fn purge_transaction(
    tx: &Connection,
    email: &str,
    cutoff: i64,
    snapshots: Vec<(String, Value)>,
) -> rusqlite::Result<()> {
    let email = normalize_email(email);
    tx.execute("INSERT INTO account_chat_removals(email,cutoff) VALUES(?1,?2) ON CONFLICT(email) DO UPDATE SET cutoff=MAX(cutoff,excluded.cutoff), matrix_done=0, last_error=''", rusqlite::params![email, cutoff])?;
    tx.execute(
        "UPDATE moderated_media SET blocked=1 WHERE email=?1 AND created_at<=?2",
        rusqlite::params![email, cutoff],
    )?;
    tx.execute(
        "UPDATE account_chat_removal_ids SET cutoff=MAX(cutoff,?2) WHERE email=?1",
        rusqlite::params![email, cutoff],
    )?;
    for (key, fallback) in snapshots {
        let raw: Option<String> = tx
            .query_row(
                "SELECT content FROM json_documents WHERE path=?1",
                [&key],
                |r| r.get(0),
            )
            .optional()?;
        let mut value = raw
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(fallback);
        if key.ends_with("matrix_users.json") {
            if let Some(map) = value.as_object() {
                for (key, name) in map {
                    let owner = if key.contains('@') && !key.starts_with('@') {
                        Some(normalize_email(key))
                    } else {
                        tx.query_row("SELECT email FROM names WHERE sid=?1", [key], |r| {
                            r.get::<_, String>(0)
                        })
                        .optional()?
                        .map(|e| normalize_email(&e))
                    };
                    if owner.as_deref() == Some(&email) {
                        if let Some(name) = name.as_str().filter(|n| !n.is_empty()) {
                            let id = if name.starts_with('@') {
                                name.to_string()
                            } else {
                                format!("@{name}:mitch.pro")
                            };
                            tx.execute("INSERT INTO account_chat_removal_ids(email,user_id,cutoff) VALUES(?1,?2,?3) ON CONFLICT(email,user_id) DO UPDATE SET cutoff=MAX(cutoff,excluded.cutoff)",rusqlite::params![email,id,cutoff])?;
                        }
                    }
                }
            }
            continue;
        }
        if key.ends_with("e2e_attachments.json") {
            if let Some(entries) = value.as_object() {
                let cutoffs = removals(tx)?;
                for entry in entries.values().filter(|e| old_entry(e, "user", &cutoffs)) {
                    if let Some(file) = entry
                        .get("file")
                        .and_then(Value::as_str)
                        .filter(|f| safe_filename(f))
                    {
                        tx.execute(
                            "INSERT OR IGNORE INTO chat_files_to_remove(file) VALUES(?1)",
                            [file],
                        )?;
                    }
                }
            }
        }
        filter_document(tx, &key, &mut value);
        // Favorites are account-owned and removed even when a very short ban expires during cleanup.
        if key.ends_with("gif_favorites.json") {
            if let Some(map) = value.as_object_mut() {
                map.retain(|key, _| normalize_email(key) != email);
            }
        }
        tx.execute("INSERT INTO json_documents(path,content,updated_at) VALUES(?1,?2,?3) ON CONFLICT(path) DO UPDATE SET content=excluded.content,updated_at=excluded.updated_at", rusqlite::params![key,value.to_string(),cutoff])?;
    }
    Ok(())
}

pub fn safe_filename(file: &str) -> bool {
    !file.is_empty()
        && file != "."
        && file != ".."
        && file
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

pub fn remove_files(store: &DataStore) -> rusqlite::Result<()> {
    let files: Vec<String> = {
        let conn = store.conn();
        let mut stmt = conn.prepare("SELECT file FROM chat_files_to_remove")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    for file in files {
        if !safe_filename(&file) {
            continue;
        }
        let path = store.data_dir().join("e2e_attachments").join(&file);
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => continue,
        }
        store
            .conn()
            .execute("DELETE FROM chat_files_to_remove WHERE file=?1", [file])?;
    }
    Ok(())
}

pub fn remember_media(store: &DataStore, uri: &str, email: &str, ts: i64, blocked: bool) {
    if !uri.starts_with("mxc://") {
        return;
    }
    let _=store.conn().execute("INSERT INTO moderated_media(uri,email,created_at,blocked) VALUES(?1,?2,?3,?4) ON CONFLICT(uri) DO UPDATE SET blocked=MAX(blocked,excluded.blocked)",rusqlite::params![uri,normalize_email(email),ts,blocked]);
}

pub fn media_blocked(store: &DataStore, uri: &str) -> bool {
    store.conn().query_row("SELECT EXISTS(SELECT 1 FROM moderated_media m LEFT JOIN account_chat_removals r ON m.email=r.email WHERE m.uri=?1 AND (m.blocked=1 OR m.created_at<=r.cutoff))",[uri],|r|r.get(0)).unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ban_removes_only_authored_history_and_cannot_be_undone_by_stale_writer() {
        let dir = std::env::temp_dir().join(format!(
            "chat-cleanup-{}",
            crate::crypto::random_bytes_hex(8)
        ));
        std::fs::create_dir_all(dir.join("data/e2e_attachments")).unwrap();
        let store = DataStore::open(&dir, &dir.join("data")).unwrap();
        let path = dir.join("data/dms.json");
        let old = json!([{ "from":"Student@Example.com", "ts":1, "image":"a.gif" }, {"from":"neighbor@example.com","ts":1,"text":"keep"}]);
        store.write_document(&path, &old).unwrap();
        store.write_document(&dir.join("data/e2e_attachments.json"),&json!({"a":{"user":"student@example.com","createdAt":1,"file":"a.gif"},"b":{"user":"neighbor@example.com","file":"b.gif"},"bad":{"user":"student@example.com","file":"../outside"}})).unwrap();
        std::fs::write(dir.join("data/e2e_attachments/a.gif"), b"gif").unwrap();
        std::fs::write(dir.join("data/e2e_attachments/b.gif"), b"keep").unwrap();
        store
            .write_document(
                &dir.join("data/matrix_users.json"),
                &json!({"student@example.com":"old_handle"}),
            )
            .unwrap();
        crate::bans::blacklist_insert(&store, "student@example.com", "reason", "admin").unwrap();
        store
            .write_document(
                &dir.join("data/matrix_users.json"),
                &json!({"student@example.com":"new_handle"}),
            )
            .unwrap();
        assert!(identity_cutoffs(&store).contains_key("@old_handle:mitch.pro"));
        store.write_document(&path, &old).unwrap();
        assert_eq!(
            store
                .read_document(&path, json!([]))
                .as_array()
                .unwrap()
                .len(),
            1
        );
        remove_files(&store).unwrap();
        assert!(!dir.join("data/e2e_attachments/a.gif").exists());
        assert!(dir.join("data/e2e_attachments/b.gif").exists());
        crate::bans::blacklist_remove(&store, "student@example.com", "student@example.com")
            .unwrap();
        let mut messages = old.as_array().unwrap().clone();
        messages.push(
            json!({"from":"student@example.com","ts":crate::school::now_millis()+10,"text":"new"}),
        );
        store.write_document(&path, &json!(messages)).unwrap();
        assert_eq!(
            store
                .read_document(&path, json!([]))
                .as_array()
                .unwrap()
                .len(),
            2
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

pub fn account_removals(store: &DataStore) -> HashMap<String, i64> {
    removals(&store.conn()).unwrap_or_default()
}
pub fn pending_jobs(store: &DataStore) -> Vec<(String, i64)> {
    let conn = store.conn();
    let Ok(mut stmt) =
        conn.prepare("SELECT email,cutoff FROM account_chat_removals WHERE matrix_done=0")
    else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok).collect()
}
pub fn finish_job(store: &DataStore, email: &str, cutoff: i64, done: bool, error: &str) {
    let _=store.conn().execute("UPDATE account_chat_removals SET matrix_done=?1,last_error=?2 WHERE email=?3 AND cutoff=?4",rusqlite::params![done,error,email,cutoff]);
}
pub fn statuses(store: &DataStore) -> Value {
    let conn = store.conn();
    let Ok(mut stmt) =
        conn.prepare("SELECT email,matrix_done,last_error FROM account_chat_removals")
    else {
        return json!({});
    };
    let Ok(rows) = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, bool>(1)?,
            r.get::<_, String>(2)?,
        ))
    }) else {
        return json!({});
    };
    let mut statuses = serde_json::Map::new();
    for (email, done, error) in rows.filter_map(Result::ok) {
        statuses.insert(email, json!({"complete":done,"error":error}));
    }
    Value::Object(statuses)
}

pub fn identity_cutoffs(store: &DataStore) -> HashMap<String, (String, i64)> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare("SELECT user_id,email,cutoff FROM account_chat_removal_ids")
    else {
        return HashMap::new();
    };
    let Ok(rows) = stmt.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?)))) else {
        return HashMap::new();
    };
    rows.filter_map(Result::ok).collect()
}
pub fn remember_identities(store: &DataStore, email: &str, cutoff: i64, ids: &[String]) {
    let conn = store.conn();
    for id in ids {
        let _=conn.execute("INSERT INTO account_chat_removal_ids(email,user_id,cutoff) VALUES(?1,?2,?3) ON CONFLICT(email,user_id) DO UPDATE SET cutoff=MAX(cutoff,excluded.cutoff)",rusqlite::params![email,id,cutoff]);
    }
}
