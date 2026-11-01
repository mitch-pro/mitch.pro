//! Dayboard public feeds — port of server.js dayboard functions (~5325-5566, ~10497-10515).
//! Roseville weather (Open-Meteo), Woodcreek activity calendar (iCal),
//! and RJUHSD school info (Finalsite scraping).
#![allow(clippy::expect_used)]

use axum::response::Response;
use chrono::Datelike;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;

pub const ROSEVILLE_WEATHER_URL: &str = "https://api.open-meteo.com/v1/forecast?latitude=38.7521&longitude=-121.2880&current=temperature_2m,apparent_temperature,weather_code,wind_speed_10m&hourly=temperature_2m,weather_code&daily=weather_code,temperature_2m_max,temperature_2m_min,sunrise,sunset&temperature_unit=fahrenheit&wind_speed_unit=mph&timezone=America%2FLos_Angeles&forecast_days=3";
pub const WOODCREEK_CALENDAR_URL: &str = "https://calendar.google.com/calendar/ical/c_635a46affcb227e30bc25d20d23583c36b2b344b2266d8244cdc81f119b9aa1d%40group.calendar.google.com/public/basic.ics";

pub struct SchoolEntry {
    pub name: &'static str,
    pub url: &'static str,
}

pub const RJUHSD_SCHOOLS: &[(&str, SchoolEntry)] = &[
    (
        "woodcreek",
        SchoolEntry {
            name: "Woodcreek High School",
            url: "https://woodcreek.rjuhsd.us",
        },
    ),
    (
        "roseville",
        SchoolEntry {
            name: "Roseville High School",
            url: "https://roseville.rjuhsd.us",
        },
    ),
    (
        "westpark",
        SchoolEntry {
            name: "West Park High School",
            url: "https://westpark.rjuhsd.us",
        },
    ),
    (
        "granitebay",
        SchoolEntry {
            name: "Granite Bay High School",
            url: "https://granitebay.rjuhsd.us",
        },
    ),
    (
        "antelope",
        SchoolEntry {
            name: "Antelope High School",
            url: "https://antelope.rjuhsd.us",
        },
    ),
    (
        "oakmont",
        SchoolEntry {
            name: "Oakmont High School",
            url: "https://oakmont.rjuhsd.us",
        },
    ),
];

#[derive(Clone)]
struct CacheItem {
    payload: Value,
    expires_at: u64,
}

#[derive(Default)]
struct DayboardState {
    weather: Option<CacheItem>,
    calendar: Option<CacheItem>,
    school_info: HashMap<String, CacheItem>,
}

static DAYBOARD_CACHE: std::sync::OnceLock<Mutex<DayboardState>> = std::sync::OnceLock::new();

fn cache() -> &'static Mutex<DayboardState> {
    DAYBOARD_CACHE.get_or_init(|| Mutex::new(DayboardState::default()))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn json_resp(code: u16, val: Value) -> Response {
    crate::errors::json_resp(code, val)
}

/// GET /api/weather — server.js:5325-5343, 10506-10509.
pub async fn weather() -> Response {
    let now = now_ms();
    {
        let c = cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(item) = &c.weather {
            if item.expires_at > now {
                return json_resp(200, item.payload.clone());
            }
        }
    }

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(6500))
        .build()
    {
        Ok(c) => c,
        Err(_) => return weather_fallback_or_err(),
    };

    match client
        .get(ROSEVILLE_WEATHER_URL)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(
            reqwest::header::USER_AGENT,
            "mitch.pro command-center weather",
        )
        .send()
        .await
    {
        Ok(res) if res.status().is_success() => {
            if let Ok(mut payload) = res.json::<Value>().await {
                if payload.get("current").is_some()
                    && payload.get("hourly").is_some()
                    && payload.get("daily").is_some()
                {
                    if let Some(obj) = payload.as_object_mut() {
                        obj.insert("location".into(), json!("Roseville, CA"));
                        obj.insert("source".into(), json!("Open-Meteo"));
                        obj.insert("generated_at".into(), json!(now_iso()));
                        obj.insert("stale".into(), json!(false));
                    }
                    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
                    c.weather = Some(CacheItem {
                        payload: payload.clone(),
                        expires_at: now + 10 * 60 * 1000,
                    });
                    return json_resp(200, payload);
                }
            }
            weather_fallback_or_err()
        }
        _ => weather_fallback_or_err(),
    }
}

fn weather_fallback_or_err() -> Response {
    let c = cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(item) = &c.weather {
        let mut stale = item.payload.clone();
        if let Some(obj) = stale.as_object_mut() {
            obj.insert("stale".into(), json!(true));
        }
        return json_resp(200, stale);
    }
    json_resp(502, json!({ "error": "weather unavailable" }))
}

/// GET /api/school-calendar — server.js:5412-5430, 10508-10509.
pub async fn school_calendar() -> Response {
    let now = now_ms();
    {
        let c = cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(item) = &c.calendar {
            if item.expires_at > now {
                return json_resp(200, item.payload.clone());
            }
        }
    }

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(8500))
        .build()
    {
        Ok(c) => c,
        Err(_) => return calendar_fallback_or_err(),
    };

    match client
        .get(WOODCREEK_CALENDAR_URL)
        .header(reqwest::header::ACCEPT, "text/calendar")
        .header(
            reqwest::header::USER_AGENT,
            "mitch.pro command-center calendar",
        )
        .send()
        .await
    {
        Ok(res) if res.status().is_success() => {
            if let Ok(text) = res.text().await {
                let events = parse_woodcreek_calendar(&text);
                if !events.is_empty() {
                    let result = json!({
                        "events": events,
                        "source": "Woodcreek High School",
                        "source_url": "https://woodcreek.rjuhsd.us/calendar",
                        "generated_at": now_iso(),
                        "stale": false,
                    });
                    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
                    c.calendar = Some(CacheItem {
                        payload: result.clone(),
                        expires_at: now + 30 * 60 * 1000,
                    });
                    return json_resp(200, result);
                }
            }
            calendar_fallback_or_err()
        }
        _ => calendar_fallback_or_err(),
    }
}

fn calendar_fallback_or_err() -> Response {
    let c = cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(item) = &c.calendar {
        let mut stale = item.payload.clone();
        if let Some(obj) = stale.as_object_mut() {
            obj.insert("stale".into(), json!(true));
        }
        return json_resp(200, stale);
    }
    json_resp(502, json!({ "error": "school calendar unavailable" }))
}

/// GET /api/school-info?school=... — server.js:5514-5566, 10501-10505.
pub async fn school_info(school_key: &str) -> Response {
    let school_key = school_key.trim().to_ascii_lowercase();
    let entry = RJUHSD_SCHOOLS
        .iter()
        .find(|(k, _)| *k == school_key)
        .map(|(_, e)| e);
    let Some(school) = entry else {
        return json_resp(400, json!({ "error": "unknown school" }));
    };

    let now = now_ms();
    {
        let c = cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(item) = c.school_info.get(&school_key) {
            if item.expires_at > now {
                return json_resp(200, item.payload.clone());
            }
        }
    }

    match fetch_school_info_payload(&school_key, school).await {
        Ok(result) => {
            let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
            c.school_info.insert(
                school_key,
                CacheItem {
                    payload: result.clone(),
                    expires_at: now + 30 * 60 * 1000,
                },
            );
            json_resp(200, result)
        }
        Err(err) => {
            tracing::warn!("[school-info] {school_key}: {err}");
            let c = cache().lock().unwrap_or_else(|e| e.into_inner());
            if let Some(item) = c.school_info.get(&school_key) {
                let mut stale = item.payload.clone();
                if let Some(obj) = stale.as_object_mut() {
                    obj.insert("stale".into(), json!(true));
                }
                return json_resp(200, stale);
            }
            json_resp(502, json!({ "error": "school info unavailable" }))
        }
    }
}

async fn fetch_school_info_payload(
    school_key: &str,
    school: &SchoolEntry,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(8500))
        .build()?;

    let home_res = client
        .get(format!("{}/", school.url))
        .header(reqwest::header::ACCEPT, "text/html")
        .header(
            reqwest::header::USER_AGENT,
            "mitch.pro school hub (rjuhsd.school)",
        )
        .send()
        .await?;

    if !home_res.status().is_success() {
        return Err("school site error".into());
    }

    let html = home_res.text().await?;
    let (mut events, news) = parse_finalsite_homepage(&html);

    let calendar_subpath = match school_key {
        "roseville" => "school-calendar",
        "antelope" => "antelope-hs-calendar",
        "westpark" => "panther-calendar",
        _ => "calendar",
    };
    let calendar_url = format!("{}/{}", school.url, calendar_subpath);
    let mut calendar_verified = false;

    // The month view only lists one month; `?cal_date=` picks others, so
    // fetch this month and the next two to have upcoming events at month end.
    let mut cal_events: Vec<Value> = Vec::new();
    let first = chrono::Utc::now().date_naive().with_day0(0);
    let urls: Vec<String> = (0..3u32)
        .map(|n| {
            let month = first
                .and_then(|d| d.checked_add_months(chrono::Months::new(n)))
                .map(|d| d.format("%Y-%m-%d").to_string());
            match (n, month) {
                (0, _) | (_, None) => calendar_url.clone(),
                (_, Some(d)) => format!("{calendar_url}?cal_date={d}"),
            }
        })
        .collect();
    let pages = futures_util::future::join_all(urls.iter().map(|url| {
        let client = client.clone();
        async move {
            let res = client
                .get(url)
                .header(reqwest::header::ACCEPT, "text/html")
                .header(
                    reqwest::header::USER_AGENT,
                    "mitch.pro school hub (rjuhsd.school)",
                )
                .send()
                .await
                .ok()?;
            if !res.status().is_success() {
                return None;
            }
            res.text().await.ok()
        }
    }))
    .await;
    for html in pages.into_iter().flatten() {
        cal_events.extend(parse_school_calendar(&html));
    }
    if !cal_events.is_empty() {
        calendar_verified = true;
        let mut seen = std::collections::HashSet::new();
        let mut merged = Vec::new();
        for ev in cal_events.into_iter().chain(events) {
            let d = ev.get("date").and_then(|v| v.as_str()).unwrap_or("");
            let t = ev
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let key = format!("{d}|{t}");
            if seen.insert(key) {
                merged.push(ev);
            }
        }
        merged.sort_by(|a, b| {
            let da = a.get("date").and_then(|v| v.as_str()).unwrap_or("");
            let db = b.get("date").and_then(|v| v.as_str()).unwrap_or("");
            da.cmp(db)
        });
        events = merged;
    }

    static MOTTO_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let motto_re = MOTTO_RE.get_or_init(|| {
        regex::Regex::new(r#"class="fsLocationMotto"[^>]*>([\s\S]*?)</div>"#).expect("static regex")
    });
    let motto = if let Some(m) = motto_re.captures(&html) {
        let stripped = strip_html(&m[1]);
        if stripped.len() > 120 {
            stripped[..120].to_string()
        } else {
            stripped
        }
    } else {
        String::new()
    };

    if events.is_empty() && news.is_empty() {
        return Err("school page parse empty".into());
    }

    Ok(json!({
        "school": school_key,
        "calendar_verified": calendar_verified,
        "calendar_url": calendar_url,
        "name": school.name,
        "motto": motto,
        "url": school.url,
        "events": events,
        "news": news,
        "source": format!("{} (official site)", school.name),
        "source_url": school.url,
        "generated_at": now_iso(),
        "stale": false,
    }))
}

// ── iCal parsing ──

pub fn unfold_ical_lines(value: &str) -> Vec<String> {
    let normalized = value.replace("\r\n", "\n");
    let mut lines: Vec<String> = Vec::new();
    for line in normalized.split('\n') {
        if (line.starts_with(' ') || line.starts_with('\t')) && !lines.is_empty() {
            if let Some(last) = lines.last_mut() {
                last.push_str(&line[1..]);
            }
        } else {
            lines.push(line.trim_end_matches('\r').to_string());
        }
    }
    lines
}

pub fn decode_ical_text(value: &str) -> String {
    value
        .replace("\\N", "\n")
        .replace("\\n", "\n")
        .replace("\\,", ",")
        .replace("\\;", ";")
        .replace("\\\\", "\\")
        .trim()
        .to_string()
}

pub struct ParsedIcalDate {
    pub date_key: String,
    pub all_day: bool,
    pub display_time: Option<String>,
    pub iso_string: String,
}

pub fn parse_ical_date(value: &str, field: &str) -> Option<ParsedIcalDate> {
    let raw = value.trim();
    let all_day = field.to_ascii_uppercase().contains("VALUE=DATE")
        || (raw.len() == 8 && raw.chars().all(|c| c.is_ascii_digit()));

    static DATE_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = DATE_RE.get_or_init(|| {
        regex::Regex::new(r"^(\d{4})(\d{2})(\d{2})(?:T(\d{2})(\d{2})(\d{2})?(Z)?)?")
            .expect("static regex")
    });

    let caps = re.captures(raw)?;
    let year = caps.get(1)?.as_str();
    let month = caps.get(2)?.as_str();
    let day = caps.get(3)?.as_str();
    let hour = caps.get(4).map(|m| m.as_str()).unwrap_or("00");
    let minute = caps.get(5).map(|m| m.as_str()).unwrap_or("00");
    let second = caps.get(6).map(|m| m.as_str()).unwrap_or("00");
    let is_utc = caps.get(7).is_some();

    let date_key = format!("{year}-{month}-{day}");
    let iso = format!(
        "{year}-{month}-{day}T{hour}:{minute}:{second}{}",
        if is_utc { "Z" } else { "-07:00" }
    );

    let display_time = if !all_day {
        let hour_num: u32 = hour.parse().unwrap_or(0);
        let h12 = if hour_num == 0 {
            12
        } else if hour_num > 12 {
            hour_num - 12
        } else {
            hour_num
        };
        let ampm = if hour_num >= 12 { "PM" } else { "AM" };
        Some(format!("{h12}:{minute} {ampm}"))
    } else {
        None
    };

    Some(ParsedIcalDate {
        date_key,
        all_day,
        display_time,
        iso_string: iso,
    })
}

pub fn parse_woodcreek_calendar(ical: &str) -> Vec<Value> {
    let mut events = Vec::new();
    let mut current: Option<HashMap<String, (String, String)>> = None;

    for line in unfold_ical_lines(ical) {
        if line == "BEGIN:VEVENT" {
            current = Some(HashMap::new());
            continue;
        }
        if line == "END:VEVENT" {
            if let Some(ev) = current.take() {
                events.push(ev);
            }
            continue;
        }
        if let Some(map) = &mut current {
            if let Some(colon) = line.find(':') {
                let field = &line[..colon];
                let value = &line[colon + 1..];
                let name = field.split(';').next().unwrap_or("").to_ascii_uppercase();
                map.entry(name)
                    .or_insert_with(|| (field.to_string(), value.to_string()));
            }
        }
    }

    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for ev in events {
        if let Some((_, status)) = ev.get("STATUS") {
            if status.trim().eq_ignore_ascii_case("CANCELLED") {
                continue;
            }
        }
        let start = ev
            .get("DTSTART")
            .and_then(|(field, val)| parse_ical_date(val, field));
        let end = ev
            .get("DTEND")
            .and_then(|(field, val)| parse_ical_date(val, field));
        let summary_raw = ev.get("SUMMARY").map(|(_, v)| v.as_str()).unwrap_or("");
        let title = decode_ical_text(summary_raw);
        let title = if title.len() > 240 {
            title[..240].to_string()
        } else {
            title
        };

        let Some(start) = start else {
            continue;
        };
        if title.is_empty() {
            continue;
        }

        let mut detail = "All day".to_string();
        if !start.all_day {
            if let Some(st) = &start.display_time {
                detail = if let Some(et) = end.as_ref().and_then(|e| e.display_time.as_ref()) {
                    format!("{st} \u{2013} {et}")
                } else {
                    st.clone()
                };
            }
        }
        if let Some((_, loc_val)) = ev.get("LOCATION") {
            let loc = decode_ical_text(loc_val).replace('\n', " ");
            let loc = loc.trim();
            if !loc.is_empty() {
                let loc_trunc = if loc.len() > 160 { &loc[..160] } else { loc };
                detail.push_str(&format!(" \u{00b7} {loc_trunc}"));
            }
        }

        let key = format!("{}|{}|{}", start.date_key, start.iso_string, title);
        if !seen.insert(key) {
            continue;
        }

        result.push(json!({
            "date": start.date_key,
            "title": title,
            "detail": detail,
            "all_day": start.all_day,
            "starts_at": if start.all_day { Value::Null } else { json!(start.iso_string) },
        }));
    }

    result.sort_by(|a, b| {
        let da = a.get("date").and_then(|v| v.as_str()).unwrap_or("");
        let db = b.get("date").and_then(|v| v.as_str()).unwrap_or("");
        let sa = a.get("starts_at").and_then(|v| v.as_str()).unwrap_or("");
        let sb = b.get("starts_at").and_then(|v| v.as_str()).unwrap_or("");
        let ta = a.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let tb = b.get("title").and_then(|v| v.as_str()).unwrap_or("");
        da.cmp(db).then_with(|| sa.cmp(sb)).then_with(|| ta.cmp(tb))
    });

    if result.len() > 1000 {
        result.truncate(1000);
    }
    result
}

// ── Finalsite homepage & calendar parsing ──

pub fn decode_html_entities(value: &str) -> String {
    let mut s = value
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ");

    static ENT_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = ENT_RE.get_or_init(|| regex::Regex::new(r"&#(\d+);").expect("static regex"));
    s = re
        .replace_all(&s, |caps: &regex::Captures| {
            if let Ok(code) = caps[1].parse::<u32>() {
                if let Some(c) = char::from_u32(code) {
                    return c.to_string();
                }
            }
            String::new()
        })
        .to_string();

    s
}

pub fn strip_html(value: &str) -> String {
    static TAG_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = TAG_RE.get_or_init(|| regex::Regex::new(r"<[^>]*>").expect("static regex"));
    let stripped = re.replace_all(value, " ");
    let decoded = decode_html_entities(&stripped);
    static WS_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let ws_re = WS_RE.get_or_init(|| regex::Regex::new(r"\s+").expect("static regex"));
    ws_re.replace_all(&decoded, " ").trim().to_string()
}

pub fn parse_finalsite_homepage(html: &str) -> (Vec<Value>, Vec<Value>) {
    let mut events = Vec::new();
    let mut news = Vec::new();

    static ARTICLE_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let art_re = ARTICLE_RE.get_or_init(|| {
        regex::Regex::new(r#"(?s)<article\b[^>]*>(.*?)</article>"#).expect("static regex")
    });

    static NEWS_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let news_re = NEWS_RE.get_or_init(|| {
        regex::Regex::new(
            r#"(?s)<a\s+class="fsPostLink[^"]*"\s+data-slug="([^"]+)"[^>]*>(.*?)</a>"#,
        )
        .expect("static regex")
    });

    static TIME_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let time_re = TIME_RE
        .get_or_init(|| regex::Regex::new(r#"<time\s+datetime="([^"]+)""#).expect("static regex"));

    static CAL_LINK_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let cal_link_re = CAL_LINK_RE.get_or_init(|| {
        regex::Regex::new(r#"(?s)class="fsCalendarEventLink"[^>]*>(.*?)</a>"#)
            .expect("static regex")
    });

    static DETAILS_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let details_re = DETAILS_RE.get_or_init(|| {
        regex::Regex::new(r#"(?s)class="fsEventDetails"[^>]*>(.*?)</div>"#).expect("static regex")
    });

    for caps in art_re.captures_iter(html) {
        let block = &caps[1];
        if let Some(nlink) = news_re.captures(block) {
            if !nlink[0].contains("fsThumbnail") {
                let mut title = strip_html(&nlink[2]);
                if title
                    .to_ascii_lowercase()
                    .ends_with("(opens in new window/tab)")
                {
                    title = title[..title.len() - "(opens in new window/tab)".len()]
                        .trim()
                        .to_string();
                }
                let when = time_re.captures(block);
                let date = when
                    .map(|w| {
                        let dt = &w[1];
                        if dt.len() >= 10 {
                            dt[..10].to_string()
                        } else {
                            dt.to_string()
                        }
                    })
                    .unwrap_or_default();
                if !title.is_empty() && !title.eq_ignore_ascii_case("read more") {
                    news.push(json!({
                        "title": title,
                        "slug": &nlink[1],
                        "date": date,
                    }));
                }
                continue;
            }
        }

        let Some(when) = time_re.captures(block) else {
            continue;
        };
        let Some(title_link) = cal_link_re.captures(block) else {
            continue;
        };
        let title = strip_html(&title_link[1]);
        if title.is_empty() || title.eq_ignore_ascii_case("read more") {
            continue;
        }
        let all_day = block.contains("class=\"fsAllDay\"");
        let mut detail = String::new();
        if let Some(det) = details_re.captures(block) {
            detail = strip_html(&det[1]);
        } else if all_day {
            detail = "All day".to_string();
        }

        let raw_when = &when[1];
        let date_slice = if raw_when.len() >= 10 {
            &raw_when[..10]
        } else {
            raw_when
        };
        let title_slice = if title.len() > 240 {
            &title[..240]
        } else {
            &title
        };
        let detail_slice = if detail.len() > 200 {
            &detail[..200]
        } else {
            &detail
        };

        events.push(json!({
            "date": date_slice,
            "title": title_slice,
            "detail": detail_slice,
            "all_day": all_day,
            "starts_at": if all_day { Value::Null } else { json!(raw_when) },
        }));
    }

    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let mut filtered_events: Vec<Value> = events
        .into_iter()
        .filter(|e| {
            e.get("date")
                .and_then(|v| v.as_str())
                .map(|d| d >= today.as_str())
                .unwrap_or(false)
        })
        .collect();
    if filtered_events.len() > 60 {
        filtered_events.truncate(60);
    }
    if news.len() > 12 {
        news.truncate(12);
    }

    (filtered_events, news)
}

pub fn parse_school_calendar(html: &str) -> Vec<Value> {
    static DAY_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let day_re = DAY_RE.get_or_init(|| {
        regex::Regex::new(r#"class="fsCalendarDate"[^>]*data-day="(\d+)"[^>]*data-year="(\d+)"[^>]*data-month="(\d+)""#).expect("static regex")
    });

    static TITLE_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let title_re = TITLE_RE.get_or_init(|| {
        regex::Regex::new(r#"class="fsCalendar(?:EventTitle|Title)[^"]*"[^>]*title="([^"]+)""#)
            .expect("static regex")
    });

    let mut events = Vec::new();
    let matches: Vec<regex::Captures> = day_re.captures_iter(html).collect();

    for i in 0..matches.len() {
        let caps = &matches[i];
        let day: u32 = caps[1].parse().unwrap_or(0);
        let year = &caps[2];
        let month_0: u32 = caps[3].parse().unwrap_or(0);
        let month = month_0 + 1;
        let date = format!("{year}-{month:02}-{day:02}");

        let start_pos = caps.get(0).map(|m| m.end()).unwrap_or(0);
        let end_pos = if i + 1 < matches.len() {
            matches[i + 1]
                .get(0)
                .map(|m| m.start())
                .unwrap_or(html.len())
        } else {
            html.len()
        };
        let segment = &html[start_pos..end_pos];

        for t in title_re.captures_iter(segment) {
            let title = decode_html_entities(&t[1]);
            events.push(json!({
                "date": date,
                "title": title,
                "detail": "",
                "all_day": true,
                "starts_at": Value::Null,
            }));
        }
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unfold_ical_lines() {
        let input = "SUMMARY:Line 1\r\n Line 2 continued\r\n\tLine 3 continued\r\nLOCATION:Gym\r\n";
        let lines = unfold_ical_lines(input);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "SUMMARY:Line 1Line 2 continuedLine 3 continued");
        assert_eq!(lines[1], "LOCATION:Gym");
        assert_eq!(lines[2], "");
    }

    #[test]
    fn test_decode_ical_text() {
        let raw = r"First\, Second\; Third\\Fourth\NNew line";
        assert_eq!(
            decode_ical_text(raw),
            "First, Second; Third\\Fourth\nNew line"
        );
    }

    #[test]
    fn test_parse_ical_date() {
        let dt = parse_ical_date("20260925T143000Z", "DTSTART").unwrap();
        assert_eq!(dt.date_key, "2026-09-25");
        assert!(!dt.all_day);
        assert_eq!(dt.display_time.as_deref(), Some("2:30 PM"));

        let allday = parse_ical_date("20260925", "DTSTART;VALUE=DATE").unwrap();
        assert_eq!(allday.date_key, "2026-09-25");
        assert!(allday.all_day);
        assert!(allday.display_time.is_none());
    }

    #[test]
    fn test_decode_html_entities_and_strip() {
        let raw = "<div class=\"test\">&quot;Hello&quot; &amp; &#39;World&#39; &gt; 0</div>";
        assert_eq!(strip_html(raw), "\"Hello\" & 'World' > 0");
    }

    #[test]
    fn test_parse_finalsite_homepage() {
        let html = r#"
        <article>
          <time datetime="2028-10-15T09:00:00"></time>
          <a class="fsCalendarEventLink">Varsity Football</a>
          <div class="fsEventDetails">Stadium</div>
        </article>
        <article>
          <time datetime="2028-10-12"></time>
          <a class="fsPostLink" data-slug="homecoming-rally">Homecoming Rally (opens in new window/tab)</a>
        </article>
        "#;
        let (events, news) = parse_finalsite_homepage(html);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["title"], "Varsity Football");
        assert_eq!(events[0]["detail"], "Stadium");
        assert_eq!(events[0]["date"], "2028-10-15");

        assert_eq!(news.len(), 1);
        assert_eq!(news[0]["title"], "Homecoming Rally");
        assert_eq!(news[0]["slug"], "homecoming-rally");
    }

    #[test]
    fn test_parse_school_calendar() {
        let html = r#"
        <div class="fsCalendarDate" data-day="18" data-year="2026" data-month="8">
          <a class="fsCalendarEventTitle" title="Minimum Day &amp; Staff Meeting"></a>
        </div>
        "#;
        let events = parse_school_calendar(html);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["date"], "2026-09-18");
        assert_eq!(events[0]["title"], "Minimum Day & Staff Meeting");
        assert_eq!(events[0]["all_day"], true);
    }

    #[test]
    fn parses_current_finalsite_event_buttons() {
        let html = r#"
        <div class="fsCalendarDate"
             data-day="2"
             data-year="2026"
             data-month="10">
        </div>
        <button name="button" type="button" class="fsCalendarTitle fsCalendarEventLink fsButtonReset" title="Teacher Work day- No School" data-occur-id="1">
          Teacher Work day- No School
        </button>
        "#;
        let events = parse_school_calendar(html);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["date"], "2026-11-02");
        assert_eq!(events[0]["title"], "Teacher Work day- No School");
    }
}
