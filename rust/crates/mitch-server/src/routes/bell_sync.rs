//! Live bell schedules from the district school sites.
//!
//! `GET /api/bell/published?school=<key>` returns the schedule tables parsed
//! from the school's own bell-schedule page (see `mitch_lib::bell_sync` for the
//! parser and storage). A background worker refreshes every school every six
//! hours; the endpoint only fetches inline when a school has never been synced
//! or its last attempt failed and is old enough to retry.
//!
//! Each table is annotated with the weekdays and lunch it applies to so the
//! client can overlay regular days onto its bundled schedule without
//! interpreting page headings. Special schedules (minimum day, rally, finals)
//! are returned with `days: null` and are not applied by the client.
//!
//! Only the six hardcoded `*.rjuhsd.us` origins from `RJUHSD_SCHOOLS` are ever
//! fetched; the `school` parameter selects from that list and is never used to
//! build a URL.

use crate::routes::dayboard::RJUHSD_SCHOOLS;
use axum::response::Response;
use mitch_lib::bell_sync::{self, PublishedTable};
use mitch_lib::data::DataStore;
use serde_json::{json, Value};

/// Re-sync interval for the background worker.
const REFRESH_EVERY: std::time::Duration = std::time::Duration::from_secs(6 * 3600);
/// A successful sync older than this is refetched inline by the endpoint.
const STALE_AFTER_MS: i64 = 12 * 3600 * 1000;
/// A failed sync is retried inline at most this often.
const RETRY_AFTER_MS: i64 = 15 * 60 * 1000;
/// Bell pages are ~60-80 KB; anything far larger is not the page we expect.
const MAX_PAGE_BYTES: usize = 2 * 1024 * 1024;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Path of a school's bell page. Antelope's is singular.
fn bell_path(school: &str) -> &'static str {
    if school == "antelope" {
        "about/bell-schedule"
    } else {
        "about/bell-schedules"
    }
}

fn source_url(school: &str, base: &str) -> String {
    format!("{base}/{}", bell_path(school))
}

/// Weekdays (1 = Monday .. 5 = Friday) named in a heading or caption such as
/// "(M, Tu, Th, F) Regular Day", "Tuesday Schedule" or "(Mondays/Fridays)".
fn weekdays_in(text: &str) -> Vec<u8> {
    let mut days = Vec::new();
    for word in text
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|w| !w.is_empty())
    {
        let day = match word {
            "m" => 1,
            "tu" => 2,
            "w" => 3,
            "th" => 4,
            "f" => 5,
            w if w.starts_with("mon") => 1,
            w if w.starts_with("tue") => 2,
            w if w.starts_with("wed") => 3,
            w if w.starts_with("thu") => 4,
            w if w.starts_with("fri") => 5,
            _ => continue,
        };
        if !days.contains(&day) {
            days.push(day);
        }
    }
    days
}

/// Which weekdays and lunch a table is the regular schedule for, or `None`
/// for special days the client should not overlay.
///
/// - A plain "1st Lunch" / "2nd Lunch" caption takes its weekdays from the
///   group heading.
/// - A caption that names its own weekday ("(Wed) Collaboration ...") applies
///   to that weekday for both lunches (those days have a single lunch).
/// - Anything else (Minimum Day, Rally, Midterms/Finals, unlabeled tables) is
///   a special schedule.
fn classify(table: &PublishedTable) -> Option<(Vec<u8>, &'static str)> {
    let label = table.label.trim().to_lowercase();
    let own_days = weekdays_in(&table.label);
    if !own_days.is_empty() {
        return Some((own_days, "all"));
    }
    let lunch = match label.as_str() {
        "1st lunch" => "1",
        "2nd lunch" => "2",
        _ => return None,
    };
    let days = weekdays_in(&table.group);
    if days.is_empty() {
        None
    } else {
        Some((days, lunch))
    }
}

fn clock(minutes: u32) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

fn table_json(table: &PublishedTable) -> Value {
    let applies = classify(table);
    json!({
        "group": table.group,
        "label": table.label,
        "days": applies.as_ref().map(|(d, _)| d.clone()),
        "lunch": applies.as_ref().map(|(_, l)| *l),
        "periods": table.periods.iter().map(|p| json!({
            "name": p.name,
            "start": clock(p.start),
            "end": clock(p.end),
            "duration": p.end - p.start,
        })).collect::<Vec<_>>(),
    })
}

/// Fetch and store one school's schedule. On any failure the previously stored
/// schedule is kept and the error is recorded.
pub async fn refresh_school(store: &DataStore, school: &str) -> Result<usize, String> {
    let Some((_, entry)) = RJUHSD_SCHOOLS.iter().find(|(k, _)| *k == school) else {
        return Err("unknown school".into());
    };
    let url = source_url(school, entry.url);
    let result = fetch_tables(&url).await;
    let now = now_ms();
    match result {
        Ok(tables) => {
            let count = tables.len();
            if bell_sync::replace_school(store, school, &url, &tables, now) {
                Ok(count)
            } else {
                let msg = "no recognizable schedule tables on the page";
                bell_sync::record_failure(store, school, &url, msg, now);
                Err(msg.into())
            }
        }
        Err(err) => {
            bell_sync::record_failure(store, school, &url, &err, now);
            Err(err)
        }
    }
}

async fn fetch_tables(url: &str) -> Result<Vec<PublishedTable>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "text/html")
        .header(
            reqwest::header::USER_AGENT,
            "mitch.pro school hub (rjuhsd.school)",
        )
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("school site returned {}", response.status()));
    }
    let html = response.text().await.map_err(|e| e.to_string())?;
    if html.len() > MAX_PAGE_BYTES {
        return Err("bell page unexpectedly large".into());
    }
    Ok(bell_sync::parse_bell_page(&html))
}

/// Refresh every school once, spaced out so the district site sees six
/// ordinary page loads rather than a burst.
pub async fn refresh_all(store: &DataStore) {
    for (key, _) in RJUHSD_SCHOOLS {
        if let Err(err) = refresh_school(store, key).await {
            tracing::warn!(school = key, "bell schedule sync failed: {err}");
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

/// Start the periodic sync. The first pass waits a minute so it never
/// competes with server startup.
pub fn spawn(state: std::sync::Arc<crate::state::AppState>) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        loop {
            refresh_all(&state.store).await;
            tokio::time::sleep(REFRESH_EVERY).await;
        }
    });
}

/// `GET /api/bell/published?school=<key>`.
pub async fn published(state: &crate::state::AppState, school: &str) -> Response {
    let school = school.trim().to_ascii_lowercase();
    let Some((_, entry)) = RJUHSD_SCHOOLS.iter().find(|(k, _)| *k == school) else {
        return crate::errors::json_resp(400, json!({ "error": "unknown school" }));
    };

    let now = now_ms();
    let needs_fetch = match bell_sync::load_status(&state.store, &school) {
        None => true,
        Some(s) if s.ok => now - s.checked_at > STALE_AFTER_MS,
        Some(s) => now - s.checked_at > RETRY_AFTER_MS,
    };
    if needs_fetch {
        // Failure is recorded and reported through `ok`/`error` below.
        let _ = refresh_school(&state.store, &school).await;
    }

    let tables = bell_sync::load_school(&state.store, &school);
    let status = bell_sync::load_status(&state.store, &school);
    crate::errors::json_resp(
        200,
        json!({
            "school": school,
            "source_url": source_url(&school, entry.url),
            "fetched_at": status.as_ref().map(|s| s.fetched_at).unwrap_or(0),
            "checked_at": status.as_ref().map(|s| s.checked_at).unwrap_or(0),
            "ok": status.as_ref().map(|s| s.ok).unwrap_or(false),
            "tables": tables.iter().map(table_json).collect::<Vec<_>>(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mitch_lib::bell_sync::PublishedPeriod;

    fn table(group: &str, label: &str) -> PublishedTable {
        PublishedTable {
            group: group.into(),
            label: label.into(),
            periods: vec![
                PublishedPeriod {
                    name: "Period 1".into(),
                    start: 510,
                    end: 592,
                },
                PublishedPeriod {
                    name: "Period 2".into(),
                    start: 600,
                    end: 682,
                },
            ],
        }
    }

    #[test]
    fn weekday_headings() {
        assert_eq!(weekdays_in("(M, Tu, Th, F) Regular Day"), vec![1, 2, 4, 5]);
        assert_eq!(
            weekdays_in("(Mon, Tues, Thur, Fri) Regular Day"),
            vec![1, 2, 4, 5]
        );
        assert_eq!(
            weekdays_in("(Mondays/Fridays) Regular Two Lunch"),
            vec![1, 5]
        );
        assert_eq!(weekdays_in("Tuesday Schedule"), vec![2]);
        assert_eq!(weekdays_in("(Wednesday) Intervention"), vec![3]);
        assert!(weekdays_in("Non-Pack Days").is_empty());
        assert!(weekdays_in("Rally Schedule").is_empty());
    }

    #[test]
    fn classifies_regular_and_special_tables() {
        assert_eq!(
            classify(&table("Tuesday Schedule", "2nd Lunch")),
            Some((vec![2], "2"))
        );
        assert_eq!(
            classify(&table(
                "(Mon, Tues, Thur, Fri) Regular Day",
                "(Wed) Collaboration"
            )),
            Some((vec![3], "all"))
        );
        assert_eq!(
            classify(&table("(M, Tu, Th, F) Regular Day", "Minimum Day")),
            None
        );
        assert_eq!(classify(&table("Non-Pack Days", "1st Lunch")), None);
        assert_eq!(classify(&table("Friday Schedule", "")), None);
        assert_eq!(
            classify(&table("Rally Schedule", "Midterms* / Finals**")),
            None
        );
    }

    #[test]
    fn formats_clock() {
        assert_eq!(clock(510), "08:30");
        assert_eq!(clock(15 * 60 + 20), "15:20");
    }
}
