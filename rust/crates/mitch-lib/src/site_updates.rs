//! The manual update log shown on the homepage ("what changed"). Entries are
//! written by an admin on `/admin/updates/`; they are not generated from commit
//! messages and are not tracked by git. They live in the `site_updates` SQL
//! table (see `docs/site-updates.md`).

use crate::data::DataStore;

pub const MAX_TITLE: usize = 120;
pub const MAX_BODY: usize = 2000;
/// Most entries the admin page lists.
pub const ADMIN_LIST_LIMIT: i64 = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    pub id: i64,
    pub title: String,
    pub body: String,
    /// When it is shown as posted (milliseconds since the epoch).
    pub posted_at: i64,
    pub author: String,
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Update> {
    Ok(Update {
        id: r.get(0)?,
        title: r.get(1)?,
        body: r.get(2)?,
        posted_at: r.get(3)?,
        author: r.get(4)?,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub enum SaveError {
    EmptyTitle,
    TitleTooLong,
    BodyTooLong,
    NotFound,
    Database,
}

fn check(title: &str, body: &str) -> Result<(), SaveError> {
    if title.trim().is_empty() {
        return Err(SaveError::EmptyTitle);
    }
    if title.chars().count() > MAX_TITLE {
        return Err(SaveError::TitleTooLong);
    }
    if body.chars().count() > MAX_BODY {
        return Err(SaveError::BodyTooLong);
    }
    Ok(())
}

/// Newest first. `limit` is clamped to 1..=200.
pub fn list(store: &DataStore, limit: i64) -> Vec<Update> {
    let limit = limit.clamp(1, ADMIN_LIST_LIMIT);
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, title, body, posted_at, author FROM site_updates
         ORDER BY posted_at DESC, id DESC LIMIT ?1",
    ) else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![limit], row)
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// Adds an entry. `posted_at` defaults to now.
pub fn create(
    store: &DataStore,
    title: &str,
    body: &str,
    posted_at: Option<i64>,
    author: &str,
) -> Result<i64, SaveError> {
    check(title, body)?;
    let now = now_millis();
    let conn = store.conn();
    conn.execute(
        "INSERT INTO site_updates (title, body, posted_at, updated_at, author) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![title.trim(), body.trim(), posted_at.unwrap_or(now), now, author],
    )
    .map_err(|_| SaveError::Database)?;
    Ok(conn.last_insert_rowid())
}

/// Edits an entry's text and, when given, its posted date.
pub fn update(
    store: &DataStore,
    id: i64,
    title: &str,
    body: &str,
    posted_at: Option<i64>,
) -> Result<(), SaveError> {
    check(title, body)?;
    let changed = store
        .conn()
        .execute(
            "UPDATE site_updates SET title = ?2, body = ?3, posted_at = COALESCE(?4, posted_at), updated_at = ?5 WHERE id = ?1",
            rusqlite::params![id, title.trim(), body.trim(), posted_at, now_millis()],
        )
        .map_err(|_| SaveError::Database)?;
    if changed == 0 {
        return Err(SaveError::NotFound);
    }
    Ok(())
}

pub fn delete(store: &DataStore, id: i64) -> bool {
    store
        .conn()
        .execute(
            "DELETE FROM site_updates WHERE id = ?1",
            rusqlite::params![id],
        )
        .map(|n| n > 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-site-updates-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn entries_are_listed_newest_first_and_can_be_edited_and_removed() {
        let store = temp_store("crud");
        assert!(list(&store, 10).is_empty());
        let a = create(&store, " First ", "one", Some(1_000), "me@x.test").unwrap();
        let b = create(&store, "Second", "", Some(3_000), "me@x.test").unwrap();
        let c = create(&store, "Third", "three", Some(2_000), "me@x.test").unwrap();
        let ids: Vec<i64> = list(&store, 10).iter().map(|u| u.id).collect();
        assert_eq!(ids, [b, c, a]);
        assert_eq!(list(&store, 10)[2].title, "First");
        assert_eq!(list(&store, 1).len(), 1);

        update(&store, a, "First, edited", "uno", None).unwrap();
        let first = list(&store, 10).into_iter().find(|u| u.id == a).unwrap();
        assert_eq!(
            (first.title.as_str(), first.body.as_str(), first.posted_at),
            ("First, edited", "uno", 1_000)
        );
        update(&store, a, "First, edited", "uno", Some(9_000)).unwrap();
        assert_eq!(list(&store, 10)[0].id, a);

        assert_eq!(update(&store, 999, "x", "", None), Err(SaveError::NotFound));
        assert!(delete(&store, b));
        assert!(!delete(&store, b));
        assert_eq!(list(&store, 10).len(), 2);
    }

    #[test]
    fn titles_and_bodies_are_checked() {
        let store = temp_store("limits");
        assert_eq!(
            create(&store, "  ", "x", None, "a").unwrap_err(),
            SaveError::EmptyTitle
        );
        assert_eq!(
            create(&store, &"t".repeat(MAX_TITLE + 1), "", None, "a").unwrap_err(),
            SaveError::TitleTooLong
        );
        assert_eq!(
            create(&store, "ok", &"b".repeat(MAX_BODY + 1), None, "a").unwrap_err(),
            SaveError::BodyTooLong
        );
        assert!(create(
            &store,
            &"t".repeat(MAX_TITLE),
            &"b".repeat(MAX_BODY),
            None,
            "a"
        )
        .is_ok());
    }
}
