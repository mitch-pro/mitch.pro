//! Public lunch menus from the ISITE feed linked by RJUHSD Food Services.
//! Dates and food names are preserved; nutrition and allergy claims are not inferred.

use axum::response::Response;
use chrono::{Duration, NaiveDate, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::OnceLock;
use tokio::sync::Mutex;

const MENU_PAGE: &str =
    "https://www.rjuhsdfoodservice.com/index.php?sid=0212112146052111&page=menus";
const MENU_API: &str = "https://api.schoolnutritionandfitness.com/graphql";
const CACHE_SECONDS: i64 = 30 * 60;

#[derive(Clone, Default, Serialize)]
struct Meals {
    entrees: Vec<String>,
    sides: Vec<String>,
}
#[derive(Clone, Serialize)]
struct MenuDay {
    date: String,
    #[serde(flatten)]
    meals: Meals,
}
#[derive(Clone, Serialize)]
struct MenuFeed {
    source: &'static str,
    source_url: &'static str,
    menu_url: String,
    published_month: String,
    generated_at: String,
    dates_checked: bool,
    days: Vec<MenuDay>,
    published_options: Meals,
}
struct CachedMenu {
    date: NaiveDate,
    fetched_at: i64,
    feed: MenuFeed,
}
#[derive(Default)]
struct MenuCache {
    item: Option<CachedMenu>,
    retry_after: i64,
}
static MENU_CACHE: OnceLock<Mutex<MenuCache>> = OnceLock::new();

/// Accept the client's Pacific date without permitting arbitrary historical fetches.
fn requested_date(raw: &str, today: NaiveDate) -> Option<NaiveDate> {
    let date = NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()?;
    (date.format("%Y-%m-%d").to_string() == raw
        && date.signed_duration_since(today).num_days().abs() <= 1)
        .then_some(date)
}

pub async fn school_menu(school: &str, date: &str) -> Response {
    let school = school.trim().to_ascii_lowercase();
    if !super::dayboard::RJUHSD_SCHOOLS
        .iter()
        .any(|(key, _)| *key == school)
    {
        return crate::errors::json_resp(400, json!({"error":"unknown school"}));
    }
    let Some(date) = requested_date(date, Utc::now().date_naive()) else {
        return crate::errors::json_resp(400, json!({"error":"invalid menu date"}));
    };
    // One district feed and one in-flight refresh for all campuses. No disk storage.
    let mut cached = MENU_CACHE
        .get_or_init(|| Mutex::new(MenuCache::default()))
        .lock()
        .await;
    let now = Utc::now().timestamp();
    if let Some(item) = cached.item.as_ref() {
        if item.date == date
            && now - item.fetched_at
                < if item.feed.dates_checked {
                    CACHE_SECONDS
                } else {
                    120
                }
        {
            return menu_response(&school, &item.feed, false);
        }
    }
    if now >= cached.retry_after {
        let fetched =
            tokio::time::timeout(std::time::Duration::from_secs(18), fetch_menu(date)).await;
        if let Ok(Ok(feed)) = fetched {
            let response = menu_response(&school, &feed, false);
            cached.item = Some(CachedMenu {
                date,
                fetched_at: now,
                feed,
            });
            cached.retry_after = 0;
            return response;
        }
        // Suppress repeated upstream requests while the public menu service is down.
        cached.retry_after = now + 120;
    }
    if let Some(item) = cached.item.as_ref() {
        if item.date == date && now - item.fetched_at < 24 * 60 * 60 {
            return menu_response(&school, &item.feed, true);
        }
    }
    crate::errors::json_resp(
        502,
        json!({"error":"menu unavailable","source_url":MENU_PAGE}),
    )
}
fn menu_response(school: &str, feed: &MenuFeed, stale: bool) -> Response {
    crate::errors::json_resp(200, json!({"school":school,"stale":stale,"menu":feed}))
}

/// Follow the district's Lunch link instead of hardcoding an old month's document ID.
fn lunch_menu_id(html: &str) -> Option<String> {
    let anchors = regex::Regex::new(r"(?is)<a\b[^>]*>").ok()?;
    let id = regex::Regex::new(r#"(?i)[?&]id=([a-f0-9]{24})(?:["'&\s]|$)"#).ok()?;
    for anchor in anchors.find_iter(html) {
        let tag = anchor.as_str().replace("&amp;", "&");
        let lower = tag.to_ascii_lowercase();
        if lower.contains("webmenus2/") && lower.contains("lunch") {
            if let Some(found) = id.captures(&tag) {
                return Some(found[1].to_ascii_lowercase());
            }
        }
    }
    None
}
async fn graphql(client: &reqwest::Client, query: &str, variables: Value) -> Result<Value, String> {
    let response = client
        .post(MENU_API)
        .json(&json!({"query":query,"variables":variables}))
        .send()
        .await
        .map_err(|_| "menu request failed")?
        .error_for_status()
        .map_err(|_| "menu service unavailable")?;
    let payload: Value = response.json().await.map_err(|_| "invalid menu response")?;
    if payload.get("errors").is_some() {
        return Err("menu query failed".into());
    }
    payload
        .get("data")
        .cloned()
        .ok_or("missing menu data".into())
}
async fn fetch_menu(date: NaiveDate) -> Result<MenuFeed, String> {
    let client = reqwest::Client::builder()
        .user_agent("rjuhsd.school public lunch menu")
        .timeout(std::time::Duration::from_secs(12))
        .build()
        .map_err(|_| "menu client unavailable")?;
    let html = client
        .get(MENU_PAGE)
        .send()
        .await
        .map_err(|_| "menu page unavailable")?
        .error_for_status()
        .map_err(|_| "menu page unavailable")?
        .text()
        .await
        .map_err(|_| "invalid menu page")?;
    let id = lunch_menu_id(&html).ok_or("no published lunch menu")?;
    let published = graphql(&client,
        "query PublishedLunch($id:String!){menu(id:$id){id month year menuType{id name} items{day hidden product{name category hide_on_web_menu_view hide_on_calendars trash is_ancillary}}}}",
        json!({"id":id})).await?;
    let menu = published
        .get("menu")
        .filter(|v| v.is_object())
        .ok_or("missing lunch menu")?;
    let menu_type = menu
        .pointer("/menuType/id")
        .and_then(Value::as_str)
        .ok_or("missing menu type")?;
    if menu.pointer("/menuType/name").and_then(Value::as_str) != Some("Lunch") {
        return Err("not a lunch menu".into());
    }
    // Date-specific entries can be newer than the pinned month. Include the first
    // school day after district breaks, without extrapolating a repeating menu.
    let dated = graphql(&client,
        "query LunchDates($id:String!,$start:String!,$end:String!){menuType(id:$id){items(start_date:$start,end_date:$end){date product{name category hide_on_web_menu_view hide_on_calendars trash is_ancillary}}}}",
        json!({"id":menu_type,"start":date.format("%m/%d/%Y").to_string(),"end":(date+Duration::days(45)).format("%m/%d/%Y").to_string()})).await;
    // A published menu remains useful when date-specific lookups are unavailable.
    parse_feed(menu, dated.ok().as_ref(), &id)
}
fn truthy(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(v)) => *v,
        Some(Value::Number(v)) => v.as_i64() == Some(1),
        Some(Value::String(v)) => v == "1" || v.eq_ignore_ascii_case("true"),
        _ => false,
    }
}
fn add_food(meals: &mut Meals, item: &Value) {
    let Some(product) = item.get("product").filter(|p| p.is_object()) else {
        return;
    };
    if truthy(item.get("hidden"))
        || [
            "trash",
            "hide_on_web_menu_view",
            "hide_on_calendars",
            "is_ancillary",
        ]
        .iter()
        .any(|key| truthy(product.get(*key)))
    {
        return;
    }
    let category = product
        .get("category")
        .and_then(Value::as_str)
        .unwrap_or("");
    if category.eq_ignore_ascii_case("Ancillary") {
        return;
    }
    let Some(name) = product.get("name").and_then(Value::as_str) else {
        return;
    };
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() || name.len() > 240 {
        return;
    }
    let list = if category.eq_ignore_ascii_case("Entrees") {
        &mut meals.entrees
    } else {
        &mut meals.sides
    };
    if !list.iter().any(|old| old.eq_ignore_ascii_case(&name)) {
        list.push(name);
    }
}
fn parse_feed(menu: &Value, dated: Option<&Value>, id: &str) -> Result<MenuFeed, String> {
    let year = menu
        .get("year")
        .and_then(Value::as_i64)
        .and_then(|v| i32::try_from(v).ok())
        .ok_or("invalid menu year")?;
    let month = menu
        .get("month")
        .and_then(Value::as_u64)
        .filter(|v| *v < 12)
        .ok_or("invalid menu month")? as u32
        + 1;
    let month_start = NaiveDate::from_ymd_opt(year, month, 1).ok_or("invalid menu date")?;
    let items = menu
        .get("items")
        .and_then(Value::as_array)
        .ok_or("missing menu items")?;
    let mut days: BTreeMap<String, Meals> = BTreeMap::new();
    let mut published_options = Meals::default();
    for item in items {
        let day = item
            .get("day")
            .and_then(Value::as_u64)
            .and_then(|v| u32::try_from(v).ok());
        if let Some(date) = day.and_then(|d| NaiveDate::from_ymd_opt(year, month, d)) {
            add_food(days.entry(date.to_string()).or_default(), item);
            add_food(&mut published_options, item);
        }
    }
    let dated_items = dated
        .and_then(|v| v.pointer("/menuType/items"))
        .and_then(Value::as_array);
    if let Some(items) = dated_items {
        for item in items {
            if let Some(date) = item
                .get("date")
                .and_then(Value::as_str)
                .and_then(|s| NaiveDate::parse_from_str(s, "%m/%d/%Y").ok())
            {
                add_food(days.entry(date.to_string()).or_default(), item);
            }
        }
    }
    if published_options.entrees.is_empty() {
        return Err("empty lunch menu".into());
    }
    Ok(MenuFeed {
        source: "RJUHSD Food Services",
        source_url: MENU_PAGE,
        menu_url: format!("https://www.rjuhsdfoodservice.com/webmenus2/#/view-no-design?id={id}"),
        published_month: month_start.format("%Y-%m").to_string(),
        generated_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        dates_checked: dated_items.is_some(),
        days: days
            .into_iter()
            .filter(|(_, meals)| !meals.entrees.is_empty())
            .map(|(date, meals)| MenuDay { date, meals })
            .collect(),
        published_options,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    /// Opt-in verification against the district's public feed; regular tests stay offline.
    #[tokio::test]
    #[ignore = "requires the public RJUHSD menu service"]
    async fn live_published_lunch_feed() {
        let feed = fetch_menu(Utc::now().date_naive())
            .await
            .expect("official menu fetched");
        assert!(!feed.published_options.entrees.is_empty());
        tracing::info!(
            "Published month: {}; lunch choices: {}; dated days: {}",
            feed.published_month,
            feed.published_options.entrees.len(),
            feed.days.len()
        );
    }

    #[test]
    fn discovers_only_the_linked_lunch_menu() {
        let html = r#"<a href="webmenus2/#/view-no-design?id=aaaaaaaaaaaaaaaaaaaaaaaa" title="Breakfast Menu"></a><a title="26-27 Lunch Menu" href="webmenus2/#/view-no-design?id=bbbbbbbbbbbbbbbbbbbbbbbb"></a>"#;
        assert_eq!(
            lunch_menu_id(html).as_deref(),
            Some("bbbbbbbbbbbbbbbbbbbbbbbb")
        );
        assert!(lunch_menu_id("<a href='https://other.example'>Lunch</a>").is_none());
    }
    #[test]
    fn preserves_dates_and_excludes_hidden_or_non_food_items() {
        let menu = json!({"year":2026,"month":7,"items":[
            {"day":6,"product":{"name":"Cheese Pizza","category":"Entrees"}},
            {"day":6,"product":{"name":"cheese pizza","category":"Entrees"}},
            {"day":6,"product":{"name":"Milk","category":"Milk"}},
            {"day":6,"product":{"name":"Shared Items","category":"Ancillary"}},
            {"day":6,"hidden":true,"product":{"name":"Hidden","category":"Entrees"}},
            {"day":6,"product":{"name":"Removed","category":"Entrees","trash":"1"}},
            {"day":40,"product":{"name":"Invalid date","category":"Entrees"}}
        ]});
        let dated = json!({"menuType":{"items":[{"date":"10/26/2026","product":{"name":"Rice Bowl","category":"Entrees"}}]}});
        let feed = parse_feed(&menu, Some(&dated), "public-id").expect("menu parsed");
        assert_eq!(feed.published_month, "2026-08");
        assert_eq!(feed.published_options.entrees, ["Cheese Pizza"]);
        assert_eq!(feed.published_options.sides, ["Milk"]);
        assert_eq!(feed.days[0].date, "2026-08-06");
        assert_eq!(feed.days[1].date, "2026-10-26");
        assert_eq!(feed.days[1].meals.entrees, ["Rice Bowl"]);
        assert!(!feed.days.iter().any(|day| day.date == "2026-10-25"));
        assert!(feed.dates_checked);
        let unavailable = json!({"menuType":null});
        assert!(
            !parse_feed(&menu, Some(&unavailable), "public-id")
                .expect("published menu remains available")
                .dates_checked
        );
    }
    #[test]
    fn accepts_pacific_dates_without_allowing_arbitrary_date_requests() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 26).expect("date");
        assert!(requested_date("2026-10-25", today).is_some());
        assert!(requested_date("2026-10-26", today).is_some());
        assert!(requested_date("2026-08-06", today).is_none());
        assert!(requested_date("not-a-date", today).is_none());
        assert!(requested_date("2026-2-30", today).is_none());
    }
}
