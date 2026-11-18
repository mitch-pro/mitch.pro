//! Reports of Matrix chat messages and rooms, and what staff did with them.
//! Staff handle them on `/staff-dashboard/`; see `docs/staff-dashboard.md`.
//!
//! Stored in the `matrix_reports` SQL table. Reports used to be appended to
//! `data/chat_reports.json` alongside DM reports; `import_legacy` moves those
//! entries into the table (safe to run repeatedly) and returns the blob
//! without them, so the caller can write it back.

use crate::data::DataStore;
use serde_json::Value;

pub const STATUSES: &[&str] = &["open", "resolved", "dismissed"];
/// Most reports one list call returns.
pub const LIST_LIMIT: i64 = 300;
const MAX_REASON: usize = 500;
const MAX_BODY: usize = 2000;
const MAX_NOTE: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub id: String,
    pub room_id: String,
    pub room_name: String,
    /// Empty when a whole room was reported.
    pub event_id: String,
    pub reason: String,
    pub reported_by: String,
    /// Matrix ID of whoever wrote the reported message; filled in by staff-side
    /// lookup (`set_details`), empty until then or when it can't be found.
    pub sender: String,
    /// Text of the reported message, captured the same way.
    pub body: String,
    pub created_at: i64,
    pub status: String,
    pub handled_by: String,
    pub handled_at: Option<i64>,
    pub note: String,
}

const COLS: &str = "id, room_id, room_name, event_id, reason, reported_by, sender, body, created_at, status, handled_by, handled_at, note";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Report> {
    Ok(Report {
        id: r.get(0)?,
        room_id: r.get(1)?,
        room_name: r.get(2)?,
        event_id: r.get(3)?,
        reason: r.get(4)?,
        reported_by: r.get(5)?,
        sender: r.get(6)?,
        body: r.get(7)?,
        created_at: r.get(8)?,
        status: r.get(9)?,
        handled_by: r.get(10)?,
        handled_at: r.get(11)?,
        note: r.get(12)?,
    })
}

fn cap(s: &str, max: usize) -> String {
    s.trim().chars().take(max).collect()
}

/// Adds a report. The id is derived from the reported event, so reporting the
/// same message twice keeps one row (the first reason wins); a room report gets
/// a unique id. Returns the id, or `None` when there is no room.
pub fn add(
    store: &DataStore,
    room_id: &str,
    room_name: &str,
    event_id: &str,
    reason: &str,
    reported_by: &str,
    now: i64,
) -> Option<String> {
    if room_id.trim().is_empty() {
        return None;
    }
    let id = if event_id.is_empty() {
        format!("room-{now}-{:04x}", rand::random::<u16>())
    } else {
        format!("matrix-{event_id}")
    };
    store
        .conn()
        .execute(
            "INSERT OR IGNORE INTO matrix_reports (id, room_id, room_name, event_id, reason, reported_by, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                id,
                cap(room_id, 200),
                cap(room_name, 200),
                cap(event_id, 200),
                cap(reason, MAX_REASON),
                cap(reported_by, 200),
                now
            ],
        )
        .ok()?;
    Some(id)
}

pub fn get(store: &DataStore, id: &str) -> Option<Report> {
    store
        .conn()
        .query_row(
            &format!("SELECT {COLS} FROM matrix_reports WHERE id = ?1"),
            rusqlite::params![id],
            row,
        )
        .ok()
}

/// `open` lists unresolved reports oldest first, so the longest-waiting one is on
/// top. `closed` and `all` list newest first.
pub fn list(store: &DataStore, filter: &str, limit: i64) -> Vec<Report> {
    let (clause, order) = match filter {
        "open" => ("status = 'open'", "ASC"),
        "closed" => ("status <> 'open'", "DESC"),
        _ => ("1 = 1", "DESC"),
    };
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {COLS} FROM matrix_reports WHERE {clause} ORDER BY created_at {order}, id LIMIT ?1"
    )) else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![limit], row)
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

pub fn open_count(store: &DataStore) -> i64 {
    store
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM matrix_reports WHERE status = 'open'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0)
}

/// Records who wrote the reported message and what it said. Only fills empty
/// fields, so a later lookup (after the message was redacted) can't erase them.
pub fn set_details(store: &DataStore, id: &str, sender: &str, body: &str) {
    let _ = store.conn().execute(
        "UPDATE matrix_reports SET
            sender = CASE WHEN sender = '' THEN ?2 ELSE sender END,
            body = CASE WHEN body = '' THEN ?3 ELSE body END
         WHERE id = ?1",
        rusqlite::params![id, cap(sender, 200), cap(body, MAX_BODY)],
    );
}

/// Resolves, dismisses, or reopens a report. `false` for an unknown report or
/// status.
pub fn set_status(
    store: &DataStore,
    id: &str,
    status: &str,
    by: &str,
    note: &str,
    now: i64,
) -> bool {
    if !STATUSES.contains(&status) {
        return false;
    }
    let (by, at, note) = if status == "open" {
        ("", None, String::new())
    } else {
        (by, Some(now), cap(note, MAX_NOTE))
    };
    store
        .conn()
        .execute(
            "UPDATE matrix_reports SET status = ?2, handled_by = ?3, handled_at = ?4, note = ?5 WHERE id = ?1",
            rusqlite::params![id, status, by, at, note],
        )
        .map(|n| n > 0)
        .unwrap_or(false)
}

/// Moves Matrix reports out of the legacy `chat_reports.json` list into the
/// table. Returns the list without them when anything moved, so the caller can
/// store it back; `None` when there was nothing to move. Old `Resolved` entries
/// keep that state.
pub fn import_legacy(store: &DataStore, legacy: &Value) -> Option<Value> {
    let list = legacy.as_array()?;
    let is_matrix = |r: &Value| {
        r.get("matrixRoomId").is_some()
            || r.get("id")
                .and_then(Value::as_str)
                .is_some_and(|s| s.starts_with("matrix-"))
    };
    if !list.iter().any(is_matrix) {
        return None;
    }
    let text = |r: &Value, k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    for r in list.iter().filter(|r| is_matrix(r)) {
        let id = text(r, "id");
        if id.is_empty() {
            continue;
        }
        let resolved = text(r, "status").eq_ignore_ascii_case("resolved");
        let sender = text(r, "matrixSender");
        let _ = store.conn().execute(
            "INSERT OR IGNORE INTO matrix_reports
             (id, room_id, room_name, event_id, reason, reported_by, sender, created_at, status, handled_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                cap(&id, 200),
                cap(&text(r, "matrixRoomId"), 200),
                cap(&text(r, "matrixRoomName"), 200),
                cap(&text(r, "matrixEventId"), 200),
                cap(&text(r, "reason"), MAX_REASON),
                cap(&text(r, "reportedBy"), 200),
                if sender == "unknown" { String::new() } else { cap(&sender, 200) },
                r.get("ts").and_then(Value::as_f64).unwrap_or(0.0) as i64,
                if resolved { "resolved" } else { "open" },
                if resolved { Some(0_i64) } else { None },
            ],
        );
    }
    Some(Value::Array(
        list.iter().filter(|r| !is_matrix(r)).cloned().collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-matrix-reports-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn reports_queue_oldest_first_and_can_be_closed_and_reopened() {
        let store = temp_store("queue");
        assert!(add(&store, " ", "", "", "x", "a@x.test", 1).is_none());
        let a = add(&store, "!r:x", "General", "$e1", "spam", "a@x.test", 2_000).unwrap();
        let b = add(&store, "!r:x", "General", "$e2", "rude", "b@x.test", 1_000).unwrap();
        // Reporting the same message again keeps the first report.
        add(&store, "!r:x", "General", "$e1", "again", "c@x.test", 3_000).unwrap();
        assert_eq!(open_count(&store), 2);
        let ids: Vec<String> = list(&store, "open", 10).into_iter().map(|r| r.id).collect();
        assert_eq!(ids, [b.clone(), a.clone()]);
        assert_eq!(get(&store, &a).unwrap().reason, "spam");

        assert!(set_status(
            &store,
            &a,
            "dismissed",
            "m@x.test",
            "fine",
            5_000
        ));
        assert!(!set_status(&store, &a, "weird", "m@x.test", "", 5_000));
        assert!(!set_status(
            &store, "nope", "resolved", "m@x.test", "", 5_000
        ));
        assert_eq!(open_count(&store), 1);
        let closed = list(&store, "closed", 10);
        assert_eq!(closed.len(), 1);
        assert_eq!(
            (closed[0].handled_by.as_str(), closed[0].note.as_str()),
            ("m@x.test", "fine")
        );
        assert!(set_status(&store, &a, "open", "", "", 6_000));
        assert_eq!(get(&store, &a).unwrap().handled_by, "");
        assert_eq!(list(&store, "all", 10).len(), 2);
    }

    #[test]
    fn details_fill_once_and_are_never_erased() {
        let store = temp_store("details");
        let id = add(&store, "!r:x", "", "$e", "bad", "a@x.test", 1).unwrap();
        set_details(&store, &id, "@kid:mitch.pro", "hello");
        set_details(&store, &id, "@other:mitch.pro", "");
        let r = get(&store, &id).unwrap();
        assert_eq!(
            (r.sender.as_str(), r.body.as_str()),
            ("@kid:mitch.pro", "hello")
        );
    }

    #[test]
    fn legacy_matrix_entries_move_out_of_the_chat_report_list() {
        let store = temp_store("legacy");
        let legacy = json!([
            { "id": "dm-1", "reason": "dm", "ts": 1 },
            { "id": "matrix-$e", "reason": "[Matrix Room !r:x] spam", "reportedBy": "a@x.test",
              "ts": 2000, "status": "Needs review", "matrixRoomId": "!r:x",
              "matrixEventId": "$e", "matrixSender": "unknown" },
            { "id": "room-9", "reason": "r", "ts": 3000, "status": "Resolved", "matrixRoomId": "!q:x" }
        ]);
        let rest = import_legacy(&store, &legacy).unwrap();
        assert_eq!(rest, json!([{ "id": "dm-1", "reason": "dm", "ts": 1 }]));
        assert_eq!(open_count(&store), 1);
        assert_eq!(get(&store, "matrix-$e").unwrap().sender, "");
        assert_eq!(get(&store, "room-9").unwrap().status, "resolved");
        // Nothing left to move the second time.
        assert!(import_legacy(&store, &rest).is_none());
    }
}
