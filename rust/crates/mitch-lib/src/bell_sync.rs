//! Published bell schedules scraped from the district's school sites.
//!
//! Each RJUHSD school publishes its bell schedule as Finalsite tables on
//! `/about/bell-schedule(s)`: a group heading (`<h2 class="fsElementTitle">`,
//! e.g. "(M, Tu, Th, F) Regular Day" or "Tuesday Schedule"), then one table per
//! variant with a `<caption>` ("1st Lunch", "Minimum Day", ...) and
//! `Period | Start | End` rows in 12-hour times without an AM/PM marker.
//!
//! This module has two halves:
//! - [`parse_bell_page`] turns that HTML into [`PublishedTable`]s. It is
//!   deliberately strict: a table is kept only if every row parses and the day
//!   runs forward in time, so a redesigned page yields nothing rather than
//!   wrong bells.
//! - [`replace_school`] / [`load_school`] persist the result in the
//!   `bell_published_periods` and `bell_sync_status` tables. A school's rows are
//!   replaced in one transaction and only when the parse produced at least one
//!   table, so a failed or empty fetch keeps the last good data.
//!
//! The bundled `webserver/bell/schedule.js` remains the fallback; the client
//! overlays this data on top of it (see `docs/bell-sync.md`).
#![allow(clippy::expect_used)]

use crate::data::DataStore;
use regex::Regex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

/// One period row, in minutes after midnight (24-hour).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublishedPeriod {
    pub name: String,
    pub start: u32,
    pub end: u32,
}

/// One table from a school's bell-schedule page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublishedTable {
    /// Section heading above the table ("Tuesday Schedule", "Regular Day").
    pub group: String,
    /// Table caption ("1st Lunch", "Minimum Day"); empty if the table has none.
    pub label: String,
    pub periods: Vec<PublishedPeriod>,
}

/// Outcome of the most recent fetch for a school.
#[derive(Debug, Clone, Serialize)]
pub struct SyncStatus {
    pub source_url: String,
    /// When the data last changed or was last confirmed (epoch ms).
    pub checked_at: i64,
    /// Last attempt that produced usable tables (epoch ms), 0 if never.
    pub fetched_at: i64,
    pub ok: bool,
    pub error: String,
}

/// Headings that appear on every Finalsite page and are not schedule groups.
const IGNORED_HEADINGS: &[&str] = &[
    "leaving our website",
    "logo image",
    "logo title",
    "in this section",
    "bell schedule",
    "bell schedules",
];

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("static regex"))
}

fn strip_tags(fragment: &str) -> String {
    static TAG: OnceLock<Regex> = OnceLock::new();
    let text = re(&TAG, r"(?s)<[^>]*>").replace_all(fragment, " ");
    let text = text
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// "8:30" -> 510, "1:22" -> 802. School days run roughly 7:00-17:00, so hours
/// 1-6 are afternoon and 7-12 are as written (12:xx is noon).
pub fn parse_clock(text: &str) -> Option<u32> {
    static CLOCK: OnceLock<Regex> = OnceLock::new();
    let caps = re(&CLOCK, r"^(\d{1,2}):(\d{2})$").captures(text.trim())?;
    let hour: u32 = caps[1].parse().ok()?;
    let minute: u32 = caps[2].parse().ok()?;
    if !(1..=12).contains(&hour) || minute > 59 {
        return None;
    }
    let hour24 = if hour <= 6 { hour + 12 } else { hour };
    Some(hour24 * 60 + minute)
}

/// Parse every well-formed schedule table on a bell-schedule page.
pub fn parse_bell_page(html: &str) -> Vec<PublishedTable> {
    static STRIP: OnceLock<Regex> = OnceLock::new();
    static TOKEN: OnceLock<Regex> = OnceLock::new();
    static ROW: OnceLock<Regex> = OnceLock::new();
    static CELL: OnceLock<Regex> = OnceLock::new();

    let html = re(&STRIP, r"(?is)<(script|style)\b.*?</(script|style)>").replace_all(html, "");
    let token = re(
        &TOKEN,
        r#"(?is)<h2 class="fsElementTitle[^>]*>(.*?)</h2>|<table\b[^>]*>(.*?)</table>"#,
    );
    let row = re(&ROW, r"(?is)<tr[^>]*>(.*?)</tr>");
    let cell = re(&CELL, r"(?is)<t[dh][^>]*>(.*?)</t[dh]>");
    static CAPTION: OnceLock<Regex> = OnceLock::new();
    let caption = re(&CAPTION, r"(?is)<caption[^>]*>(.*?)</caption>");

    let mut group = String::new();
    let mut tables = Vec::new();
    for caps in token.captures_iter(&html) {
        if let Some(heading) = caps.get(1) {
            let text = strip_tags(heading.as_str());
            if !text.is_empty() && !IGNORED_HEADINGS.contains(&text.to_lowercase().as_str()) {
                group = text;
            }
            continue;
        }
        let Some(body) = caps.get(2) else { continue };
        let body = body.as_str();
        let label = caption
            .captures(body)
            .map(|c| strip_tags(&c[1]))
            .unwrap_or_default();

        let mut periods = Vec::new();
        let mut valid = true;
        for row_caps in row.captures_iter(body) {
            let cells: Vec<String> = cell
                .captures_iter(&row_caps[1])
                .map(|c| strip_tags(&c[1]))
                .collect();
            // Header rows ("Period / Description | Start Time | End Time")
            // have non-time cells and are skipped; data rows must be 3 cells.
            if cells.len() != 3 {
                continue;
            }
            let (start, end) = (parse_clock(&cells[1]), parse_clock(&cells[2]));
            let (Some(start), Some(end)) = (start, end) else {
                if start.is_none() && end.is_none() {
                    continue;
                }
                valid = false;
                break;
            };
            if cells[0].is_empty() || end <= start {
                valid = false;
                break;
            }
            periods.push(PublishedPeriod {
                name: cells[0].clone(),
                start,
                end,
            });
        }
        // The day must run forward: each period starts at or after the
        // previous one ends.
        if valid {
            valid = periods.windows(2).all(|w| w[1].start >= w[0].end);
        }
        if valid && periods.len() >= 2 {
            tables.push(PublishedTable {
                group: group.clone(),
                label,
                periods,
            });
        }
    }
    tables
}

/// Hash of the parsed content, used to tell whether a refetch changed anything.
pub fn content_hash(tables: &[PublishedTable]) -> String {
    let mut hasher = Sha256::new();
    for table in tables {
        hasher.update(table.group.as_bytes());
        hasher.update([0]);
        hasher.update(table.label.as_bytes());
        hasher.update([0]);
        for p in &table.periods {
            hasher.update(p.name.as_bytes());
            hasher.update([0]);
            hasher.update(p.start.to_le_bytes());
            hasher.update(p.end.to_le_bytes());
        }
        hasher.update([1]);
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Replace a school's published tables. Returns `false` (and keeps the old
/// rows) when `tables` is empty.
pub fn replace_school(
    store: &DataStore,
    school: &str,
    source_url: &str,
    tables: &[PublishedTable],
    now_ms: i64,
) -> bool {
    if tables.is_empty() {
        return false;
    }
    let mut conn = store.conn();
    let Ok(tx) = conn.transaction() else {
        return false;
    };
    let hash = content_hash(tables);
    let unchanged: bool = tx
        .query_row(
            "SELECT content_hash = ?2 FROM bell_sync_status WHERE school = ?1",
            rusqlite::params![school, hash],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !unchanged {
        let wrote = tx
            .execute(
                "DELETE FROM bell_published_periods WHERE school = ?1",
                rusqlite::params![school],
            )
            .is_ok()
            && tables.iter().enumerate().all(|(pos, table)| {
                table.periods.iter().enumerate().all(|(seq, p)| {
                    tx.execute(
                        "INSERT INTO bell_published_periods
                         (school, table_pos, group_label, table_label, seq, name, start_min, end_min)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        rusqlite::params![
                            school,
                            pos as i64,
                            table.group,
                            table.label,
                            seq as i64,
                            p.name,
                            p.start,
                            p.end
                        ],
                    )
                    .is_ok()
                })
            });
        if !wrote {
            return false;
        }
    }
    let saved = tx
        .execute(
            "INSERT INTO bell_sync_status
             (school, source_url, checked_at, fetched_at, ok, error, content_hash)
             VALUES (?1, ?2, ?3, ?3, 1, '', ?4)
             ON CONFLICT(school) DO UPDATE SET source_url = ?2, checked_at = ?3,
               fetched_at = ?3, ok = 1, error = '', content_hash = ?4",
            rusqlite::params![school, source_url, now_ms, hash],
        )
        .is_ok();
    saved && tx.commit().is_ok()
}

/// Record a failed attempt without touching the stored schedule.
pub fn record_failure(store: &DataStore, school: &str, source_url: &str, error: &str, now_ms: i64) {
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO bell_sync_status
         (school, source_url, checked_at, fetched_at, ok, error, content_hash)
         VALUES (?1, ?2, ?3, 0, 0, ?4, '')
         ON CONFLICT(school) DO UPDATE SET source_url = ?2, checked_at = ?3,
           ok = 0, error = ?4",
        rusqlite::params![school, source_url, now_ms, error],
    );
}

/// The stored sync status for a school, if it was ever attempted.
pub fn load_status(store: &DataStore, school: &str) -> Option<SyncStatus> {
    let conn = store.conn();
    conn.query_row(
        "SELECT source_url, checked_at, fetched_at, ok, error
         FROM bell_sync_status WHERE school = ?1",
        rusqlite::params![school],
        |r| {
            Ok(SyncStatus {
                source_url: r.get(0)?,
                checked_at: r.get(1)?,
                fetched_at: r.get(2)?,
                ok: r.get::<_, i64>(3)? != 0,
                error: r.get(4)?,
            })
        },
    )
    .ok()
}

/// The stored tables for a school, in page order.
pub fn load_school(store: &DataStore, school: &str) -> Vec<PublishedTable> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(
        "SELECT table_pos, group_label, table_label, name, start_min, end_min
         FROM bell_published_periods WHERE school = ?1 ORDER BY table_pos, seq",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map(rusqlite::params![school], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            PublishedPeriod {
                name: r.get(3)?,
                start: r.get(4)?,
                end: r.get(5)?,
            },
        ))
    }) else {
        return Vec::new();
    };
    let mut tables: Vec<(i64, PublishedTable)> = Vec::new();
    for (pos, group, label, period) in rows.flatten() {
        match tables.last_mut() {
            Some((last, table)) if *last == pos => table.periods.push(period),
            _ => tables.push((
                pos,
                PublishedTable {
                    group,
                    label,
                    periods: vec![period],
                },
            )),
        }
    }
    tables.into_iter().map(|(_, t)| t).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"
    <h2 class="fsElementTitle" id="a">Leaving our website</h2>
    <h2 class="fsElementTitle" id="b">Regular Two Lunch Schedules</h2>
    <h2 class="fsElementTitle fsA11yElementTitleIconInSpan" id="c"><span></span><a href='#x'>Monday Schedule</a></h2>
    <table><caption><h2>1st&nbsp;Lunch</h2></caption>
      <thead><tr><th>Period / Description</th><th>Start Time</th><th>End Time</th></tr></thead>
      <tbody>
        <tr><td>Period 1</td><td>8:30</td><td>9:52</td></tr>
        <tr><td>Lunch</td><td>11:22</td><td>11:52</td></tr>
        <tr><td>Period 3</td><td>12:00</td><td>1:22</td></tr>
        <tr><td>Pack</td><td>2:52</td><td>3:20</td></tr>
      </tbody></table>
    <h2 class="fsElementTitle" id="d">Tuesday Schedule</h2>
    <table><caption>Broken</caption>
      <tr><td>Period 1</td><td>8:30</td><td>9:52</td></tr>
      <tr><td>Period 2</td><td>9:00</td><td>10:00</td></tr></table>
    "#;

    #[test]
    fn clock_uses_school_day_hours() {
        assert_eq!(parse_clock("8:30"), Some(510));
        assert_eq!(parse_clock("11:52"), Some(712));
        assert_eq!(parse_clock("12:00"), Some(720));
        assert_eq!(parse_clock("1:22"), Some(13 * 60 + 22));
        assert_eq!(parse_clock("3:20"), Some(15 * 60 + 20));
        assert_eq!(parse_clock("13:00"), None);
        assert_eq!(parse_clock("8:75"), None);
        assert_eq!(parse_clock("Start Time"), None);
    }

    #[test]
    fn parses_groups_captions_and_rows() {
        let tables = parse_bell_page(PAGE);
        assert_eq!(tables.len(), 1, "overlapping table must be rejected");
        let t = &tables[0];
        assert_eq!(t.group, "Monday Schedule");
        assert_eq!(t.label, "1st Lunch");
        assert_eq!(t.periods.len(), 4);
        assert_eq!(t.periods[2].name, "Period 3");
        assert_eq!((t.periods[2].start, t.periods[2].end), (720, 802));
        assert_eq!(t.periods[3].end, 15 * 60 + 20);
    }

    #[test]
    fn empty_or_unrelated_pages_yield_nothing() {
        assert!(parse_bell_page("<html><body>hi</body></html>").is_empty());
    }

    fn temp_store() -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-bell-sync-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn storage_round_trips_and_keeps_last_good_data() {
        let store = temp_store();
        let tables = parse_bell_page(PAGE);
        assert!(replace_school(
            &store,
            "woodcreek",
            "https://x/y",
            &tables,
            1000
        ));
        assert_eq!(load_school(&store, "woodcreek"), tables);
        assert!(load_school(&store, "oakmont").is_empty());

        // An empty parse must not wipe the stored schedule.
        assert!(!replace_school(
            &store,
            "woodcreek",
            "https://x/y",
            &[],
            2000
        ));
        record_failure(&store, "woodcreek", "https://x/y", "boom", 3000);
        assert_eq!(load_school(&store, "woodcreek"), tables);
        let status = load_status(&store, "woodcreek").unwrap();
        assert!(!status.ok);
        assert_eq!(status.fetched_at, 1000);
        assert_eq!(status.checked_at, 3000);
        assert_eq!(status.error, "boom");

        // A changed page replaces the rows.
        let mut changed = tables.clone();
        changed[0].periods[0].end -= 5;
        assert!(replace_school(
            &store,
            "woodcreek",
            "https://x/y",
            &changed,
            4000
        ));
        assert_eq!(load_school(&store, "woodcreek"), changed);
        assert!(load_status(&store, "woodcreek").unwrap().ok);
    }

    #[test]
    fn hash_changes_with_times() {
        let mut tables = parse_bell_page(PAGE);
        let before = content_hash(&tables);
        tables[0].periods[0].end += 1;
        assert_ne!(before, content_hash(&tables));
    }
}
