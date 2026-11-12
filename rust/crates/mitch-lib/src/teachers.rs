//! Rate My Teacher: the teacher list, anonymous star ratings, and the written
//! notes moderators review. Overview and API in `docs/rate-my-teacher.md`.
//!
//! # How ratings stay anonymous but one-per-person
//!
//! A rating row holds the teacher, the stars and a calendar day. It has no
//! account, session or exact time. To stop a second rating of the same
//! teacher we keep a separate table of *marks*, one per (account, teacher):
//! `HMAC(key, normalized email || 0x00 || teacher id)`. The key is derived
//! from the server's id secret for this one purpose.
//!
//! - A mark says "someone with this account has rated this teacher". It is
//!   not tied to a rating row, so nobody can tell which rating is whose.
//! - The marks table is `WITHOUT ROWID` and keyed by the hash, so it keeps
//!   no insertion order that could be lined up against the ratings table.
//! - Notes are a third, separate table with the same day-only timestamp.
//! - Anyone holding both the database and the id secret could test a guess
//!   ("did this student rate that teacher?"), the same trust already placed
//!   in the server for everything else, but the database alone cannot be
//!   reversed into a list of who rated whom.
//!
//! Stars are published immediately. A note starts `pending` and is shown
//! only after a moderator approves it.

#![allow(clippy::expect_used)] // only on compile-time-constant regexes

use crate::data::DataStore;
use regex::Regex;

pub const MAX_NOTE: usize = 500;
pub const MAX_NAME: usize = 80;
pub const MAX_DEPARTMENT: usize = 80;
pub const COMMENT_STATUSES: &[&str] = &["pending", "approved", "rejected"];
/// Most notes one moderation list call returns.
pub const MODERATION_LIMIT: i64 = 300;
/// Most approved notes shown on a teacher's page.
pub const PUBLIC_NOTE_LIMIT: i64 = 50;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Teacher {
    pub id: i64,
    pub school: String,
    pub name: String,
    pub department: String,
}

/// Stars given in one rating. Each is 1 to 5; categories are optional.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stars {
    pub overall: i64,
    pub homework: Option<i64>,
    pub teaching: Option<i64>,
    pub chill: Option<i64>,
}

impl Stars {
    /// `false` when any given value is outside 1 to 5.
    pub fn valid(&self) -> bool {
        let ok = |v: i64| (1..=5).contains(&v);
        ok(self.overall)
            && [self.homework, self.teaching, self.chill]
                .iter()
                .flatten()
                .all(|v| ok(*v))
    }
}

/// Averages for one teacher. `None` for a category nobody scored.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub count: i64,
    pub overall: Option<f64>,
    pub homework: Option<f64>,
    pub teaching: Option<f64>,
    pub chill: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RateError {
    NoSuchTeacher,
    InvalidStars,
    AlreadyRated,
    Storage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub id: i64,
    pub teacher_id: i64,
    pub teacher_name: String,
    pub school: String,
    pub text: String,
    /// Days since 1970-01-01 (UTC). Never a precise time.
    pub day: i64,
    pub status: String,
    pub reviewed_by: String,
}

/// Days since the Unix epoch for a millisecond timestamp.
pub fn day_of(ms: i64) -> i64 {
    ms.div_euclid(86_400_000)
}

/// The key the one-per-person marks are hashed with.
pub fn mark_key(id_secret: &[u8]) -> [u8; 32] {
    crate::crypto::hmac_sha256(id_secret, b"mitch-teacher-rating-mark-v1")
}

/// The mark for this account and teacher. `norm_email` must already be
/// normalized (`auth::normalize_email`) so alias addresses count as one person.
pub fn mark_for(key: &[u8], norm_email: &str, teacher_id: i64) -> String {
    let data = format!("{}\0{}", norm_email.to_lowercase(), teacher_id);
    crate::crypto::hmac_sha256_hex(key, data.as_bytes())
}

fn clean_text(s: &str, max: usize) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

/// Tidies a teacher name: collapses spaces and turns "Last, First" around.
pub fn clean_name(raw: &str) -> String {
    let name = clean_text(raw, MAX_NAME);
    match name.split_once(',') {
        Some((last, first))
            if !last.trim().is_empty() && !first.trim().is_empty() && !first.contains(',') =>
        {
            format!("{} {}", first.trim(), last.trim())
        }
        _ => name,
    }
}

fn teacher_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Teacher> {
    Ok(Teacher {
        id: r.get(0)?,
        school: r.get(1)?,
        name: r.get(2)?,
        department: r.get(3)?,
    })
}

const TEACHER_COLS: &str = "id, school, name, department";

/// Adds a teacher, or fills in the department of one already listed. Returns
/// `true` when a new teacher was added.
pub fn add_teacher(
    store: &DataStore,
    school: &str,
    name: &str,
    department: &str,
    source: &str,
) -> bool {
    let name = clean_name(name);
    if name.chars().count() < 3 {
        return false;
    }
    let department = clean_text(department, MAX_DEPARTMENT);
    let conn = store.conn();
    let added = conn
        .execute(
            "INSERT OR IGNORE INTO teachers (school, name, department, source) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![school, name, department, source],
        )
        .map(|n| n > 0)
        .unwrap_or(false);
    if !added && !department.is_empty() {
        let _ = conn.execute(
            "UPDATE teachers SET department = ?3 WHERE school = ?1 AND name = ?2 AND department = ''",
            rusqlite::params![school, name, department],
        );
    }
    added
}

/// Adds many `(name, department)` pairs. Returns how many were new.
pub fn add_many(store: &DataStore, school: &str, list: &[(String, String)], source: &str) -> usize {
    list.iter()
        .filter(|(n, d)| add_teacher(store, school, n, d, source))
        .count()
}

/// Removes a teacher with their ratings and notes.
pub fn remove_teacher(store: &DataStore, id: i64) -> bool {
    let conn = store.conn();
    let p = rusqlite::params![id];
    let _ = conn.execute("DELETE FROM teacher_ratings WHERE teacher_id = ?1", p);
    let _ = conn.execute("DELETE FROM teacher_comments WHERE teacher_id = ?1", p);
    conn.execute("DELETE FROM teachers WHERE id = ?1", p)
        .map(|n| n > 0)
        .unwrap_or(false)
}

pub fn get_teacher(store: &DataStore, id: i64) -> Option<Teacher> {
    store
        .conn()
        .query_row(
            &format!("SELECT {TEACHER_COLS} FROM teachers WHERE id = ?1"),
            rusqlite::params![id],
            teacher_row,
        )
        .ok()
}

/// Teachers at a school, by name. `query` filters by a case-insensitive part
/// of the name or department.
pub fn list_teachers(store: &DataStore, school: &str, query: &str) -> Vec<Teacher> {
    let conn = store.conn();
    let like = format!("%{}%", query.trim().replace(['%', '_'], ""));
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {TEACHER_COLS} FROM teachers WHERE school = ?1 AND (name LIKE ?2 OR department LIKE ?2) ORDER BY name COLLATE NOCASE LIMIT 1000"
    )) else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![school, like], teacher_row)
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// Number of teachers listed at a school.
pub fn count_teachers(store: &DataStore, school: &str) -> i64 {
    store
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM teachers WHERE school = ?1",
            rusqlite::params![school],
            |r| r.get(0),
        )
        .unwrap_or(0)
}

/// Averages for every teacher at a school, keyed by teacher id.
pub fn summaries_for_school(
    store: &DataStore,
    school: &str,
) -> std::collections::HashMap<i64, Summary> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(
        "SELECT r.teacher_id, COUNT(*), AVG(r.overall), AVG(r.homework), AVG(r.teaching), AVG(r.chill)
         FROM teacher_ratings r JOIN teachers t ON t.id = r.teacher_id
         WHERE t.school = ?1 GROUP BY r.teacher_id",
    ) else {
        return Default::default();
    };
    stmt.query_map(rusqlite::params![school], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            Summary {
                count: r.get(1)?,
                overall: r.get(2)?,
                homework: r.get(3)?,
                teaching: r.get(4)?,
                chill: r.get(5)?,
            },
        ))
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

pub fn summary(store: &DataStore, teacher_id: i64) -> Summary {
    store
        .conn()
        .query_row(
            "SELECT COUNT(*), AVG(overall), AVG(homework), AVG(teaching), AVG(chill) FROM teacher_ratings WHERE teacher_id = ?1",
            rusqlite::params![teacher_id],
            |r| Ok(Summary { count: r.get(0)?, overall: r.get(1)?, homework: r.get(2)?, teaching: r.get(3)?, chill: r.get(4)? }),
        )
        .unwrap_or_default()
}

/// Whether this account has already rated this teacher.
pub fn has_rated(store: &DataStore, key: &[u8], norm_email: &str, teacher_id: i64) -> bool {
    store
        .conn()
        .query_row(
            "SELECT 1 FROM teacher_rating_marks WHERE mark = ?1",
            rusqlite::params![mark_for(key, norm_email, teacher_id)],
            |_| Ok(()),
        )
        .is_ok()
}

/// Records a rating, and the note if there is one. The one-per-teacher check
/// is the insert into the marks table (its primary key), so two requests at
/// once cannot both succeed.
pub fn rate(
    store: &DataStore,
    key: &[u8],
    norm_email: &str,
    teacher_id: i64,
    stars: Stars,
    note: &str,
    now_ms: i64,
) -> Result<(), RateError> {
    if !stars.valid() {
        return Err(RateError::InvalidStars);
    }
    if get_teacher(store, teacher_id).is_none() {
        return Err(RateError::NoSuchTeacher);
    }
    let mark = mark_for(key, norm_email, teacher_id);
    let day = day_of(now_ms);
    let conn = store.conn();
    let fresh = conn
        .execute(
            "INSERT OR IGNORE INTO teacher_rating_marks (mark) VALUES (?1)",
            rusqlite::params![mark],
        )
        .map_err(|_| RateError::Storage)?;
    if fresh == 0 {
        return Err(RateError::AlreadyRated);
    }
    let saved = conn.execute(
        "INSERT INTO teacher_ratings (teacher_id, overall, homework, teaching, chill, day) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![teacher_id, stars.overall, stars.homework, stars.teaching, stars.chill, day],
    );
    if saved.is_err() {
        // Free the mark again so the person can try once more.
        let _ = conn.execute(
            "DELETE FROM teacher_rating_marks WHERE mark = ?1",
            rusqlite::params![mark],
        );
        return Err(RateError::Storage);
    }
    let note = clean_text(note, MAX_NOTE);
    if !note.is_empty() {
        let _ = conn.execute(
            "INSERT INTO teacher_comments (teacher_id, text, day) VALUES (?1, ?2, ?3)",
            rusqlite::params![teacher_id, note, day],
        );
    }
    Ok(())
}

fn note_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Note> {
    Ok(Note {
        id: r.get(0)?,
        teacher_id: r.get(1)?,
        teacher_name: r.get(2)?,
        school: r.get(3)?,
        text: r.get(4)?,
        day: r.get(5)?,
        status: r.get(6)?,
        reviewed_by: r.get(7)?,
    })
}

const NOTE_SELECT: &str =
    "SELECT c.id, c.teacher_id, t.name, t.school, c.text, c.day, c.status, c.reviewed_by
    FROM teacher_comments c JOIN teachers t ON t.id = c.teacher_id";

/// Approved notes for a teacher, newest first.
pub fn approved_notes(store: &DataStore, teacher_id: i64) -> Vec<Note> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(&format!(
        "{NOTE_SELECT} WHERE c.teacher_id = ?1 AND c.status = 'approved' ORDER BY c.id DESC LIMIT ?2"
    )) else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![teacher_id, PUBLIC_NOTE_LIMIT], note_row)
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// Notes for moderators. `status` is `pending`, `approved`, `rejected`, or
/// anything else for all. Oldest first for pending so the queue is fair.
pub fn list_notes(store: &DataStore, status: &str) -> Vec<Note> {
    let conn = store.conn();
    let (filter, order) = if COMMENT_STATUSES.contains(&status) {
        (status, if status == "pending" { "ASC" } else { "DESC" })
    } else {
        ("%", "DESC")
    };
    let Ok(mut stmt) = conn.prepare(&format!(
        "{NOTE_SELECT} WHERE c.status LIKE ?1 ORDER BY c.id {order} LIMIT ?2"
    )) else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![filter, MODERATION_LIMIT], note_row)
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// `(pending, approved, rejected)` note counts.
pub fn note_counts(store: &DataStore) -> (i64, i64, i64) {
    let conn = store.conn();
    let count = |status: &str| -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM teacher_comments WHERE status = ?1",
            rusqlite::params![status],
            |r| r.get(0),
        )
        .unwrap_or(0)
    };
    (count("pending"), count("approved"), count("rejected"))
}

/// Approves, rejects, or reopens a note. `false` for an unknown note or status.
pub fn set_note_status(store: &DataStore, id: i64, status: &str, by: &str, now_ms: i64) -> bool {
    if !COMMENT_STATUSES.contains(&status) {
        return false;
    }
    let (by, at) = if status == "pending" {
        ("", None)
    } else {
        (by, Some(now_ms))
    };
    store
        .conn()
        .execute(
            "UPDATE teacher_comments SET status = ?2, reviewed_by = ?3, reviewed_at = ?4 WHERE id = ?1",
            rusqlite::params![id, status, by, at],
        )
        .map(|n| n > 0)
        .unwrap_or(false)
}

pub fn delete_note(store: &DataStore, id: i64) -> bool {
    store
        .conn()
        .execute(
            "DELETE FROM teacher_comments WHERE id = ?1",
            rusqlite::params![id],
        )
        .map(|n| n > 0)
        .unwrap_or(false)
}

// ── Reading teacher lists ────────────────────────────────────────────────

fn strip_tags(html: &str) -> String {
    static TAG: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let tag = TAG.get_or_init(|| Regex::new(r"(?s)<[^>]*>").expect("static regex"));
    let text = tag.replace_all(html, " ");
    text.replace("&amp;", "&")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&rsquo;", "'")
        .replace("&quot;", "\"")
        .replace("&nbsp;", " ")
}

/// Titles of people who are not classroom teachers, so a directory that lists
/// everyone doesn't fill the rating list with the front office.
const NON_TEACHING: &[&str] = &[
    "principal",
    "counselor",
    "secretary",
    "clerk",
    "custodian",
    "nurse",
    "aide",
    "librarian",
    "psychologist",
    "coordinator",
    "technician",
    "registrar",
    "administrator",
    "director",
    "manager",
    "supervisor",
    "receptionist",
    "bookkeeper",
    "attendance",
    "campus",
    "security",
    "cafeteria",
    "food service",
    "maintenance",
    "specialist",
    "dean",
    "athletic",
    "assistant",
];

/// Whether a directory title belongs to someone who isn't a teacher. An empty
/// title is kept: the page may simply not show one.
pub fn is_non_teaching(title: &str) -> bool {
    let t = title.to_lowercase();
    !t.contains("teacher") && NON_TEACHING.iter().any(|w| t.contains(w))
}

/// Turns a directory title ("Teacher - Mathematics", "Science Teacher") into
/// the subject ("Mathematics", "Science").
pub fn subject_from_title(title: &str) -> String {
    let generic = [
        "teacher",
        "instructor",
        "faculty",
        "staff",
        "certificated",
        "classroom teacher",
    ];
    let parts: Vec<String> = title
        .split(['|', ',', ';', '/'])
        .flat_map(|p| p.split(" - "))
        .map(|p| {
            let p = p.trim();
            let lower = p.to_lowercase();
            ["teacher", "instructor"]
                .iter()
                .find_map(|w| {
                    lower
                        .strip_suffix(w)
                        .map(|rest| p[..rest.len()].trim().to_string())
                })
                .unwrap_or_else(|| p.to_string())
        })
        .filter(|p| !p.is_empty() && !generic.contains(&p.to_lowercase().as_str()))
        .collect();
    clean_text(&parts.join(", "), MAX_DEPARTMENT)
}

/// When a school's directory was last read: `(time, how many found, error)`.
pub fn sync_status(store: &DataStore, school: &str) -> Option<(i64, i64, String)> {
    store
        .conn()
        .query_row(
            "SELECT last_at, found, error FROM teacher_sync_status WHERE school = ?1",
            rusqlite::params![school],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok()
}

pub fn record_sync(store: &DataStore, school: &str, now_ms: i64, found: i64, error: &str) {
    let _ = store.conn().execute(
        "INSERT INTO teacher_sync_status (school, last_at, found, error) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(school) DO UPDATE SET last_at = ?2, found = ?3, error = ?4",
        rusqlite::params![school, now_ms, found, error],
    );
}

/// Whether the automatic sync should read this school again: never read, a
/// good read older than `ok_age_ms`, or a failed one older than `retry_ms`.
pub fn sync_due(
    store: &DataStore,
    school: &str,
    now_ms: i64,
    ok_age_ms: i64,
    retry_ms: i64,
) -> bool {
    match sync_status(store, school) {
        None => true,
        Some((at, _, err)) if err.is_empty() => now_ms - at >= ok_age_ms,
        Some((at, ..)) => now_ms - at >= retry_ms,
    }
}

/// Reads `(name, department)` pairs out of a school's Finalsite staff
/// directory page. Each person is a block with class `fsConstituentItem`; the
/// name is the element with class `fsFullName` and the department or title
/// comes from `fsDepartment`/`fsTitle` when present. Returns nothing when the
/// page has no such blocks (for instance when the directory loads by script),
/// so a changed page never produces junk.
pub fn parse_finalsite_directory(html: &str) -> Vec<(String, String)> {
    static NAME: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static DEPT: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let name_re = NAME.get_or_init(|| {
        Regex::new(r#"(?s)class="[^"]*\bfsFullName\b[^"]*"[^>]*>(.*?)</(?:h\d|div|span|a|p)>"#)
            .expect("static regex")
    });
    let dept_re = DEPT.get_or_init(|| {
        Regex::new(
            r#"(?s)class="[^"]*\bfs(?:Department|Title)s?\b[^"]*"[^>]*>(.*?)</(?:div|span|p|li)>"#,
        )
        .expect("static regex")
    });
    let mut out: Vec<(String, String)> = Vec::new();
    for chunk in html.split("fsConstituentItem").skip(1) {
        let Some(name) = name_re
            .captures(chunk)
            .map(|c| clean_name(&strip_tags(&c[1])))
        else {
            continue;
        };
        let title = dept_re
            .captures(chunk)
            .map(|c| clean_text(&strip_tags(&c[1]), 160))
            .unwrap_or_default();
        if is_non_teaching(&title) {
            continue;
        }
        let dept = subject_from_title(&title);
        if name.chars().count() >= 3 && !out.iter().any(|(n, _)| *n == name) {
            out.push((name, dept));
        }
    }
    out
}

/// Reads a pasted list: one teacher per line, `Name` or `Name | Department`
/// (a tab or a comma-less ` - ` also separates). Blank lines are skipped.
pub fn parse_pasted_list(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let (name, dept) = match line
            .split_once('|')
            .or_else(|| line.split_once('\t'))
            .or_else(|| line.split_once(" - "))
        {
            Some((n, d)) => (n, d),
            None => (line, ""),
        };
        let name = clean_name(name);
        if name.chars().count() >= 3 && !out.iter().any(|(n, _)| *n == name) {
            out.push((name, clean_text(dept, MAX_DEPARTMENT)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-teachers-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    const KEY: &[u8] = b"test-secret";

    fn stars(o: i64) -> Stars {
        Stars {
            overall: o,
            homework: Some(4),
            teaching: Some(5),
            chill: None,
        }
    }

    #[test]
    fn names_are_tidied_and_deduplicated() {
        let store = temp_store("names");
        assert!(add_teacher(
            &store,
            "woodcreek",
            "Smith,  Jane",
            "Math",
            "manual"
        ));
        assert!(!add_teacher(&store, "woodcreek", "Jane Smith", "", "sync"));
        assert!(add_teacher(&store, "oakmont", "Jane Smith", "", "manual"));
        assert!(!add_teacher(&store, "woodcreek", "x", "", "manual"));
        let list = list_teachers(&store, "woodcreek", "");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Jane Smith");
        assert_eq!(list[0].department, "Math");
        assert_eq!(list_teachers(&store, "woodcreek", "math").len(), 1);
        assert!(list_teachers(&store, "woodcreek", "zzz").is_empty());
    }

    #[test]
    fn a_person_rates_each_teacher_once_and_averages_update() {
        let store = temp_store("rate");
        add_teacher(&store, "woodcreek", "Jane Smith", "", "manual");
        add_teacher(&store, "woodcreek", "Raj Patel", "", "manual");
        let (a, b) = (
            list_teachers(&store, "woodcreek", "Jane")[0].id,
            list_teachers(&store, "woodcreek", "Raj")[0].id,
        );
        assert_eq!(
            rate(&store, KEY, "ab@student.rjuhsd.us", a, stars(5), "", 1_000),
            Ok(())
        );
        assert_eq!(
            rate(&store, KEY, "ab@student.rjuhsd.us", a, stars(1), "", 2_000),
            Err(RateError::AlreadyRated)
        );
        assert_eq!(
            rate(&store, KEY, "AB@student.rjuhsd.us", a, stars(1), "", 2_000),
            Err(RateError::AlreadyRated)
        );
        assert_eq!(
            rate(&store, KEY, "ab@student.rjuhsd.us", b, stars(2), "", 3_000),
            Ok(())
        );
        assert_eq!(
            rate(&store, KEY, "cd@student.rjuhsd.us", a, stars(3), "", 4_000),
            Ok(())
        );
        assert!(has_rated(&store, KEY, "ab@student.rjuhsd.us", a));
        assert!(!has_rated(&store, KEY, "zz@student.rjuhsd.us", a));
        let s = summary(&store, a);
        assert_eq!((s.count, s.overall, s.chill), (2, Some(4.0), None));
        assert_eq!(s.teaching, Some(5.0));
        assert_eq!(summaries_for_school(&store, "woodcreek")[&b].count, 1);
    }

    #[test]
    fn bad_stars_and_unknown_teachers_are_refused_without_using_up_the_rating() {
        let store = temp_store("bad");
        add_teacher(&store, "woodcreek", "Jane Smith", "", "manual");
        let id = list_teachers(&store, "woodcreek", "")[0].id;
        for bad in [0, 6, -1] {
            assert_eq!(
                rate(&store, KEY, "ab@x.test", id, stars(bad), "", 1),
                Err(RateError::InvalidStars)
            );
        }
        let off = Stars {
            overall: 3,
            homework: Some(9),
            ..Default::default()
        };
        assert_eq!(
            rate(&store, KEY, "ab@x.test", id, off, "", 1),
            Err(RateError::InvalidStars)
        );
        assert_eq!(
            rate(&store, KEY, "ab@x.test", 999, stars(3), "", 1),
            Err(RateError::NoSuchTeacher)
        );
        assert_eq!(rate(&store, KEY, "ab@x.test", id, stars(3), "", 1), Ok(()));
    }

    #[test]
    fn nothing_stored_links_a_rating_to_an_account() {
        let store = temp_store("anon");
        add_teacher(&store, "woodcreek", "Jane Smith", "", "manual");
        let id = list_teachers(&store, "woodcreek", "")[0].id;
        rate(
            &store,
            KEY,
            "ab@student.rjuhsd.us",
            id,
            stars(4),
            "Fair grader",
            86_400_000 * 5 + 777,
        )
        .unwrap();
        let conn = store.conn();
        // The rating and note carry a whole day, not a time of day.
        let day: i64 = conn
            .query_row("SELECT day FROM teacher_ratings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(day, 5);
        // Neither table has a column that could name the person.
        for table in [
            "teacher_ratings",
            "teacher_comments",
            "teacher_rating_marks",
        ] {
            let mut stmt = conn
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap();
            let cols: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(1))
                .unwrap()
                .flatten()
                .collect();
            assert!(
                cols.iter()
                    .all(|c| !["email", "account", "sid", "user", "created_at"]
                        .contains(&c.as_str())),
                "{table}: {cols:?}"
            );
        }
        // The mark is a keyed hash, so it does not contain the address.
        let mark: String = conn
            .query_row("SELECT mark FROM teacher_rating_marks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mark.len(), 64);
        assert!(
            !mark.contains("student"),
            "hash should not echo the address"
        );
        // A different key (another server) produces a different mark.
        assert_ne!(
            mark_for(&mark_key(b"a"), "ab@student.rjuhsd.us", id),
            mark_for(&mark_key(b"b"), "ab@student.rjuhsd.us", id)
        );
    }

    #[test]
    fn notes_wait_for_a_moderator_before_they_show() {
        let store = temp_store("notes");
        add_teacher(&store, "woodcreek", "Jane Smith", "", "manual");
        let id = list_teachers(&store, "woodcreek", "")[0].id;
        rate(
            &store,
            KEY,
            "ab@x.test",
            id,
            stars(5),
            "  Great\n  class  ",
            1,
        )
        .unwrap();
        rate(&store, KEY, "cd@x.test", id, stars(5), "", 1).unwrap();
        assert_eq!(note_counts(&store), (1, 0, 0));
        assert!(approved_notes(&store, id).is_empty());
        let pending = list_notes(&store, "pending");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].text, "Great class");
        assert_eq!(pending[0].teacher_name, "Jane Smith");
        assert!(!set_note_status(&store, pending[0].id, "weird", "m", 2));
        assert!(set_note_status(
            &store,
            pending[0].id,
            "approved",
            "mod@x.test",
            2
        ));
        assert_eq!(approved_notes(&store, id)[0].text, "Great class");
        assert!(set_note_status(
            &store,
            pending[0].id,
            "rejected",
            "mod@x.test",
            3
        ));
        assert!(approved_notes(&store, id).is_empty());
        assert!(set_note_status(&store, pending[0].id, "pending", "", 4));
        assert_eq!(note_counts(&store), (1, 0, 0));
        assert!(delete_note(&store, pending[0].id));
        assert_eq!(note_counts(&store), (0, 0, 0));
    }

    #[test]
    fn long_notes_are_capped_and_removing_a_teacher_removes_their_data() {
        let store = temp_store("cap");
        add_teacher(&store, "woodcreek", "Jane Smith", "", "manual");
        let id = list_teachers(&store, "woodcreek", "")[0].id;
        rate(&store, KEY, "ab@x.test", id, stars(5), &"n".repeat(900), 1).unwrap();
        assert_eq!(list_notes(&store, "all")[0].text.chars().count(), MAX_NOTE);
        assert!(remove_teacher(&store, id));
        assert!(!remove_teacher(&store, id));
        assert_eq!(note_counts(&store), (0, 0, 0));
        assert_eq!(summary(&store, id).count, 0);
    }

    #[test]
    fn finalsite_directory_blocks_are_read_and_junk_pages_give_nothing() {
        let html = r#"<ul>
          <li class="fsConstituentItem"><h3 class="fsFullName"><a href="/x">Smith, Jane</a></h3>
            <div class="fsTitles"><div class="fsTitle">Mathematics &amp; Science</div></div></li>
          <li class="fsConstituentItem"><h3 class="fsFullName">Raj Patel</h3></li>
          <li class="fsConstituentItem"><h3 class="fsFullName">Raj Patel</h3></li>
        </ul>"#;
        let got = parse_finalsite_directory(html);
        assert_eq!(
            got,
            vec![
                (
                    "Jane Smith".to_string(),
                    "Mathematics & Science".to_string()
                ),
                ("Raj Patel".to_string(), String::new())
            ]
        );
        assert!(parse_finalsite_directory("<html><body>Loading…</body></html>").is_empty());
    }

    #[test]
    fn titles_become_subjects_and_office_staff_are_skipped() {
        assert_eq!(subject_from_title("Teacher - Mathematics"), "Mathematics");
        assert_eq!(subject_from_title("Science Teacher"), "Science");
        assert_eq!(subject_from_title("Teacher"), "");
        assert_eq!(
            subject_from_title("English Teacher, Journalism"),
            "English, Journalism"
        );
        assert!(is_non_teaching("Assistant Principal"));
        assert!(is_non_teaching("Attendance Clerk"));
        assert!(!is_non_teaching("Math Teacher"));
        assert!(!is_non_teaching(""));
        let html = r#"<li class="fsConstituentItem"><h3 class="fsFullName">Sam Cho</h3><div class="fsTitle">Counselor</div></li>
          <li class="fsConstituentItem"><h3 class="fsFullName">Lee Ann</h3><div class="fsTitle">Teacher - Art</div></li>"#;
        assert_eq!(
            parse_finalsite_directory(html),
            vec![("Lee Ann".to_string(), "Art".to_string())]
        );
    }

    #[test]
    fn sync_is_due_when_never_read_or_stale() {
        let store = temp_store("syncdue");
        let (day, hr) = (86_400_000, 3_600_000);
        assert!(sync_due(&store, "woodcreek", 10 * day, day, 6 * hr));
        record_sync(&store, "woodcreek", 10 * day, 40, "");
        assert!(!sync_due(&store, "woodcreek", 10 * day + hr, day, 6 * hr));
        assert!(sync_due(&store, "woodcreek", 11 * day, day, 6 * hr));
        record_sync(&store, "oakmont", 10 * day, 0, "nope");
        assert!(!sync_due(&store, "oakmont", 10 * day + hr, day, 6 * hr));
        assert!(sync_due(&store, "oakmont", 10 * day + 7 * hr, day, 6 * hr));
    }

    #[test]
    fn pasted_lists_accept_a_few_separators() {
        let got = parse_pasted_list("Jane Smith | Math\n\nLee, Ann\tArt\nSam Cho - PE\nx\n");
        let names: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Jane Smith", "Ann Lee", "Sam Cho"]);
        assert_eq!(got[1].1, "Art");
        assert_eq!(got[2].1, "PE");
    }
}
