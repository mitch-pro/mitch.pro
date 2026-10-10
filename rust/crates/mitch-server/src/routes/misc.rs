//! First ported route group (plan Step 7): read-heavy, low-coupling API
//! endpoints. Each returns `Some(Response)` for a match, `None` for
//! fallthrough to the page block (which 404s unmatched /api/ paths).
//!
//! Ported in this pass (simplest subset):
//! - `GET /api/site-info` — reads data/site.json (PRESERVED disk file)
//! - `GET /api/bad-passwords` — raw passthrough of data/bad_passwords.json
//! - `GET /api/backgrounds/list` — scans webserver/backgrounds/ for .webp/.webm
//! - `POST /api/log-click` — appends to data/heatmap.json (trimmed to 1000)
//! - `GET /api/leaderboard` — coins/profiles/cosmetics ranked by coins desc
//! - `GET /api/games` — game list with category/query filters + featured
//!
//! Deferred to a later pass (needs external proxies or coins/presence):
//! `/api/ping` (heaviest), `/api/solve|submit|stats` (captcha proxy),
//! `/api/weather|school-calendar|school-info` (external fetches),
//! `/api/content` (complex injection ladder).

use crate::errors::json_resp;
use crate::state::AppState;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::Response;
use mitch_lib::jsval;
use serde_json::{json, Value};
use std::sync::Arc;

/// API route dispatch — called from handler.rs after the auth gates.
/// Returns `Some(Response)` for a matched route, `None` for fallthrough.
pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    _search: &str,
    body: &serde_json::Value,
    body_bytes: &[u8],
) -> Option<Response> {
    if path == "/api/site-info" && *method == Method::GET {
        return Some(site_info(state));
    }
    if path == "/api/pass" && *method == Method::POST {
        return Some(site_catalog(state, headers));
    }
    if path == "/api/verify-open" || path == "/verify-open.json" {
        return Some(verify_open(method));
    }
    if path == "/api/guest-session" && *method == Method::GET {
        return Some(guest_session(state, headers));
    }
    if (path == "/api/health" || path == "/health" || path == "/healthz") && *method == Method::GET
    {
        return Some(health_check());
    }
    if path == "/api/dev/test-access" {
        return Some(dev_test_access(state, method, headers));
    }
    if path == "/api/cache/refresh"
        || path == "/api/admin/cache/refresh"
        || path == "/api/refresh-cache"
    {
        let ip = crate::handler::get_real_ip(headers, None);
        return Some(cache_refresh(state, method, headers, &ip, body));
    }
    if path == "/api/weather" && *method == Method::GET {
        return Some(crate::routes::dayboard::weather().await);
    }
    if path == "/api/school-calendar" && *method == Method::GET {
        return Some(crate::routes::dayboard::school_calendar().await);
    }
    if path == "/api/school-info" && *method == Method::GET {
        let school_key = if let Some(query) = _search.strip_prefix('?') {
            url::form_urlencoded::parse(query.as_bytes())
                .find(|(k, _)| k == "school")
                .map(|(_, v)| v.to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        return Some(crate::routes::dayboard::school_info(&school_key).await);
    }
    if path == "/api/bell/override" && *method == Method::GET {
        let override_val = state.store.read_document(
            &state.data_dir().join("bell_overrides.json"),
            Value::Null,
        );
        return Some(json_resp(200, serde_json::json!({ "override": override_val })));
    }
    if path == "/api/bad-passwords" && *method == Method::GET {
        return Some(bad_passwords(state));
    }
    if path == "/api/backgrounds/list" && *method == Method::GET {
        return Some(backgrounds_list(state));
    }
    if path == "/api/log-click" && *method == Method::POST {
        return Some(log_click(state, body));
    }
    if path == "/api/leaderboard" && *method == Method::GET {
        return Some(leaderboard(state, headers));
    }
    // GET /api/me/coins — server.js:23789-23802 (merged tree). Public wallet
    // for the signed-out top bar; the authed branch assembles coins,
    // achievements, and user stats.
    if path == "/api/me/coins" && *method == Method::GET {
        return Some(me_coins(state, headers));
    }
    if path == "/api/games" && (*method == Method::GET || *method == Method::POST) {
        return Some(games(state, body, headers));
    }
    // POST /api/presence/heartbeat (server.js:16307-16316) — the Step 11
    // presence writer; POST-only, everything else falls through.
    if path == "/api/presence/heartbeat" && *method == Method::POST {
        return Some(presence_heartbeat(state, headers, body_bytes));
    }
    None
}

/// `String(body.playing || '').trim()` — the presence heartbeat's playing
/// field. NOTE: unlike most heartbeat endpoints there is no isRevoked check.
fn presence_heartbeat(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cookies.get("id").unwrap_or(""))
        .to_string();
    if !mitch_lib::auth::valid_id(&sid, &state.id_secret) {
        return json_response(401, json!({ "error": "unauthorized" }));
    }
    let Some(email) = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid) else {
        return json_response(401, json!({ "error": "email not found" }));
    };
    let Some(body) = crate::routes::me::parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let playing = jsval::string(&jsval::or(body.get("playing"), json!("")));
    let playing = playing.trim();
    crate::ws::touch_user_presence(state, &email, playing);
    json_response(200, json!({ "ok": true }))
}

fn json_response(code: u16, obj: serde_json::Value) -> Response {
    crate::errors::json_resp(code, obj)
}

/// `GET /api/site-info` — server.js:7912-7918.
fn site_info(state: &Arc<AppState>) -> Response {
    let site = state
        .store
        .read_document(&state.cfg.data_dir.join("site.json"), json!({}));
    json_response(
        200,
        json!({
            "primary": site.get("primary").and_then(|v| v.as_str()).unwrap_or("https://mitch.pro"),
            "alternate": site.get("alternate").and_then(|v| v.as_str()).unwrap_or(""),
            "name": site.get("name").and_then(|v| v.as_str()).unwrap_or("mitch.pro"),
        }),
    )
}

/// Homepage destination catalog. Keep the existing line-format response used
/// by parseSites(), including edits made through the admin content tools.
fn site_catalog(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    if !state.check_password_cookie(headers, None) {
        return json_response(200, json!({ "success": false }));
    }

    let raw = match std::fs::read_to_string(state.cfg.data_dir.join("sites")) {
        Ok(raw) => raw,
        Err(error) => {
            tracing::error!("failed to read homepage site catalog: {error}");
            return json_response(500, json!({ "success": false, "error": "catalog_unavailable" }));
        }
    };
    let cookie_header = headers
        .get(axum::http::header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let cookies = mitch_lib::auth::get_cookies_from_header_value(
        cookie_header,
        &state.store,
        &state.id_secret,
        std::env::var("NODE_ENV").unwrap_or_default() == "test",
    );
    let is_admin = mitch_lib::auth::is_admin_id(
        &state.store,
        &state.id_secret,
        &cookies.auth_sid(),
        std::env::var("NODE_ENV").unwrap_or_default() == "test",
    );
    json_response(
        200,
        json!({ "success": true, "content": visible_site_catalog(&raw, is_admin) }),
    )
}

fn visible_site_catalog(raw: &str, is_admin: bool) -> String {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("//"))
        .filter(|line| is_admin || !line.starts_with("admin "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `GET /api/bad-passwords` — server.js:20450-20459. Raw file passthrough.
fn bad_passwords(state: &Arc<AppState>) -> Response {
    let file = state.cfg.data_dir.join("bad_passwords.json");
    match std::fs::read_to_string(&file) {
        Ok(raw) => Response::builder()
            .status(StatusCode::OK)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(raw))
            .unwrap_or_else(|_| json_response(500, json!([]))),
        Err(_) => json_response(200, json!([])),
    }
}

/// `GET /api/backgrounds/list` — server.js:8448-8469 (without the ffmpeg
/// conversion side effect — webp/webm files only, sorted by filename).
fn backgrounds_list(state: &Arc<AppState>) -> Response {
    let dir = state.cfg.base_dir.join("webserver/backgrounds");
    let items = list_backgrounds(&dir);
    json_response(200, json!({ "ok": true, "items": items }))
}

fn list_backgrounds(dir: &std::path::Path) -> serde_json::Value {
    let mut items = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut files: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_lowercase();
                name.ends_with(".webp") || name.ends_with(".webm")
            })
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        files.sort();
        for file in files {
            let stem = file
                .trim_end_matches(".webp")
                .trim_end_matches(".webm")
                .trim_start_matches("bg-");
            let name = title_case(&stem.replace('-', " "));
            let url = format!("/backgrounds/{file}");
            let ty = if file.ends_with(".webm") {
                "video"
            } else {
                "image"
            };
            items.push(json!({ "id": stem, "name": name, "url": url, "type": ty }));
        }
    }
    serde_json::Value::Array(items)
}

fn title_case(s: &str) -> String {
    s.split(' ')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &c.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `POST /api/log-click` — server.js:12662-12677. Appends to heatmap.json
/// (DB-stored), trimmed to the last 1000 entries per page.
fn log_click(state: &Arc<AppState>, body: &serde_json::Value) -> Response {
    let page = body.get("page").and_then(|v| v.as_str()).unwrap_or("");
    let x = body.get("x").and_then(|v| v.as_f64());
    let y = body.get("y").and_then(|v| v.as_f64());
    if page.is_empty() || x.is_none() || y.is_none() {
        return json_response(400, json!({ "error": "invalid data" }));
    }
    let file = state.cfg.data_dir.join("heatmap.json");
    let mut heatmap = state.store.read_document(&file, json!({}));
    let entry = json!({
        "x": x.unwrap_or(0.0),
        "y": y.unwrap_or(0.0),
    });
    if let Some(map) = heatmap.as_object_mut() {
        let list = map.entry(page.to_string()).or_insert(json!([]));
        if let Some(arr) = list.as_array_mut() {
            arr.push(entry);
            if arr.len() > 1000 {
                arr.drain(0..arr.len() - 1000);
            }
        }
    }
    match state.store.write_document(&file, &heatmap) {
        Ok(()) => json_response(200, json!({ "ok": true })),
        Err(e) => json_response(400, json!({ "error": e.to_string() })),
    }
}

/// `GET /api/leaderboard` — server.js:20489-20535. Ranked by coins desc,
/// tie-break name asc. Coins come from the real `coins` table now; stats,
/// profiles, and cosmetics are still read from their JSON-blob documents.
fn leaderboard(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let cookie_header = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let cookies = mitch_lib::auth::get_cookies_from_header_value(
        cookie_header,
        &state.store,
        &state.id_secret,
        false,
    );
    let sid = cookies.auth_sid();
    let viewer_email = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid);
    let viewer_norm = viewer_email
        .as_deref()
        .map(mitch_lib::auth::normalize_email)
        .unwrap_or_default();

    // coins now lives in a real table, not the (no-longer-written, so
    // increasingly stale) coins.json blob — go through the function that
    // actually reads it.
    let coins = mitch_lib::coins::load_coins(&state.store, state.data_dir());
    let profiles = state
        .store
        .read_document(&state.data_dir().join("profiles.json"), json!({}));
    let cosmetics = state
        .store
        .read_document(&state.data_dir().join("cosmetics.json"), json!({}));
    let user_stats = state
        .store
        .read_document(&state.data_dir().join("user_stats.json"), json!({}));

    let mut keys: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for source in [&coins, &profiles] {
        if let Some(map) = source.as_object() {
            for k in map.keys() {
                if seen.insert(k.clone()) {
                    keys.push(k.clone());
                }
            }
        }
    }
    if !viewer_norm.is_empty() && seen.insert(viewer_norm.clone()) {
        keys.push(viewer_norm.clone());
    }

    let mut rows: Vec<serde_json::Value> = Vec::new();
    for norm in &keys {
        let profile = profiles.get(norm).and_then(|v| v.as_object());
        let stats = user_stats.get(norm).and_then(|v| v.as_object());
        let cosm = cosmetics.get(norm).and_then(|v| v.as_object());
        let nickname = profile
            .and_then(|p| p.get("nickname"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let display_name = profile
            .and_then(|p| p.get("displayName"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let username = profile
            .and_then(|p| p.get("username"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let name: &str = if !nickname.is_empty() {
            nickname
        } else if !display_name.is_empty() {
            display_name
        } else if !username.is_empty() {
            username
        } else {
            &default_username_for_email(norm)
        };
        let field = |obj: Option<&serde_json::Map<String, Value>>, key: &str| -> Value {
            obj.and_then(|o| o.get(key)).cloned().unwrap_or(json!(0))
        };
        let badge = cosm
            .and_then(|c| c.get("activeBadge"))
            .cloned()
            .unwrap_or(Value::Null);
        rows.push(json!({
            "name": name,
            "coins": coins.get(norm).cloned().unwrap_or(json!(0)),
            "wins": field(stats, "chess_wins"),
            "puzzles": field(stats, "puzzles_solved"),
            "pixels": field(stats, "pixels"),
            "clicker_pts": field(stats, "clicker_points"),
            "clicker_coins": field(stats, "clicker_coins"),
            "typing_races": field(stats, "typing_races"),
            "typing_coins": field(stats, "typing_coins"),
            "logic_puzzles": field(stats, "logic_puzzles"),
            "logic_coins": field(stats, "logic_coins"),
            "email": if !username.is_empty() { json!(username) } else { json!(norm) },
            "badge": badge,
            "isMe": norm == &viewer_norm,
        }));
    }
    // Sort by coins desc, tie-break name asc. Rank is implicit (1-based).
    rows.sort_by(|a, b| {
        let ca = a.get("coins").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let cb = b.get("coins").and_then(|v| v.as_f64()).unwrap_or(0.0);
        cb.partial_cmp(&ca)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                let na = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let nb = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
                na.cmp(nb)
            })
    });
    let total = rows.len();
    let top = rows.iter().take(10).cloned().collect::<Vec<_>>();
    let players = rows.iter().take(100).cloned().collect::<Vec<_>>();
    let me_idx = rows
        .iter()
        .position(|r| r.get("isMe").and_then(|v| v.as_bool()).unwrap_or(false));
    let me_val = me_idx.map(|p| rows[p].clone()).unwrap_or(Value::Null);
    let me_out = match me_idx {
        Some(p) if p < 10 => Value::Null, // viewer is in the top 10, no separate "me"
        Some(_) => me_val,
        None => me_val, // still include if resolvable
    };
    json_response(
        200,
        json!({ "top": top, "me": me_out, "players": players, "total": total }),
    )
}

fn default_username_for_email(norm: &str) -> String {
    norm.split('@').next().unwrap_or(norm).to_string()
}

impl AppState {
    /// `data/<name>` path under the data dir.
    pub fn data_dir(&self) -> &std::path::Path {
        &self.cfg.data_dir
    }
}

/// `GET /api/games` — server.js:12705-12783. Reads the line-format game lists
/// + categories, applies filters, returns content + featured.
fn games(state: &Arc<AppState>, body: &serde_json::Value, headers: &HeaderMap) -> Response {
    let cookie_header = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let cookies = mitch_lib::auth::get_cookies_from_header_value(
        cookie_header,
        &state.store,
        &state.id_secret,
        std::env::var("NODE_ENV").unwrap_or_default() == "test",
    );
    let hash = body
        .get("hash")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| cookies.auth_sid());

    if hash.is_empty() || !mitch_lib::auth::valid_id(&hash, &state.id_secret) {
        return json_response(200, json!({ "success": false, "error": "no_valid_token" }));
    }

    let q = body
        .get("q")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    let cat = body
        .get("cat")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    let offset = body.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let limit = body
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(50)
        .clamp(1, 200) as usize;

    // Read the line-format game lists (PRESERVED disk files).
    let mut game_lines: Vec<String> = Vec::new();
    for name in ["games", "games_local", "games_external"] {
        let path = state.cfg.data_dir.join(name);
        if let Ok(content) = std::fs::read_to_string(&path) {
            game_lines.extend(content.lines().map(str::to_string));
        }
    }
    // Dedup by href (second space-separated field).
    let mut seen = std::collections::HashSet::new();
    let all: Vec<String> = game_lines
        .into_iter()
        .filter(|line| {
            let href = line.split(' ').nth(1).unwrap_or("");
            seen.insert(href.to_string())
        })
        .collect();

    // Categories from the three PRESERVED files (external overrides local).
    let mut cats = state
        .store
        .read_document(&state.cfg.data_dir.join("game_categories.json"), json!({}));
    let local = state.store.read_document(
        &state.cfg.data_dir.join("game_categories_local.json"),
        json!({}),
    );
    let external = state.store.read_document(
        &state.cfg.data_dir.join("game_categories_external.json"),
        json!({}),
    );
    for source in [local, external] {
        if let Some(map) = source.as_object() {
            for (k, v) in map {
                if let Some(target) = cats.as_object_mut() {
                    target.insert(k.clone(), v.clone());
                }
            }
        }
    }
    let cats = cats;

    let filtered: Vec<&String> = all
        .iter()
        .filter(|line| {
            let href = line.split(' ').nth(1).unwrap_or("");
            let label = line.splitn(3, ' ').nth(2).unwrap_or("");
            if !cat.is_empty() && cat != "all" {
                let category = cats
                    .get(href)
                    .or_else(|| cats.get(href.trim_start_matches("/games/")))
                    .and_then(|v| v.as_str())
                    .unwrap_or("other");
                if !category.to_lowercase().contains(&cat) && category.to_lowercase() != cat {
                    return false;
                }
            }
            if !q.is_empty() && !label.to_lowercase().contains(&q) {
                return false;
            }
            true
        })
        .collect();
    let total = filtered.len();
    let end = (offset + limit).min(total);
    let page: Vec<&String> = filtered
        .iter()
        .skip(offset)
        .take(end - offset)
        .cloned()
        .collect();
    let content: Vec<String> = page
        .iter()
        .map(|s| s.replace("https://html5.gamemonetize.co/", "/proxy/gamemonetize/"))
        .collect();

    // Featured: top 8 by play count, only on page 0 with no filters.
    let featured = if offset == 0 && q.is_empty() && (cat.is_empty() || cat == "all") {
        let stats = state
            .store
            .read_document(&state.data_dir().join("global_game_stats.json"), json!({}));
        let mut ranked: Vec<(&String, u64)> = all
            .iter()
            .map(|line| {
                let href = line.split(' ').nth(1).unwrap_or("");
                let count = stats.get(href).and_then(|v| v.as_u64()).unwrap_or(0);
                (line, count)
            })
            .collect();
        ranked.sort_by_key(|a| std::cmp::Reverse(a.1));
        let top: Vec<String> = ranked
            .iter()
            .take(8)
            .map(|(line, _)| line.replace("https://html5.gamemonetize.co/", "/proxy/gamemonetize/"))
            .collect();
        top.join("\n")
    } else {
        String::new()
    };

    json_response(
        200,
        json!({
            "success": true,
            "content": content.join("\n"),
            "featured": featured,
            "total": total,
            "offset": offset,
            "limit": limit,
            "hasMore": offset + limit < total,
        }),
    )
}

/// `GET /api/me/coins` — server.js:23789-23802. Signed-out visitors get a
/// clean `{authenticated: false, coins: null}` (the wallet renders in the
/// public top bar); authed sessions get coins + achievements + user stats.
fn me_coins(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let cookie_header = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let cookies = mitch_lib::auth::get_cookies_from_header_value(
        cookie_header,
        &state.store,
        &state.id_secret,
        false,
    );
    let sid = cookies.auth_sid();
    if !mitch_lib::auth::valid_id(&sid, &state.id_secret) {
        return json_response(200, json!({ "authenticated": false, "coins": null }));
    }
    let email = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid);
    let Some(email) = email else {
        return json_response(200, json!({ "authenticated": false, "coins": null }));
    };
    let norm = mitch_lib::auth::normalize_email(&email);
    let coins = mitch_lib::coins::get_coins(&state.store, state.data_dir(), &email);
    let achievements = state
        .store
        .read_document(&state.data_dir().join("achievements.json"), json!({}))
        .get(norm.as_str())
        .cloned()
        .unwrap_or(json!([]));
    let stats = state
        .store
        .read_document(&state.data_dir().join("user_stats.json"), json!({}))
        .get(norm.as_str())
        .cloned()
        .unwrap_or(json!({}));
    json_response(
        200,
        json!({
            "authenticated": true,
            "coins": jsval::num_value(coins),
            "achievements": achievements,
            "stats": stats,
        }),
    )
}

/// `OPTIONS|GET|HEAD /api/verify-open` & `/verify-open.json` — server.js:10517-10548.
pub fn verify_open(method: &Method) -> Response {
    if *method == Method::OPTIONS {
        return Response::builder()
            .status(StatusCode::NO_CONTENT)
            .header("access-control-allow-origin", "*")
            .header("access-control-allow-methods", "GET, HEAD, OPTIONS")
            .header("access-control-allow-headers", "*")
            .header("cache-control", "no-cache, no-store, must-revalidate")
            .body(axum::body::Body::empty())
            .unwrap_or_else(|_| crate::errors::err_resp(500, None, None));
    }
    if *method == Method::GET || *method == Method::HEAD {
        let payload = json!({
            "status": "open",
            "domain": "mitch.pro",
            "verified": true,
            "token": "mitch-open-verified-2026",
        });
        let body = if *method == Method::HEAD {
            axum::body::Body::empty()
        } else {
            axum::body::Body::from(serde_json::to_string(&payload).unwrap_or_default())
        };
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json; charset=utf-8")
            .header("access-control-allow-origin", "*")
            .header("access-control-allow-methods", "GET, HEAD, OPTIONS")
            .header("access-control-allow-headers", "*")
            .header("cache-control", "no-cache, no-store, must-revalidate")
            .header("pragma", "no-cache")
            .body(body)
            .unwrap_or_else(|_| crate::errors::err_resp(500, None, None));
    }
    crate::errors::err_resp(405, None, None)
}

/// `GET /api/guest-session` — server.js:10584-10590.
fn guest_session(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    if state.check_password_cookie(headers, None) {
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .header("cache-control", "no-store")
            .body(axum::body::Body::from(r#"{"authenticated":true}"#))
            .unwrap_or_else(|_| crate::errors::err_resp(500, None, None));
    }

    let cookie_header = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let cookies = mitch_lib::auth::get_cookies_from_header_value(
        cookie_header,
        &state.store,
        &state.id_secret,
        false,
    );
    let guest_token = cookies.get("mitch_guest").unwrap_or("");

    let secret = mitch_lib::crypto::hmac_sha256(&state.id_secret, b"guest-preview-v1");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let preview = mitch_lib::guest::guest_preview(guest_token, &secret, now);
    let secure_flag = std::env::var("SESSION_COOKIE_SECURE").unwrap_or_default();
    let node_env_prod = std::env::var("NODE_ENV").unwrap_or_default() == "production";
    let cookie_hdr = mitch_lib::auth::set_cookie_header(
        "mitch_guest",
        &preview.token,
        &secure_flag,
        node_env_prod,
        31536000,
        true,
    );

    let body = json!({
        "authenticated": false,
        "expiresAt": preview.expires_at,
        "serverNow": preview.server_now,
    });

    let mut resp = Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .header("cache-control", "no-store")
        .body(axum::body::Body::from(
            serde_json::to_string(&body).unwrap_or_default(),
        ))
        .unwrap_or_else(|_| crate::errors::err_resp(500, None, None));

    if let Ok(hv) = axum::http::HeaderValue::from_str(&cookie_hdr) {
        resp.headers_mut()
            .append(axum::http::header::SET_COOKIE, hv);
    }
    resp
}

const DEV_TEST_EMAIL: &str = "admin@mitch.pro";

fn dev_test_access_enabled() -> bool {
    let node_env = std::env::var("NODE_ENV").unwrap_or_default();
    let dev_access = std::env::var("DEV_TEST_ACCESS").unwrap_or_default();
    node_env != "production" && dev_access == "1"
}

fn is_private_ip(ip: &str) -> bool {
    if ip.is_empty() {
        return false;
    }
    if ip == "127.0.0.1" || ip == "::1" || ip == "localhost" {
        return true;
    }
    if ip.starts_with("10.")
        || ip.starts_with("192.168.")
        || ip.starts_with("169.254.")
        || ip.starts_with("100.")
    {
        return true;
    }
    if let Some(rest) = ip.strip_prefix("172.") {
        if let Some(dot) = rest.find('.') {
            if let Ok(octet) = rest[..dot].parse::<u8>() {
                if (16..=31).contains(&octet) {
                    return true;
                }
            }
        }
    }
    if ip.len() >= 2 {
        let prefix = &ip[..2].to_ascii_lowercase();
        if prefix == "fc" || prefix == "fd" {
            return true;
        }
        if ip.len() >= 4 {
            let p4 = &ip[..4].to_ascii_lowercase();
            if p4 == "fe80" || p4 == "fe90" || p4 == "fea0" || p4 == "feb0" {
                return true;
            }
        }
    }
    false
}

fn dev_test_request_allowed(headers: &HeaderMap) -> bool {
    if !dev_test_access_enabled() {
        return false;
    }
    let host = crate::hosts::request_host(headers);
    let hostname = host.split(':').next().unwrap_or("").to_ascii_lowercase();
    hostname == "localhost"
        || hostname == "127.0.0.1"
        || hostname == "::1"
        || is_private_ip(&hostname)
}

/// `GET|POST /api/dev/test-access` — server.js:10564-10582.
fn dev_test_access(state: &Arc<AppState>, method: &Method, headers: &HeaderMap) -> Response {
    if !dev_test_request_allowed(headers) {
        return crate::errors::err_resp(404, None, None);
    }
    if *method == Method::GET {
        return json_response(200, json!({ "enabled": true, "label": "Test Mitch.pro" }));
    }
    if *method == Method::POST {
        mitch_lib::profile::ensure_profile_defaults(
            &state.store,
            state.data_dir(),
            &state.id_secret,
            DEV_TEST_EMAIL,
            DEV_TEST_EMAIL,
            &json!({}),
        );
        let user_agent = headers
            .get(axum::http::header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let ip = crate::handler::get_real_ip(headers, None);
        let session = mitch_lib::auth::create_auth_session(
            &state.store,
            &state.id_secret,
            DEV_TEST_EMAIL,
            DEV_TEST_EMAIL,
            user_agent,
            &ip,
            true, // dev_superuser
        );
        let secure_flag = std::env::var("SESSION_COOKIE_SECURE").unwrap_or_default();
        let node_env_production = std::env::var("NODE_ENV").unwrap_or_default() == "production";
        let max_age = mitch_lib::auth::AUTH_SESSION_TTL_MS / 1000;
        let mut resp = json_response(
            200,
            json!({
                "success": true,
                "devTest": true,
                "roles": ["owner", "admin", "moderator", "premium"],
                "id": session.sid,
                "email": DEV_TEST_EMAIL,
            }),
        );
        let append = |resp: &mut Response, value: String| {
            if let Ok(hv) = axum::http::HeaderValue::from_str(&value) {
                resp.headers_mut()
                    .append(axum::http::header::SET_COOKIE, hv);
            }
        };
        append(
            &mut resp,
            mitch_lib::auth::set_cookie_header(
                "mitch_session",
                &session.token,
                &secure_flag,
                node_env_production,
                max_age,
                true,
            ),
        );
        append(
            &mut resp,
            mitch_lib::auth::set_cookie_header(
                "studentId",
                &session.sid,
                &secure_flag,
                node_env_production,
                max_age,
                false,
            ),
        );
        append(
            &mut resp,
            mitch_lib::auth::clear_cookie_header(
                "password",
                &secure_flag,
                node_env_production,
                false,
            ),
        );
        append(
            &mut resp,
            mitch_lib::auth::clear_cookie_header("id", &secure_flag, node_env_production, false),
        );
        return resp;
    }
    crate::errors::err_resp(405, None, None)
}

fn is_authorized_cache_refresh(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    client_ip: &str,
) -> bool {
    if std::env::var("NODE_ENV").unwrap_or_default() == "test" {
        return true;
    }
    let configured_secret = std::env::var("DEPLOY_SECRET")
        .or_else(|_| std::env::var("SECRET_KEY"))
        .unwrap_or_default()
        .trim()
        .to_string();

    let auth_header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim();
    let bearer_token = if auth_header.to_ascii_lowercase().starts_with("bearer ") {
        auth_header[7..].trim()
    } else {
        ""
    };
    let token_header = headers
        .get("x-deploy-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim();
    let token = if !bearer_token.is_empty() {
        bearer_token
    } else {
        token_header
    };

    if !configured_secret.is_empty() && !token.is_empty() && token == configured_secret {
        return true;
    }

    let raw_mitch = headers
        .get("x-mitch-client-ip")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim();
    let raw_real = headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim();
    let xff = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim();
    let is_direct_loopback = raw_mitch.is_empty()
        && raw_real.is_empty()
        && xff.is_empty()
        && (client_ip == "127.0.0.1" || client_ip == "::1");

    if is_direct_loopback {
        let internal_refresh = headers
            .get("x-internal-refresh")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .trim();
        if internal_refresh == "1" {
            return true;
        }
    }

    let cookie_header = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let cookies = mitch_lib::auth::get_cookies_from_header_value(
        cookie_header,
        &state.store,
        &state.id_secret,
        false,
    );
    let sid = cookies.auth_sid();
    if mitch_lib::auth::valid_id(&sid, &state.id_secret)
        && mitch_lib::auth::is_any_admin_id(&state.store, &state.id_secret, &sid, false)
    {
        return true;
    }

    false
}

/// `GET|POST /api/cache/refresh` (and aliases) — server.js:10624-10655.
fn cache_refresh(
    state: &Arc<AppState>,
    method: &Method,
    headers: &HeaderMap,
    client_ip: &str,
    body: &Value,
) -> Response {
    if *method == Method::GET {
        let (size, bytes, max_bytes) = state.static_cache.stats();
        return json_response(
            200,
            json!({
                "cacheSize": size,
                "cacheBytes": bytes,
                "maxBytes": max_bytes,
            }),
        );
    }
    if *method != Method::POST {
        return crate::errors::err_resp(405, Some("method not allowed"), None);
    }
    if !is_authorized_cache_refresh(state, headers, client_ip) {
        return json_response(
            401,
            json!({
                "error": "unauthorized",
                "message": "Valid deploy token or secret key required",
            }),
        );
    }

    let mut files_to_refresh = Vec::new();
    if let Some(files) = body.get("files").and_then(|v| v.as_array()) {
        for f in files {
            if let Some(s) = f.as_str() {
                let st = s.trim();
                if !st.is_empty() {
                    files_to_refresh.push(st.to_string());
                }
            }
        }
    }

    let result =
        state
            .static_cache
            .clear(&state.cfg.webroot, &state.cfg.base_dir, &files_to_refresh);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    json_response(
        200,
        json!({
            "success": true,
            "message": "Static cache refreshed",
            "evicted": result.evicted,
            "reloaded": result.reloaded,
            "full": result.full,
            "selective": !result.full,
            "timestamp": now,
        }),
    )
}

fn health_check() -> Response {
    json_response(
        200,
        json!({
            "status": "ok",
            "uptime": 0
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_catalog_filters_comments_and_staff_links() {
        let raw = "// hidden\nurl /matrix/ Matrix Chat\nadmin url /admin/ Admin\n\nurl /casino/ Casino\n";
        assert_eq!(
            visible_site_catalog(raw, false),
            "url /matrix/ Matrix Chat\nurl /casino/ Casino"
        );
        assert_eq!(
            visible_site_catalog(raw, true),
            "url /matrix/ Matrix Chat\nadmin url /admin/ Admin\nurl /casino/ Casino"
        );
    }

    #[test]
    fn test_verify_open_options() {
        let res = verify_open(&Method::OPTIONS);
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            res.headers()
                .get("access-control-allow-origin")
                .unwrap()
                .to_str()
                .unwrap(),
            "*"
        );
    }

    #[test]
    fn test_verify_open_get() {
        let res = verify_open(&Method::GET);
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers().get("content-type").unwrap().to_str().unwrap(),
            "application/json; charset=utf-8"
        );
    }

    #[test]
    fn test_is_private_ip() {
        assert!(is_private_ip("127.0.0.1"));
        assert!(is_private_ip("::1"));
        assert!(is_private_ip("localhost"));
        assert!(is_private_ip("10.0.0.1"));
        assert!(is_private_ip("192.168.1.100"));
        assert!(is_private_ip("172.16.0.5"));
        assert!(is_private_ip("172.31.255.255"));
        assert!(!is_private_ip("172.32.0.1"));
        assert!(!is_private_ip("8.8.8.8"));
        assert!(!is_private_ip("1.1.1.1"));
    }

    #[test]
    fn test_health_check() {
        let res = health_check();
        assert_eq!(res.status(), StatusCode::OK);
    }
}
