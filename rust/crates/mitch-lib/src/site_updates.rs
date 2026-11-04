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

// ── Automatic entries ──────────────────────────────────────────────────────
//
// At the start of each holiday theme (see `seasons`) one entry is posted from
// an admin-editable template. `auto_update_posted` remembers which season and
// year were handled, so a deleted entry does not come back, and so the first
// run never posts for a season that is already under way.

pub const AUTO_AUTHOR: &str = "automatic";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoTemplate {
    pub season: String,
    pub title: String,
    pub body: String,
    pub enabled: bool,
}

const OFF_HINT: &str = "Turn it off with the button at the top of the homepage or in Preferences.";

fn default_template(season: &str) -> (&'static str, String) {
    let (title, detail) = match season {
        "halloween" => (
            "Halloween theme is on",
            "Pumpkin colors and floating embers until Halloween.",
        ),
        "thanksgiving" => (
            "Thanksgiving theme is on",
            "Autumn colors and falling leaves until Thanksgiving.",
        ),
        "christmas" => (
            "Christmas theme is on",
            "Evergreen and red with snow and string lights through December 26.",
        ),
        "newyear" => (
            "New Year's theme is on",
            "Midnight blue, gold and confetti until January 3.",
        ),
        _ => (
            "Valentine's theme is on",
            "Rose colors and floating hearts until February 14.",
        ),
    };
    (title, format!("{detail} {OFF_HINT}"))
}

/// Creates the default template for any season that has none.
pub fn seed_templates(store: &DataStore) {
    let conn = store.conn();
    for season in crate::seasons::ALL {
        let (title, body) = default_template(season);
        let _ = conn.execute(
            "INSERT OR IGNORE INTO auto_update_templates (season, title, body, enabled) VALUES (?1, ?2, ?3, 1)",
            rusqlite::params![season, title, body],
        );
    }
}

/// All templates, in season order.
pub fn templates(store: &DataStore) -> Vec<AutoTemplate> {
    seed_templates(store);
    let conn = store.conn();
    crate::seasons::ALL
        .iter()
        .filter_map(|season| {
            conn.query_row(
                "SELECT season, title, body, enabled FROM auto_update_templates WHERE season = ?1",
                rusqlite::params![season],
                |r| {
                    Ok(AutoTemplate {
                        season: r.get(0)?,
                        title: r.get(1)?,
                        body: r.get(2)?,
                        enabled: r.get::<_, i64>(3)? != 0,
                    })
                },
            )
            .ok()
        })
        .collect()
}

pub fn save_template(
    store: &DataStore,
    season: &str,
    title: &str,
    body: &str,
    enabled: bool,
) -> Result<(), SaveError> {
    if !crate::seasons::ALL.contains(&season) {
        return Err(SaveError::NotFound);
    }
    check(title, body)?;
    seed_templates(store);
    store
        .conn()
        .execute(
            "UPDATE auto_update_templates SET title = ?2, body = ?3, enabled = ?4 WHERE season = ?1",
            rusqlite::params![season, title.trim(), body.trim(), enabled as i64],
        )
        .map(|_| ())
        .map_err(|_| SaveError::Database)
}

/// Posts the entry for the season `today` is in, if it has not been handled
/// this year. Returns the new entry's id when one was posted.
///
/// - The first run ever (nothing recorded yet) only records the season already
///   under way as handled, so enabling this never posts retroactively.
/// - A season whose template is switched off is recorded without posting.
/// - A deleted entry stays deleted: the season/year record is kept.
pub fn run_auto(store: &DataStore, today: chrono::NaiveDate, now_ms: i64) -> Option<i64> {
    use chrono::Datelike;
    let (season, start) = crate::seasons::current(today)?;
    let year = start.year();
    let conn = store.conn();
    let handled: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM auto_update_posted WHERE season = ?1 AND year = ?2",
            rusqlite::params![season, year],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(true);
    if handled {
        return None;
    }
    let first_run: bool = conn
        .query_row("SELECT COUNT(*) FROM auto_update_posted", [], |r| {
            r.get::<_, i64>(0)
        })
        .map(|n| n == 0)
        .unwrap_or(false);
    drop(conn);
    let record = |entry_id: Option<i64>| {
        let _ = store.conn().execute(
            "INSERT OR IGNORE INTO auto_update_posted (season, year, entry_id, posted_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![season, year, entry_id, now_ms],
        );
    };
    if first_run {
        record(None);
        return None;
    }
    let template = templates(store).into_iter().find(|t| t.season == season)?;
    if !template.enabled {
        record(None);
        return None;
    }
    match create(
        store,
        &template.title,
        &template.body,
        Some(now_ms),
        AUTO_AUTHOR,
    ) {
        Ok(id) => {
            record(Some(id));
            Some(id)
        }
        Err(_) => None,
    }
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

    fn day(y: i32, m: u32, d: u32) -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn the_first_run_never_posts_for_a_season_already_under_way() {
        let store = temp_store("auto-first");
        // Nov 4 is inside Thanksgiving, which began Nov 1: recorded, not posted.
        assert_eq!(run_auto(&store, day(2026, 11, 4), 1_000), None);
        assert!(list(&store, 10).is_empty());
        // Later the same season does nothing.
        assert_eq!(run_auto(&store, day(2026, 11, 20), 2_000), None);
        // The next season posts on its first day (and again that day: only once).
        let id = run_auto(&store, day(2026, 11, 27), 3_000).expect("christmas entry");
        assert_eq!(run_auto(&store, day(2026, 11, 27), 3_500), None);
        assert_eq!(run_auto(&store, day(2026, 12, 3), 4_000), None);
        let entries = list(&store, 10);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, id);
        assert_eq!(entries[0].title, "Christmas theme is on");
        assert_eq!(entries[0].author, AUTO_AUTHOR);
        assert_eq!(entries[0].posted_at, 3_000);
    }

    #[test]
    fn nothing_is_posted_between_seasons() {
        let store = temp_store("auto-gap");
        assert_eq!(run_auto(&store, day(2027, 3, 1), 1), None);
        // No record was made, so the first season after still counts as the first run.
        assert_eq!(run_auto(&store, day(2027, 2, 7), 2), None);
        assert!(list(&store, 10).is_empty());
    }

    #[test]
    fn templates_can_be_customised_switched_off_and_a_deleted_entry_stays_deleted() {
        let store = temp_store("auto-custom");
        run_auto(&store, day(2026, 11, 4), 1); // first run: Thanksgiving recorded
        let all = templates(&store);
        assert_eq!(all.len(), 5);
        assert_eq!(all[2].season, "christmas");
        assert!(all.iter().all(|t| t.enabled && !t.title.is_empty()));

        save_template(
            &store,
            "christmas",
            "Winter is here",
            "Snow and lights.",
            true,
        )
        .unwrap();
        save_template(&store, "newyear", "New Year's theme is on", "x", false).unwrap();
        assert_eq!(
            save_template(&store, "nope", "t", "", true),
            Err(SaveError::NotFound)
        );
        assert_eq!(
            save_template(&store, "christmas", " ", "", true),
            Err(SaveError::EmptyTitle)
        );

        let id = run_auto(&store, day(2026, 11, 27), 10).unwrap();
        assert_eq!(list(&store, 10)[0].title, "Winter is here");
        assert_eq!(list(&store, 10)[0].body, "Snow and lights.");
        // An admin deletes it: it does not come back.
        assert!(delete(&store, id));
        assert_eq!(run_auto(&store, day(2026, 12, 1), 20), None);
        assert!(list(&store, 10).is_empty());
        // A switched-off season is skipped, not posted.
        assert_eq!(run_auto(&store, day(2026, 12, 28), 30), None);
        assert!(list(&store, 10).is_empty());
        // Next year's Christmas posts again.
        assert!(run_auto(&store, day(2027, 11, 26), 40).is_some());
    }
}
