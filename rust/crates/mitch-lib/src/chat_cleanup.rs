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

pub fn reset_status(conn: &Connection) -> Value {
    conn.query_row(
        "SELECT value FROM metadata WHERE key='chat_history_reset'",
        [],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .and_then(|s| serde_json::from_str(&s).ok())
    .unwrap_or(Value::Null)
}

pub fn global_cutoff(conn: &Connection) -> i64 {
    reset_status(conn)["cutoff"].as_i64().unwrap_or(0)
}

pub fn history_reset_cutoff(store: &DataStore) -> i64 {
    global_cutoff(&store.conn())
}
pub fn history_reset_status(store: &DataStore) -> Value {
    reset_status(&store.conn())
}
pub fn pending_file_count(store: &DataStore) -> i64 {
    store
        .conn()
        .query_row("SELECT COUNT(*) FROM chat_files_to_remove", [], |r| {
            r.get(0)
        })
        .unwrap_or(0)
}

/// A single transaction sets a durable cutoff and clears stored histories.
/// The request ID makes a retried request safe after an interrupted response.
pub fn reset_all(
    store: &DataStore,
    actor: &str,
    request_id: &str,
    cutoff: i64,
) -> rusqlite::Result<Value> {
    let snapshots = snapshots(store);
    let mut conn = store.conn();
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let request_key = format!("chat_history_reset_request:{request_id}");
    let recorded: Option<String> = tx
        .query_row(
            "SELECT value FROM metadata WHERE key=?1",
            [&request_key],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(recorded) = recorded {
        return serde_json::from_str(&recorded).map_err(|_| rusqlite::Error::InvalidQuery);
    }
    let previous = reset_status(&tx);
    if previous["requestId"].as_str() == Some(request_id) {
        return Ok(previous);
    }
    let cutoff = cutoff.max(global_cutoff(&tx));
    let mut messages = 0usize;
    let mut attachments = 0usize;
    let mut favorites = 0usize;
    let emails: Vec<String> = {
        let mut stmt = tx.prepare("SELECT DISTINCT email FROM names WHERE email<>''")?;
        let emails = stmt
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        emails
    };
    for email in emails {
        tx.execute("INSERT INTO account_chat_removals(email,cutoff) VALUES(?1,?2) ON CONFLICT(email) DO UPDATE SET cutoff=MAX(cutoff,excluded.cutoff),matrix_done=0,last_error=''",rusqlite::params![normalize_email(&email),cutoff])?;
    }
    tx.execute(
        "UPDATE moderated_media SET blocked=1 WHERE created_at<=?1",
        [cutoff],
    )?;
    for (key, fallback) in snapshots {
        let raw: Option<String> = tx
            .query_row(
                "SELECT content FROM json_documents WHERE path=?1",
                [&key],
                |r| r.get(0),
            )
            .optional()?;
        let value: Value = raw
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(fallback);
        if key.ends_with("matrix_users.json") {
            if let Some(map) = value.as_object() {
                for (key, name) in map {
                    let email = if key.contains('@') && !key.starts_with('@') {
                        Some(normalize_email(key))
                    } else {
                        tx.query_row("SELECT email FROM names WHERE sid=?1", [key], |r| {
                            r.get::<_, String>(0)
                        })
                        .optional()?
                    };
                    if let (Some(email), Some(name)) = (email, name.as_str()) {
                        let id = if name.starts_with('@') {
                            name.to_string()
                        } else {
                            format!("@{name}:mitch.pro")
                        };
                        tx.execute("INSERT INTO account_chat_removal_ids(email,user_id,cutoff) VALUES(?1,?2,?3) ON CONFLICT(email,user_id) DO UPDATE SET cutoff=MAX(cutoff,excluded.cutoff)",rusqlite::params![email,id,cutoff])?;
                    }
                }
            }
            continue;
        }
        let empty = if let Some(list) = value.as_array() {
            messages += list.len();
            json!([])
        } else {
            if key.ends_with("e2e_attachments.json") {
                if let Some(map) = value.as_object() {
                    attachments += map.len();
                    for entry in map.values() {
                        if let Some(file) = entry["file"].as_str().filter(|s| safe_filename(s)) {
                            tx.execute(
                                "INSERT OR IGNORE INTO chat_files_to_remove(file) VALUES(?1)",
                                [file],
                            )?;
                        }
                    }
                }
            }
            if key.ends_with("gif_favorites.json") {
                favorites += value
                    .as_object()
                    .map(|m| {
                        m.values()
                            .filter_map(Value::as_array)
                            .map(Vec::len)
                            .sum::<usize>()
                    })
                    .unwrap_or(0);
            }
            json!({})
        };
        tx.execute("INSERT INTO json_documents(path,content,updated_at) VALUES(?1,?2,?3) ON CONFLICT(path) DO UPDATE SET content=excluded.content,updated_at=excluded.updated_at",rusqlite::params![key,empty.to_string(),cutoff])?;
    }
    let status = json!({"cutoff":cutoff,"requestedBy":actor,"requestId":request_id,"messages":messages,"attachments":attachments,"savedGifs":favorites});
    tx.execute("INSERT INTO metadata(key,value,updated_at) VALUES('chat_history_reset',?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",rusqlite::params![status.to_string(),cutoff])?;
    tx.execute(
        "INSERT INTO metadata(key,value,updated_at) VALUES(?1,?2,?3)",
        rusqlite::params![request_key, status.to_string(), cutoff],
    )?;
    tx.commit()?;
    Ok(status)
}

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
    let cutoff = global_cutoff(conn);
    let newer = |entry: &Value| {
        entry
            .get("ts")
            .or_else(|| entry.get("createdAt"))
            .and_then(Value::as_i64)
            .unwrap_or(0)
            > cutoff
    };
    if cutoff > 0 {
        match name {
            "dms.json" | "pickle_dms.json" => {
                if let Some(list) = value.as_array_mut() {
                    list.retain(&newer);
                }
            }
            "e2e_attachments.json" => {
                if let Some(map) = value.as_object_mut() {
                    map.retain(|_, entry| newer(entry));
                }
            }
            "gif_favorites.json" => {
                if let Some(map) = value.as_object_mut() {
                    for list in map.values_mut().filter_map(Value::as_array_mut) {
                        list.retain(&newer);
                    }
                }
            }
            _ => {}
        }
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
    let conn = store.conn();
    let cutoff = global_cutoff(&conn);
    conn.query_row("SELECT EXISTS(SELECT 1 FROM moderated_media m LEFT JOIN account_chat_removals r ON m.email=r.email WHERE m.uri=?1 AND (m.blocked=1 OR m.created_at<=r.cutoff OR (?2>0 AND m.created_at<=?2)))",rusqlite::params![uri,cutoff],|r|r.get(0)).unwrap_or(true)
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn global_reset_is_durable_idempotent_and_preserves_new_history_and_accounts() {
        let dir =
            std::env::temp_dir().join(format!("chat-reset-{}", crate::crypto::random_bytes_hex(8)));
        std::fs::create_dir_all(dir.join("data/e2e_attachments")).unwrap();
        let store = DataStore::open(&dir, &dir.join("data")).unwrap();
        crate::auth::names_set(&store, "fixture", "member@example.com");
        let messages = json!([{"from":"member@example.com","ts":1,"text":"old"},{"from":"remote@example.com","ts":2,"text":"other"}]);
        let path = dir.join("data/dms.json");
        store.write_document(&path, &messages).unwrap();
        store
            .write_document(
                &dir.join("data/e2e_attachments.json"),
                &json!({"a":{"file":"old.gif","createdAt":1}}),
            )
            .unwrap();
        store
            .write_document(
                &dir.join("data/matrix_users.json"),
                &json!({"member@example.com":"old_name"}),
            )
            .unwrap();
        std::fs::write(dir.join("data/e2e_attachments/old.gif"), b"gif").unwrap();
        remember_media(
            &store,
            "mxc://mitch.pro/old",
            "member@example.com",
            1,
            false,
        );
        let status = reset_all(&store, "owner@example.com", "fixture-reset-0001", 10).unwrap();
        assert_eq!(status["messages"], 2);
        assert_eq!(store.read_document(&path, json!([])), json!([]));
        assert!(media_blocked(&store, "mxc://mitch.pro/old"));
        remove_files(&store).unwrap();
        assert!(!dir.join("data/e2e_attachments/old.gif").exists());
        assert_eq!(
            crate::auth::names_get_email(&store, "fixture").as_deref(),
            Some("member@example.com")
        );
        assert!(identity_cutoffs(&store).contains_key("@old_name:mitch.pro"));
        store
            .write_document(
                &path,
                &json!([messages[0],{"from":"member@example.com","ts":11,"text":"new"}]),
            )
            .unwrap();
        assert_eq!(
            store
                .read_document(&path, json!([]))
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            reset_all(&store, "owner@example.com", "fixture-reset-0001", 20).unwrap(),
            status
        );
        assert_eq!(history_reset_cutoff(&store), 10);
        drop(store);
        let store = DataStore::open(&dir, &dir.join("data")).unwrap();
        assert_eq!(history_reset_cutoff(&store), 10);
        assert_eq!(store.read_document(&path, json!([]))[0]["text"], "new");
        reset_all(&store, "other-owner@example.com", "fixture-reset-0002", 20).unwrap();
        store
            .write_document(
                &path,
                &json!([{"from":"neighbor@example.com","ts":21,"text":"after second reset"}]),
            )
            .unwrap();
        assert_eq!(
            reset_all(&store, "owner@example.com", "fixture-reset-0001", 30).unwrap(),
            status
        );
        assert_eq!(history_reset_cutoff(&store), 20);
        assert_eq!(
            store.read_document(&path, json!([]))[0]["text"],
            "after second reset"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
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
