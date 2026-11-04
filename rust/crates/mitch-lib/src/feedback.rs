//! Feedback sent from `/feedback/`, and what staff do with it. Staff (moderators
//! and admins) read it on `/moderator/feedback/`; see `docs/feedback.md`.
//!
//! Stored in the `feedback_entries` SQL table. The old `data/suggestions.json`
//! file is imported into it (`import_legacy`, safe to run repeatedly), and
//! nothing writes to that file any more.

use crate::data::DataStore;
use serde_json::Value;

pub const MAX_TEXT: usize = 2000;
pub const MAX_KIND: usize = 50;
pub const STATUSES: &[&str] = &["new", "handled"];
/// Most entries one list call returns.
pub const LIST_LIMIT: i64 = 300;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: i64,
    pub kind: String,
    pub text: String,
    /// The signed-in account that sent it, "" when it came from a visitor.
    pub account: String,
    pub created_at: i64,
    pub status: String,
    pub handled_by: String,
    pub handled_at: Option<i64>,
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
    Ok(Entry {
        id: r.get(0)?,
        kind: r.get(1)?,
        text: r.get(2)?,
        account: r.get(3)?,
        created_at: r.get(4)?,
        status: r.get(5)?,
        handled_by: r.get(6)?,
        handled_at: r.get(7)?,
    })
}

const COLS: &str = "id, kind, text, account, created_at, status, handled_by, handled_at";

/// Adds a submission. `None` when the text is empty.
pub fn add(
    store: &DataStore,
    kind: &str,
    text: &str,
    account: &str,
    created_at: i64,
) -> Option<i64> {
    let text: String = text.trim().chars().take(MAX_TEXT).collect();
    if text.is_empty() {
        return None;
    }
    let kind: String = kind.trim().chars().take(MAX_KIND).collect();
    let conn = store.conn();
    conn.execute(
        "INSERT OR IGNORE INTO feedback_entries (kind, text, account, created_at) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![kind, text, account, created_at],
    )
    .ok()?;
    Some(conn.last_insert_rowid())
}

/// Newest first. `status` is `new`, `handled`, or anything else for all.
pub fn list(store: &DataStore, status: &str) -> Vec<Entry> {
    let conn = store.conn();
    let filter = if STATUSES.contains(&status) {
        status
    } else {
        "%"
    };
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {COLS} FROM feedback_entries WHERE status LIKE ?1 ORDER BY created_at DESC, id DESC LIMIT ?2"
    )) else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![filter, LIST_LIMIT], row)
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// `(new, handled)` counts.
pub fn counts(store: &DataStore) -> (i64, i64) {
    let conn = store.conn();
    let count = |status: &str| -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM feedback_entries WHERE status = ?1",
            rusqlite::params![status],
            |r| r.get(0),
        )
        .unwrap_or(0)
    };
    (count("new"), count("handled"))
}

/// Marks an entry handled or new again. `false` when there is no such entry or
/// the status is not one of [`STATUSES`].
pub fn set_status(store: &DataStore, id: i64, status: &str, by: &str, now: i64) -> bool {
    if !STATUSES.contains(&status) {
        return false;
    }
    let (by, at) = if status == "handled" {
        (by, Some(now))
    } else {
        ("", None)
    };
    store
        .conn()
        .execute(
            "UPDATE feedback_entries SET status = ?2, handled_by = ?3, handled_at = ?4 WHERE id = ?1",
            rusqlite::params![id, status, by, at],
        )
        .map(|n| n > 0)
        .unwrap_or(false)
}

pub fn delete(store: &DataStore, id: i64) -> bool {
    store
        .conn()
        .execute(
            "DELETE FROM feedback_entries WHERE id = ?1",
            rusqlite::params![id],
        )
        .map(|n| n > 0)
        .unwrap_or(false)
}

/// Imports entries from the old `suggestions.json` list. An entry is added at
/// most once (the table is unique on time, kind and text), so this can run on
/// every list call. Only an email is kept as the account; the old file's raw
/// session ids are not copied.
pub fn import_legacy(store: &DataStore, legacy: &Value) {
    for s in legacy.as_array().into_iter().flatten() {
        let text = s.get("text").and_then(Value::as_str).unwrap_or("");
        let kind = s.get("type").and_then(Value::as_str).unwrap_or("general");
        let name = s.get("name").and_then(Value::as_str).unwrap_or("");
        let account = if name.contains('@') { name } else { "" };
        let ts = s.get("ts").and_then(Value::as_f64).unwrap_or(0.0);
        add(store, kind, text, account, (ts * 1000.0) as i64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-feedback-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn entries_are_listed_newest_first_and_can_be_handled_and_deleted() {
        let store = temp_store("crud");
        assert!(add(&store, "feedback", "  ", "a@x.test", 1).is_none());
        let a = add(&store, "feedback", "first", "a@x.test", 1_000).unwrap();
        let b = add(&store, "broken_game", "second", "", 2_000).unwrap();
        let ids: Vec<i64> = list(&store, "all").iter().map(|e| e.id).collect();
        assert_eq!(ids, [b, a]);
        assert_eq!(counts(&store), (2, 0));

        assert!(set_status(&store, a, "handled", "mod@x.test", 5_000));
        assert!(!set_status(&store, a, "weird", "mod@x.test", 5_000));
        assert!(!set_status(&store, 99, "handled", "mod@x.test", 5_000));
        assert_eq!(counts(&store), (1, 1));
        let handled = list(&store, "handled");
        assert_eq!(handled.len(), 1);
        assert_eq!(
            (handled[0].handled_by.as_str(), handled[0].handled_at),
            ("mod@x.test", Some(5_000))
        );
        assert_eq!(list(&store, "new")[0].id, b);
        // Reopening clears who handled it.
        assert!(set_status(&store, a, "new", "", 6_000));
        assert_eq!(list(&store, "new").len(), 2);
        assert_eq!(list(&store, "new")[1].handled_by, "");

        assert!(delete(&store, b));
        assert!(!delete(&store, b));
        assert_eq!(list(&store, "all").len(), 1);
    }

    #[test]
    fn text_and_kind_are_trimmed_and_capped() {
        let store = temp_store("caps");
        let id = add(
            &store,
            &"k".repeat(80),
            &format!("  {}  ", "t".repeat(3000)),
            "",
            1,
        )
        .unwrap();
        let entry = &list(&store, "all")[0];
        assert_eq!(entry.id, id);
        assert_eq!(entry.text.chars().count(), MAX_TEXT);
        assert_eq!(entry.kind.chars().count(), MAX_KIND);
    }

    #[test]
    fn the_legacy_file_imports_once_and_keeps_only_emails() {
        let store = temp_store("legacy");
        let legacy = json!([
            { "id": "RAW-SESSION-ID", "name": "student@x.test", "type": "feedback", "text": "love it", "ts": 1700000000.5 },
            { "id": "RAW-SESSION-ID-2", "name": "abc123", "type": "broken_game", "text": "drive mad crashes", "ts": 1700000100.0 },
            { "id": "x", "name": "", "type": "general", "text": " ", "ts": 1.0 }
        ]);
        import_legacy(&store, &legacy);
        import_legacy(&store, &legacy);
        let entries = list(&store, "all");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].text, "drive mad crashes");
        assert_eq!(entries[0].account, "");
        assert_eq!(entries[1].account, "student@x.test");
        assert_eq!(entries[1].created_at, 1_700_000_000_500);
        assert!(entries
            .iter()
            .all(|e| !e.text.contains("RAW") && !e.account.contains("RAW")));
    }
}
