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

/// Apply a friend action and both membership lists in one write transaction.
/// The result's `changed` flag is the sole permission to emit a push alert;
/// retries and simultaneous requests cannot generate duplicate notifications.
pub fn change_request(
    store: &DataStore,
    from: &str,
    to: &str,
    action: &str,
) -> Result<(&'static str, bool), &'static str> {
    use rusqlite::{params, OptionalExtension, TransactionBehavior};
    use std::collections::BTreeSet;
    let (from, to) = (
        crate::auth::normalize_email(from),
        crate::auth::normalize_email(to),
    );
    if from.is_empty() || to.is_empty() || from == to {
        return Err("Choose another account.");
    }
    let mut conn = store.conn();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| "Could not update friends.")?;
    let key = store.relative_key(&store.data_dir().join("friend_requests.json"));
    let raw: Option<String> = tx
        .query_row(
            "SELECT content FROM json_documents WHERE path=?1",
            [&key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| "Could not load requests.")?;
    let raw = match raw {
        Some(raw) => Some(raw),
        None => match std::fs::read_to_string(store.data_dir().join("friend_requests.json")) {
            Ok(raw) => Some(raw),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err("Could not load requests."),
        },
    };
    let mut requests: Vec<Value> = match raw {
        Some(raw) => serde_json::from_str(&raw).map_err(|_| "Could not read requests.")?,
        None => vec![],
    };
    let get_list = |email: &str| -> Result<BTreeSet<String>, &'static str> {
        let raw: Option<String> = tx
            .query_row(
                "SELECT data FROM friend_lists WHERE email=?1",
                [email],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| "Could not load friends.")?;
        let list: Vec<String> = match raw {
            Some(raw) => serde_json::from_str(&raw).map_err(|_| "Could not read friends.")?,
            None => vec![],
        };
        Ok(list
            .iter()
            .map(|email| crate::auth::normalize_email(email))
            .collect())
    };
    let (mut mine, mut theirs) = (get_list(&from)?, get_list(&to)?);
    let mutual = mine.contains(&to) && theirs.contains(&from);
    let between = |r: &Value, a: &str, b: &str| {
        crate::auth::normalize_email(r["from"].as_str().unwrap_or("")) == a
            && crate::auth::normalize_email(r["to"].as_str().unwrap_or("")) == b
    };
    let incoming = requests.iter().any(|r| between(r, &to, &from));
    let outgoing = requests.iter().any(|r| between(r, &from, &to));
    let (status, changed, connect) = match action {
        "request" if mutual => ("accepted", false, Some(true)),
        "request" if incoming => ("accepted", true, Some(true)),
        "request" if outgoing => ("pending", false, None),
        "request" => {
            requests.push(json!({"from":from,"to":to,"timestamp":now_millis()}));
            ("pending", true, None)
        }
        "accept" if incoming => ("accepted", true, Some(true)),
        "accept" if mutual => ("accepted", false, Some(true)),
        "accept" => return Err("No pending request from this account."),
        "reject" => ("rejected", incoming, None),
        "cancel" => ("cancelled", outgoing, None),
        "remove" => (
            "removed",
            mine.contains(&to) || theirs.contains(&from),
            Some(false),
        ),
        _ => return Err("Invalid friend action."),
    };
    if matches!(action, "accept" | "remove") || connect == Some(true) {
        requests.retain(|r| !between(r, &from, &to) && !between(r, &to, &from));
    } else if action == "reject" {
        requests.retain(|r| !between(r, &to, &from));
    } else if action == "cancel" {
        requests.retain(|r| !between(r, &from, &to));
    }
    if let Some(connect) = connect {
        if connect {
            mine.insert(to.clone());
            theirs.insert(from.clone());
        } else {
            mine.remove(&to);
            theirs.remove(&from);
        }
        for (email, list) in [(&from, mine), (&to, theirs)] {
            let encoded = serde_json::to_string(&list).map_err(|_| "Could not save friends.")?;
            tx.execute("INSERT INTO friend_lists(email,data,updated_at) VALUES(?1,?2,?3) ON CONFLICT(email) DO UPDATE SET data=excluded.data,updated_at=excluded.updated_at",params![email,encoded,now_millis()]).map_err(|_|"Could not save friends.")?;
        }
    }
    let encoded = serde_json::to_string(&requests).map_err(|_| "Could not save requests.")?;
    tx.execute("INSERT INTO json_documents(path,content,updated_at) VALUES(?1,?2,?3) ON CONFLICT(path) DO UPDATE SET content=excluded.content,updated_at=excluded.updated_at",params![key,encoded,now_millis()]).map_err(|_|"Could not save requests.")?;
    tx.commit().map_err(|_| "Could not save friends.")?;
    Ok((status, changed))
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
    fn mutual_requests_and_retries_update_both_lists_once() {
        let store = std::sync::Arc::new(temp_store("request-atomic"));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let store = store.clone();
                std::thread::spawn(move || {
                    change_request(
                        &store,
                        "sender@example.test",
                        "receiver@example.test",
                        "request",
                    )
                    .unwrap()
                })
            })
            .collect();
        let changes = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|(_, changed)| *changed)
            .count();
        assert_eq!(changes, 1);
        assert_eq!(
            change_request(
                &store,
                "receiver@example.test",
                "sender@example.test",
                "request"
            ),
            Ok(("accepted", true))
        );
        assert_eq!(
            change_request(
                &store,
                "receiver@example.test",
                "sender@example.test",
                "accept"
            ),
            Ok(("accepted", false))
        );
        assert_eq!(
            friends_get(&store, "receiver@example.test"),
            json!(["sender@example.test"])
        );
        assert_eq!(
            friends_get(&store, "sender@example.test"),
            json!(["receiver@example.test"])
        );
        assert_eq!(
            change_request(
                &store,
                "receiver@example.test",
                "sender@example.test",
                "remove"
            ),
            Ok(("removed", true))
        );
        assert_eq!(
            change_request(
                &store,
                "receiver@example.test",
                "sender@example.test",
                "remove"
            ),
            Ok(("removed", false))
        );
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
