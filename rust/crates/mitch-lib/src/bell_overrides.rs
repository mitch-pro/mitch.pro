//! One-day replacement bell schedules, one per school (`bell_overrides` /
//! `bell_override_periods`). Staff set them from `/admin/special-schedule/`
//! (`routes/admin/special_schedule.rs`); the public read side is
//! `GET /api/bell/override` in `routes/misc.rs`. Docs: `docs/bell-overrides.md`.
//!
//! The JSON shape served to the bell pages is unchanged:
//! `{school, date, name, special_event, schedule: {lunch1: [...], lunch2: [...]}}`
//! with each period `{name, start, end, duration}`.
//!
//! Rules enforced here, not in the page: a known school, a real date that is
//! not in the past (Los Angeles time), bounded text, periods with valid
//! `HH:MM` times where the end is after the start. `duration` is always
//! computed from the times, never taken from the request.

use crate::data::DataStore;
use rusqlite::{params, Connection};
use serde_json::{json, Value};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS bell_overrides (
    school TEXT PRIMARY KEY,
    date TEXT NOT NULL,
    name TEXT NOT NULL,
    special_event TEXT,
    set_by TEXT NOT NULL DEFAULT '',
    updated_at INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS bell_override_periods (
    school TEXT NOT NULL REFERENCES bell_overrides(school) ON DELETE CASCADE,
    lunch TEXT NOT NULL CHECK (lunch IN ('lunch1', 'lunch2')),
    position INTEGER NOT NULL,
    name TEXT NOT NULL,
    start TEXT NOT NULL,
    end TEXT NOT NULL,
    duration INTEGER NOT NULL,
    PRIMARY KEY (school, lunch, position)
);
";

pub const SCHOOLS: &[(&str, &str)] = &[
    ("woodcreek", "Woodcreek"),
    ("roseville", "Roseville"),
    ("granitebay", "Granite Bay"),
    ("antelope", "Antelope"),
    ("westpark", "West Park"),
    ("oakmont", "Oakmont"),
];

const MAX_NAME: usize = 80;
const MAX_EVENT: usize = 200;
const MAX_PERIOD_NAME: usize = 60;
const MAX_PERIODS: usize = 20;
/// How far ahead a schedule may be set.
const MAX_DAYS_AHEAD: i64 = 90;

/// Official WHS alternate schedules: `(name, lunch 1 periods, lunch 2 periods)`
/// as `(period, start, end)`.
pub type PresetPeriods = &'static [(&'static str, &'static str, &'static str)];
pub const PRESETS: &[(&str, PresetPeriods, PresetPeriods)] = &[
    (
        "Non-Pack Day",
        &[
            ("Period 1", "08:30", "09:59"),
            ("Period 2", "10:07", "11:36"),
            ("Lunch 1", "11:36", "12:06"),
            ("Period 3", "12:14", "13:43"),
            ("Period 4", "13:51", "15:20"),
        ],
        &[
            ("Period 1", "08:30", "09:59"),
            ("Period 2", "10:07", "11:36"),
            ("Period 3", "11:44", "13:13"),
            ("Lunch 2", "13:13", "13:43"),
            ("Period 4", "13:51", "15:20"),
        ],
    ),
    (
        "Minimum Day",
        &[
            ("Period 1", "08:30", "09:26"),
            ("Period 2", "09:34", "10:30"),
            ("Period 3", "10:38", "11:34"),
            ("Break / Lunch", "11:34", "11:46"),
            ("Period 4", "11:54", "12:50"),
        ],
        &[
            ("Period 1", "08:30", "09:26"),
            ("Period 2", "09:34", "10:30"),
            ("Period 3", "10:38", "11:34"),
            ("Break / Lunch", "11:34", "11:46"),
            ("Period 4", "11:54", "12:50"),
        ],
    ),
    (
        "Finals",
        &[
            ("Period 1/3", "08:30", "10:30"),
            ("Break / Lunch", "10:30", "10:42"),
            ("Period 2/4", "10:50", "12:50"),
        ],
        &[
            ("Period 1/3", "08:30", "10:30"),
            ("Break / Lunch", "10:30", "10:42"),
            ("Period 2/4", "10:50", "12:50"),
        ],
    ),
];

#[derive(Debug, Clone, PartialEq)]
pub struct Period {
    pub name: String,
    pub start: String,
    pub end: String,
    pub duration: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Override {
    pub school: String,
    pub date: String,
    pub name: String,
    pub special_event: Option<String>,
    pub lunch1: Vec<Period>,
    pub lunch2: Vec<Period>,
}

/// An unvalidated period from a request.
#[derive(Debug, Clone)]
pub struct PeriodInput {
    pub name: String,
    pub start: String,
    pub end: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveError {
    UnknownSchool,
    BadDate,
    PastDate,
    TooFarAhead,
    BadName,
    BadEvent,
    NoPeriods,
    TooManyPeriods,
    BadPeriodName,
    BadTime,
    EndBeforeStart,
    Database,
}

pub fn school_label(key: &str) -> Option<&'static str> {
    SCHOOLS.iter().find(|(k, _)| *k == key).map(|(_, l)| *l)
}

/// Minutes since midnight for a strict `HH:MM`.
fn minutes(hhmm: &str) -> Option<i64> {
    let (h, m) = hhmm.split_once(':')?;
    if h.len() != 2 || m.len() != 2 {
        return None;
    }
    let (h, m): (i64, i64) = (h.parse().ok()?, m.parse().ok()?);
    ((0..24).contains(&h) && (0..60).contains(&m)).then_some(h * 60 + m)
}

fn parse_day(date: &str) -> Option<i64> {
    let b = date.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let (y, m, d): (i64, i64, i64) = (
        date[0..4].parse().ok()?,
        date[5..7].parse().ok()?,
        date[8..10].parse().ok()?,
    );
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Days since 1970-01-01; rejects 02-30 and the like by round-tripping.
    let days = days_from_civil(y, m, d);
    (civil_from_days(days) == (y, m, d)).then_some(days)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Today in Los Angeles as `YYYY-MM-DD`.
pub fn today_la() -> String {
    day_la(crate::school::now_millis())
}

fn day_la(ms: i64) -> String {
    let (y, m, d, ..) = crate::school::la_local_parts(ms);
    format!("{y:04}-{m:02}-{d:02}")
}

fn clean(text: &str, max: usize) -> Option<String> {
    let t = text.trim();
    (!t.is_empty() && t.chars().count() <= max && !t.chars().any(char::is_control))
        .then(|| t.to_string())
}

fn build_periods(inputs: &[PeriodInput]) -> Result<Vec<Period>, SaveError> {
    if inputs.is_empty() {
        return Err(SaveError::NoPeriods);
    }
    if inputs.len() > MAX_PERIODS {
        return Err(SaveError::TooManyPeriods);
    }
    let mut out = Vec::with_capacity(inputs.len());
    for p in inputs {
        let name = clean(&p.name, MAX_PERIOD_NAME).ok_or(SaveError::BadPeriodName)?;
        let (s, e) = (
            minutes(p.start.trim()).ok_or(SaveError::BadTime)?,
            minutes(p.end.trim()).ok_or(SaveError::BadTime)?,
        );
        if e <= s {
            return Err(SaveError::EndBeforeStart);
        }
        out.push(Period {
            name,
            start: p.start.trim().to_string(),
            end: p.end.trim().to_string(),
            duration: e - s,
        });
    }
    // HH:MM strings sort the same as the times.
    out.sort_by(|a, b| a.start.cmp(&b.start));
    Ok(out)
}

/// Checks a request and builds the stored override. `today` is the LA date, passed in so tests can fix it.
pub fn validate(
    school: &str,
    date: &str,
    name: &str,
    special_event: &str,
    lunch1: &[PeriodInput],
    lunch2: &[PeriodInput],
    today: &str,
) -> Result<Override, SaveError> {
    if school_label(school).is_none() {
        return Err(SaveError::UnknownSchool);
    }
    let day = parse_day(date).ok_or(SaveError::BadDate)?;
    let today_day = parse_day(today).ok_or(SaveError::BadDate)?;
    if day < today_day {
        return Err(SaveError::PastDate);
    }
    if day - today_day > MAX_DAYS_AHEAD {
        return Err(SaveError::TooFarAhead);
    }
    let name = clean(name, MAX_NAME).ok_or(SaveError::BadName)?;
    let event = special_event.trim();
    let special_event = if event.is_empty() {
        None
    } else {
        Some(clean(event, MAX_EVENT).ok_or(SaveError::BadEvent)?)
    };
    Ok(Override {
        school: school.to_string(),
        date: date.to_string(),
        name,
        special_event,
        lunch1: build_periods(lunch1)?,
        lunch2: build_periods(lunch2)?,
    })
}

fn insert(conn: &Connection, o: &Override, set_by: &str, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM bell_overrides WHERE school = ?1",
        params![o.school],
    )?;
    conn.execute(
        "INSERT INTO bell_overrides (school, date, name, special_event, set_by, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![o.school, o.date, o.name, o.special_event, set_by, now],
    )?;
    for (lunch, periods) in [("lunch1", &o.lunch1), ("lunch2", &o.lunch2)] {
        for (i, p) in periods.iter().enumerate() {
            conn.execute(
                "INSERT INTO bell_override_periods (school, lunch, position, name, start, end, duration)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![o.school, lunch, i as i64, p.name, p.start, p.end, p.duration],
            )?;
        }
    }
    Ok(())
}

/// Saves `o` for its school (replacing that school's current one) and drops
/// other schools' overrides whose day has passed. One transaction.
pub fn set(store: &DataStore, o: &Override, set_by: &str, today: &str) -> Result<(), SaveError> {
    let conn = store.conn();
    let run = || -> rusqlite::Result<()> {
        conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            conn.execute("DELETE FROM bell_overrides WHERE date < ?1", params![today])?;
            insert(&conn, o, set_by, crate::school::now_millis())
        })();
        conn.execute_batch(if result.is_ok() { "COMMIT" } else { "ROLLBACK" })?;
        result
    };
    run().map_err(|_| SaveError::Database)
}

/// Removes one school's override (`Some(key)`) or all of them (`None`).
/// Returns how many were removed.
pub fn clear(store: &DataStore, school: Option<&str>) -> usize {
    let conn = store.conn();
    match school {
        Some(key) => conn.execute("DELETE FROM bell_overrides WHERE school = ?1", params![key]),
        None => conn.execute("DELETE FROM bell_overrides", []),
    }
    .unwrap_or(0)
}

/// Every stored override, by school. Past days are included: the bell pages
/// compare the date themselves.
pub fn list(store: &DataStore) -> Vec<Override> {
    let conn = store.conn();
    let Ok(mut head) = conn
        .prepare("SELECT school, date, name, special_event FROM bell_overrides ORDER BY school")
    else {
        return Vec::new();
    };
    let heads: Vec<(String, String, String, Option<String>)> = head
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();
    let Ok(mut body) = conn.prepare(
        "SELECT name, start, end, duration FROM bell_override_periods
         WHERE school = ?1 AND lunch = ?2 ORDER BY position",
    ) else {
        return Vec::new();
    };
    heads
        .into_iter()
        .map(|(school, date, name, special_event)| {
            let mut periods = |lunch: &str| -> Vec<Period> {
                body.query_map(params![school, lunch], |r| {
                    Ok(Period {
                        name: r.get(0)?,
                        start: r.get(1)?,
                        end: r.get(2)?,
                        duration: r.get(3)?,
                    })
                })
                .map(|rows| rows.flatten().collect())
                .unwrap_or_default()
            };
            let (lunch1, lunch2) = (periods("lunch1"), periods("lunch2"));
            Override {
                school,
                date,
                name,
                special_event,
                lunch1,
                lunch2,
            }
        })
        .collect()
}

fn period_json(p: &Period) -> Value {
    json!({ "name": p.name, "start": p.start, "end": p.end, "duration": p.duration })
}

/// The shape the bell pages read.
pub fn to_json(o: &Override) -> Value {
    json!({
        "school": o.school,
        "date": o.date,
        "name": o.name,
        "special_event": o.special_event,
        "schedule": {
            "lunch1": o.lunch1.iter().map(period_json).collect::<Vec<_>>(),
            "lunch2": o.lunch2.iter().map(period_json).collect::<Vec<_>>(),
        },
    })
}

/// `{"overrides": [...]}`, the document `GET /api/bell/override` builds its reply from.
pub fn document(store: &DataStore) -> Value {
    json!({ "overrides": list(store).iter().map(to_json).collect::<Vec<_>>() })
}

/// Preset schedules for the staff page, in the same shape as an override's `schedule`.
pub fn presets_json() -> Value {
    let periods = |rows: PresetPeriods| -> Vec<Value> {
        rows.iter()
            .map(|(name, start, end)| {
                let d = minutes(end).unwrap_or(0) - minutes(start).unwrap_or(0);
                json!({ "name": name, "start": start, "end": end, "duration": d })
            })
            .collect()
    };
    Value::Array(
        PRESETS
            .iter()
            .map(|(name, l1, l2)| {
                json!({ "name": name, "schedule": { "lunch1": periods(l1), "lunch2": periods(l2) } })
            })
            .collect(),
    )
}

/// Imports `data/bell_overrides.json` once (either the `{"overrides": [...]}`
/// list or the older single Woodcreek object), recorded in `metadata`, so
/// clearing every override later never brings the old file back.
pub(crate) fn import_legacy(conn: &Connection, store: &DataStore) {
    const MARKER: &str = "site_content_imported:bell_overrides";
    if conn
        .query_row(
            "SELECT 1 FROM metadata WHERE key = ?1",
            params![MARKER],
            |_| Ok(()),
        )
        .is_ok()
    {
        return;
    }
    let doc = [
        store.data_dir().join("bell_overrides.json"),
        store.base_dir.join("data/bell_overrides.json"),
    ]
    .iter()
    .find_map(|p| serde_json::from_str::<Value>(&std::fs::read_to_string(p).ok()?).ok())
    .unwrap_or(Value::Null);
    let entries: Vec<Value> = match doc.get("overrides").and_then(Value::as_array) {
        Some(list) => list.clone(),
        None if doc.get("date").is_some() => vec![doc.clone()],
        None => Vec::new(),
    };
    let now = crate::school::now_millis();
    for e in &entries {
        let school = e
            .get("school")
            .and_then(Value::as_str)
            .unwrap_or("woodcreek")
            .to_ascii_lowercase();
        let periods = |key: &str| -> Vec<Period> {
            e["schedule"][key]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| Period {
                    name: p["name"].as_str().unwrap_or("").to_string(),
                    start: p["start"].as_str().unwrap_or("").to_string(),
                    end: p["end"].as_str().unwrap_or("").to_string(),
                    duration: p["duration"].as_i64().unwrap_or(0),
                })
                .collect()
        };
        let o = Override {
            school,
            date: e["date"].as_str().unwrap_or("").to_string(),
            name: e["name"].as_str().unwrap_or("Special Day").to_string(),
            special_event: e["special_event"].as_str().map(str::to_string),
            lunch1: periods("lunch1"),
            lunch2: periods("lunch2"),
        };
        if !o.date.is_empty() {
            let _ = insert(conn, &o, "imported", now);
        }
    }
    let _ = conn.execute(
        "INSERT OR IGNORE INTO metadata (key, value, updated_at) VALUES (?1, '1', ?2)",
        params![MARKER, now],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str, start: &str, end: &str) -> PeriodInput {
        PeriodInput {
            name: name.into(),
            start: start.into(),
            end: end.into(),
        }
    }
    fn day() -> Vec<PeriodInput> {
        vec![
            p("Period 2", "10:00", "11:00"),
            p("Period 1", "08:30", "09:40"),
        ]
    }
    fn check(school: &str, date: &str, name: &str) -> Result<Override, SaveError> {
        validate(school, date, name, "", &day(), &day(), "2026-11-13")
    }
    fn store() -> (DataStore, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "bell-overrides-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap();
        (DataStore::open(&dir, &dir.join("data")).unwrap(), dir)
    }

    #[test]
    fn validation_rules() {
        let ok = check("woodcreek", "2026-11-13", "Rally Day").unwrap();
        // Sorted by start; duration computed from the times.
        assert_eq!(ok.lunch1[0].name, "Period 1");
        assert_eq!(ok.lunch1[0].duration, 70);
        assert_eq!(
            check("nowhere", "2026-11-13", "x"),
            Err(SaveError::UnknownSchool)
        );
        assert_eq!(
            check("woodcreek", "2026-11-12", "x"),
            Err(SaveError::PastDate)
        );
        assert_eq!(
            check("woodcreek", "2027-03-01", "x"),
            Err(SaveError::TooFarAhead)
        );
        assert_eq!(
            check("woodcreek", "2026-02-30", "x"),
            Err(SaveError::BadDate)
        );
        assert_eq!(
            check("woodcreek", "11/13/2026", "x"),
            Err(SaveError::BadDate)
        );
        assert_eq!(
            check("woodcreek", "2026-11-13", "  "),
            Err(SaveError::BadName)
        );
        assert_eq!(
            validate(
                "woodcreek",
                "2026-11-13",
                "x",
                "",
                &[p("a", "9:00", "10:00")],
                &day(),
                "2026-11-13"
            ),
            Err(SaveError::BadTime)
        );
        assert_eq!(
            validate(
                "woodcreek",
                "2026-11-13",
                "x",
                "",
                &[p("a", "10:00", "10:00")],
                &day(),
                "2026-11-13"
            ),
            Err(SaveError::EndBeforeStart)
        );
        assert_eq!(
            validate(
                "woodcreek",
                "2026-11-13",
                "x",
                "",
                &[],
                &day(),
                "2026-11-13"
            ),
            Err(SaveError::NoPeriods)
        );
        assert_eq!(
            validate(
                "woodcreek",
                "2026-11-13",
                "x",
                &"e".repeat(201),
                &day(),
                &day(),
                "2026-11-13"
            ),
            Err(SaveError::BadEvent)
        );
    }

    #[test]
    fn presets_are_valid_schedules() {
        for (name, l1, l2) in PRESETS {
            for rows in [l1, l2] {
                let inputs: Vec<PeriodInput> = rows.iter().map(|(n, s, e)| p(n, s, e)).collect();
                assert!(build_periods(&inputs).is_ok(), "{name}");
            }
        }
        assert_eq!(presets_json().as_array().unwrap().len(), PRESETS.len());
    }

    #[test]
    fn set_replaces_the_school_and_drops_other_schools_past_days() {
        let (store, dir) = store();
        let old = validate(
            "roseville",
            "2026-11-10",
            "Old",
            "",
            &day(),
            &day(),
            "2026-11-10",
        )
        .unwrap();
        set(&store, &old, "a@x.test", "2026-11-10").unwrap();
        let first = check("woodcreek", "2026-11-13", "First").unwrap();
        set(&store, &first, "a@x.test", "2026-11-13").unwrap();
        // Roseville's entry was for a past day, so saving another school removed it.
        assert_eq!(list(&store).len(), 1);
        let second = validate(
            "woodcreek",
            "2026-11-14",
            "Second",
            "Gym",
            &day(),
            &day(),
            "2026-11-13",
        )
        .unwrap();
        set(&store, &second, "a@x.test", "2026-11-13").unwrap();
        let all = list(&store);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Second");
        assert_eq!(all[0].special_event.as_deref(), Some("Gym"));
        assert_eq!(all[0].lunch2.len(), 2);
        assert_eq!(to_json(&all[0])["schedule"]["lunch1"][0]["duration"], 70);
        assert_eq!(clear(&store, Some("roseville")), 0);
        assert_eq!(clear(&store, Some("woodcreek")), 1);
        assert!(list(&store).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn legacy_file_is_imported_once_in_both_formats() {
        let dir = std::env::temp_dir().join(format!(
            "bell-legacy-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap();
        let single = json!({"date": "2026-11-20", "name": "Rally", "special_event": null,
            "schedule": {"lunch1": [{"name": "A", "start": "08:30", "end": "09:00", "duration": 30}], "lunch2": []}});
        std::fs::write(dir.join("data/bell_overrides.json"), single.to_string()).unwrap();
        let store = DataStore::open(&dir, &dir.join("data")).unwrap();
        let all = list(&store);
        assert_eq!((all.len(), all[0].school.as_str()), (1, "woodcreek"));
        // Clearing must stick across a reboot even though the file is still there.
        clear(&store, None);
        drop(store);
        let store = DataStore::open(&dir, &dir.join("data")).unwrap();
        assert!(list(&store).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
