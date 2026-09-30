//! Captcha proxy + content injection — port of server.js's World's Hardest
//! Captcha API Proxy block (~7982-8027) and /api/content (~12932-13004).
//!
//! Captcha proxy: transparent HTTP relay to worldshardestcaptcha.com with
//! header stripping and method passthrough. /api/content serves HTML with
//! the injection ladder (IS_PREMIUM, sync.js, assistant.js, broadcast.js,
//! agree footer) behind a hash-validation auth ladder.
#![allow(clippy::expect_used)] // infallible static responses + static regexes

use crate::errors::json_resp;
use crate::state::AppState;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde_json::json;
use serde_json::Value;
use std::sync::Arc;

const CAPTCHA_TARGET: &str = "https://www.worldshardestcaptcha.com";

/// Paths relayed to worldshardestcaptcha.com.
pub fn is_captcha_proxy_path(path: &str) -> bool {
    path == "/api/token"
        || path.starts_with("/api/puzzle/")
        || path == "/api/solve"
        || path == "/api/submit"
        || path == "/api/stats"
        || path == "/api/next"
        || path.starts_with("/images/")
}

/// Transparent captcha proxy — server.js:7982-8027.
pub async fn captcha_proxy(
    _state: &Arc<AppState>,
    method: &axum::http::Method,
    path: &str,
    headers: &HeaderMap,
    search: &str,
    body_bytes: &[u8],
) -> Option<Response> {
    if !is_captcha_proxy_path(path) {
        return None;
    }
    let target = format!("{CAPTCHA_TARGET}{path}{search}");
    let client = match reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36")
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(_) => return Some(bad_gateway()),
    };

    let reqwest_m = match method.as_str() {
        "POST" => reqwest::Method::POST,
        "PUT" => reqwest::Method::PUT,
        "DELETE" => reqwest::Method::DELETE,
        "PATCH" => reqwest::Method::PATCH,
        "HEAD" => reqwest::Method::HEAD,
        _ => reqwest::Method::GET,
    };
    let mut req = client.request(reqwest_m, &target);
    // Copy request headers (stripping the proxy-forbidden set).
    const SKIP: &[&str] = &[
        "host",
        "cookie",
        "referer",
        "origin",
        "accept-encoding",
        "x-mitch-client-ip",
    ];
    for (name, value) in headers.iter() {
        let name_lower = name.as_str().to_lowercase();
        if SKIP.contains(&name_lower.as_str()) {
            continue;
        }
        if let Ok(v) = value.to_str() {
            req = req.header(name.as_str(), v);
        }
    }
    // Force Origin/Referer to the captcha site.
    req = req
        .header("Origin", CAPTCHA_TARGET)
        .header("Referer", CAPTCHA_TARGET);

    if !method.is_safe() && !body_bytes.is_empty() {
        req = req.body(body_bytes.to_vec());
    }

    match req.send().await {
        Ok(upstream) => {
            let status = axum::http::StatusCode::from_u16(upstream.status().as_u16())
                .unwrap_or(StatusCode::BAD_GATEWAY);
            let mut builder = Response::builder().status(status);
            for (name, value) in upstream.headers().iter() {
                let n = name.as_str().to_lowercase();
                if n == "content-security-policy"
                    || n == "x-frame-options"
                    || n == "content-encoding"
                    || n == "content-length"
                {
                    continue;
                }
                builder = builder.header(name, value);
            }
            builder = builder.header("access-control-allow-origin", "*");
            let body = upstream.bytes().await.unwrap_or_default();
            Some(
                builder
                    .body(axum::body::Body::from(body))
                    .unwrap_or_else(|_| bad_gateway()),
            )
        }
        Err(_) => Some(bad_gateway()),
    }
}

fn bad_gateway() -> Response {
    json_resp(
        502,
        json!({"error": "Bad Gateway", "message": "Failed to proxy captcha API."}),
    )
}

/// `GET|POST /api/content` — server.js:12932-13004.
pub async fn content(
    state: &Arc<AppState>,
    body: &serde_json::Value,
    _headers: &HeaderMap,
) -> Option<Response> {
    let hash = body.get("hash").and_then(|v| v.as_str()).unwrap_or("");
    let content_path = body.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let pathname = url::Url::parse(&format!("http://x/{content_path}"))
        .map(|u| u.path().trim_start_matches('/').to_string())
        .unwrap_or_default();
    let rel = if pathname.is_empty() || pathname.ends_with('/') {
        format!("{pathname}index.html")
    } else {
        pathname
    };

    // Auth ladder: revoked > invalidated > valid_hashes.json array fallback.
    let revoked = state
        .store
        .read_document(&state.data_dir().join("revoked.json"), json!({}))
        .get(hash)
        .cloned();
    let invalidated = state
        .store
        .read_document(&state.data_dir().join("invalidated.json"), json!({}))
        .get(hash)
        .cloned();
    let revoked = revoked.map(|v| v.as_bool().unwrap_or(true));
    let invalidated = invalidated.map(|v| v.as_bool().unwrap_or(false));
    if revoked == Some(true) {
        return Some(json_response(
            200,
            json!({
                "content": "<!DOCTYPE html><html><head><title>403</title></head><body>Access Revoked</body></html>",
                "revoked": true,
            }),
        ));
    }
    if invalidated == Some(true) {
        return Some(json_response(
            200,
            json!({
                "content": "<!DOCTYPE html><html><head><title>403</title></head><body>Access Revoked</body></html>",
                "revoked": true,
            }),
        ));
    }

    // Valid-hashes fallback.
    let valid_hashes = state
        .store
        .read_document(&state.data_dir().join("valid_hashes.json"), json!([]));
    let hash_ok = mitch_lib::auth::valid_id(hash, &state.id_secret)
        && revoked.is_none()
        && invalidated != Some(true);
    if !hash_ok
        && !valid_hashes
            .as_array()
            .map(|a| a.contains(&json!(hash)))
            .unwrap_or(false)
    {
        return Some(json_response(
            200,
            json!({
                "content": "<!DOCTYPE html><html><head><title>401</title></head><body>Not enrolled</body></html>",
            }),
        ));
    }

    // Path safety.
    let webroot = state.cfg.base_dir.join("webserver");
    let Some(file_path) = crate::static_files::safe_webroot_path(&webroot, &format!("/{rel}"))
    else {
        return Some(json_response(
            200,
            json!({
                "content": "<!DOCTYPE html><html><head><title>403</title></head><body>Forbidden</body></html>",
            }),
        ));
    };
    if rel.starts_with("simulate/") {
        return Some(json_response(
            200,
            json!({
                "content": "<!DOCTYPE html><html><head><title>410</title></head><body>Gone</body></html>",
            }),
        ));
    }
    let Ok(html) = std::fs::read_to_string(&file_path) else {
        return Some(json_response(
            200,
            json!({
                "content": "<!DOCTYPE html><html><head><title>404</title></head><body>Not found</body></html>",
            }),
        ));
    };

    // Injection ladder: IS_PREMIUM (canvas/chess-bot only), sync.js, assistant.js, broadcast.js, agree_footer.
    let mut html = html;
    let is_canvas_or_chess = rel.starts_with("canvas/") || rel.starts_with("games/chess-bot/");
    if is_canvas_or_chess && !html.contains("IS_PREMIUM") {
        let premium = is_premium_email(state, hash);
        html = html.replacen(
            "</head>",
            &format!("<script>var IS_PREMIUM = {premium};</script>\n</head>"),
            1,
        );
    }
    if !html.contains("/sync.js") {
        if let Some(idx) = html.rfind("</body>") {
            html = format!(
                "{}<script src=\"/sync.js\" defer></script>{}",
                &html[..idx],
                &html[idx..]
            );
        }
    }
    if !html.contains("/assistant.js") && !rel.starts_with("encrypt/") {
        if let Some(idx) = html.rfind("</body>") {
            html = format!(
                "{}<script src=\"/assistant.js\" defer></script>{}",
                &html[..idx],
                &html[idx..]
            );
        }
    }
    if !html.contains("/broadcast.js") {
        if let Some(idx) = html.rfind("</body>") {
            html = format!(
                "{}<script src=\"/broadcast.js?v=4\" defer></script>{}",
                &html[..idx],
                &html[idx..]
            );
        }
    }
    if !html.contains("_agree_footer") {
        let footer = "<div id=\"_agree_footer\" style=\"position:fixed;bottom:5px;left:0;right:0;text-align:center;pointer-events:none;z-index:2147483647;font-size:.65rem;color:rgba(255,255,255,.15);\">By using mitch.pro you agree to the <a href=\"/use-agreement.html\" style=\"color:rgba(255,255,255,.15);pointer-events:all;\" target=\"_blank\">use agreement</a>.</div>";
        match html.rfind("</body>") {
            Some(idx) => html = format!("{}{}{}", &html[..idx], footer, &html[idx..]),
            None => html = format!("{html}{footer}"),
        }
    }

    let featured = state
        .store
        .read_document(&state.data_dir().join("featured_game.json"), json!(""))
        .as_str()
        .unwrap_or("")
        .to_string();
    Some(json_response(
        200,
        json!({ "content": html, "featured": featured }),
    ))
}

fn is_premium_email(state: &Arc<AppState>, hash: &str) -> bool {
    // The hash IS the sid: resolve to email, then the full premium ladder.
    let Some(email) = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, hash) else {
        return false;
    };
    mitch_lib::auth::is_premium_email(&state.store, &email)
}

fn json_response(code: u16, obj: serde_json::Value) -> Response {
    crate::errors::json_resp(code, obj)
}

pub async fn ping(
    state: &Arc<AppState>,
    body: &serde_json::Value,
    headers: &HeaderMap,
) -> Option<Response> {
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
    let id = body
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| cookies.auth_sid());
    let mut page = body
        .get("page")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if page.starts_with("/proxy/gamemonetize/") {
        page = format!(
            "https://html5.gamemonetize.co/{}",
            &page["/proxy/gamemonetize/".len()..]
        );
    }
    let category = body
        .get("category")
        .and_then(|v| v.as_str())
        .unwrap_or("Utilities")
        .to_string();

    // Update global game stats.
    let stats_file = state.data_dir().join("global_game_stats.json");
    if page.starts_with("https://html5.gamemonetize.co/") || page.starts_with("/games/") {
        let normalized = normalize_game_page(&page);
        if !normalized.is_empty() {
            let mut stats = state.store.read_document(&stats_file, json!({}));
            if let Some(map) = stats.as_object_mut() {
                *map.entry(normalized).or_insert(json!(0)) =
                    json!(map.get(&normalized).and_then(|v| v.as_u64()).unwrap_or(0) + 1);
            }
            let _ = state.store.write_document(&stats_file, &stats);
        }
    }

    // Session log (capped at 50000).
    let session_file = state.data_dir().join("sessions.json");
    let mut sessions = state.store.read_document(&session_file, json!([]));
    if let Some(arr) = sessions.as_array_mut() {
        arr.push(json!({ "id": id, "timestamp": now_millis_str() }));
        if arr.len() > 50000 {
            let excess = arr.len() - 50000;
            arr.drain(0..excess);
        }
        let _ = state.store.write_document(&session_file, &sessions);
    }

    // Resolve email for presence/coins (only when sid is valid).
    let email = if !id.is_empty() && mitch_lib::auth::valid_id(&id, &state.id_secret) {
        mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &id)
    } else {
        None
    };

    // Playtime stats + ping reward for the email (server.js:15663-15682).
    if let Some(ref em) = email {
        let norm = mitch_lib::auth::normalize_email(em);
        let stats_file = state.data_dir().join("user_stats.json");
        let mut stats = state.store.read_document(&stats_file, json!({}));
        if let Some(map) = stats.as_object_mut() {
            let user = map.entry(norm.clone()).or_insert(json!({}));
            if let Some(u) = user.as_object_mut() {
                let playtime = u.entry("playtime").or_insert(json!({}));
                if let Some(pt) = playtime.as_object_mut() {
                    let cat_entry = pt.entry(category.clone()).or_insert(json!(0));
                    let cur = cat_entry.as_i64().unwrap_or(0);
                    *cat_entry = json!(cur + 1);
                }
                // last_ping_reward: 0.25 coins per 60s (server.js:15668-15673).
                let now = now_millis() as f64;
                let last_reward = u
                    .get("last_ping_reward")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                if now - last_reward >= 60_000.0 {
                    mitch_lib::coins::add_coins(
                        &state.store,
                        state.data_dir(),
                        em,
                        0.25,
                        state.coin_multiplier(),
                        "",
                    );
                    u.insert("last_ping_reward".into(), json!(now));
                }
                if now
                    - u.get("last_active_at")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0)
                    >= 10_000.0
                {
                    u.insert("last_active_at".into(), json!(now));
                }
            }
        }
        let _ = state.store.write_document(&stats_file, &stats);
        let _ = norm;
    }

    // Presence leg (server.js:15617-15639): the playingGame ladder keyed off
    // the lowercased page, then touchUserPresence.
    if let Some(ref em) = email {
        let page_lower = page.to_lowercase();
        let playing_game = if page_lower.contains("/games/chess/") {
            "Chess".to_string()
        } else if page_lower.contains("/games/casino/") || page_lower.contains("/casino/") {
            "Casino".to_string()
        } else if page_lower.contains("/canvas/") {
            "Canvas".to_string()
        } else if page_lower.contains("/encrypt.html")
            || page_lower.contains("/encrypt/")
            || page_lower.contains("/matrix/")
        {
            "Chat".to_string()
        } else if page_lower.contains("/games/") {
            match regex::Regex::new(r"/games/([^/]+)").ok().and_then(|re| {
                re.captures(&page)
                    .and_then(|c| c.get(1))
                    .map(|m| m.as_str().to_string())
            }) {
                Some(m) => m,
                None => "Games".to_string(),
            }
        } else {
            String::new()
        };
        let playing = if playing_game.is_empty() {
            page.clone()
        } else {
            playing_game
        };
        crate::ws::touch_user_presence(state, em, &playing);
    }

    // `/api/ping`'s challenge list (server.js:15683-15689): filters
    // `c.to === normalizeEmail(em)` over the cvChallenges map — which is
    // provably always empty (the /challenge handler never stores), so this
    // stays [] in practice; kept for structural fidelity. Note the JS maps
    // `type: "c".type` — undefined, so the key is dropped by JSON.stringify.
    let challenges = if let Some(ref em) = email {
        let norm = mitch_lib::auth::normalize_email(em);
        let chs = state
            .cv_challenges
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        chs.values()
            .filter(|c| mitch_lib::jsval::string_of(c.get("to")) == norm)
            .map(|c| {
                let mut m = serde_json::Map::new();
                if let Some(v) = c.get("from") {
                    m.insert("from".into(), v.clone());
                }
                if let Some(v) = c.get("id") {
                    m.insert("id".into(), v.clone());
                }
                Value::Object(m)
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    Some(json_response(
        200,
        json!({ "success": true, "challenges": challenges }),
    ))
}

fn normalize_game_page(page: &str) -> String {
    let mut p = page.to_string();
    // Strip origin prefix.
    for origin in ["https://mitch.pro", "https://mitchdog.com"] {
        if p.starts_with(origin) {
            p = p[origin.len()..].to_string();
        }
    }
    // Unmangle proxy prefix back to the external URL.
    if p.starts_with("/proxy/gamemonetize/") {
        p = format!(
            "https://html5.gamemonetize.co/{}",
            &p["/proxy/gamemonetize/".len()..]
        );
    }
    // Count only game pages.
    if !p.starts_with("https://html5.gamemonetize.co/") && !p.starts_with("/games/") {
        return String::new();
    }
    if p.ends_with("/index.html") {
        p = p.trim_end_matches("/index.html").to_string();
    }
    if !p.ends_with('/') {
        p.push('/');
    }
    p
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn now_millis_str() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_default()
}

/// `/prox` and `/prox/*` redirects and gone notice — server.js:10777-10791.
pub fn prox_redirect_or_gone(path: &str, search: &str) -> Option<Response> {
    if path == "/prox" || path.starts_with("/prox/") {
        static GM_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let gm_re = GM_RE.get_or_init(|| {
            regex::Regex::new(r"(?i)^/prox/(?:https?:/?/?|https?/)?(?:html5\.)?gamemonetize(?:\.(?:co|com))?/(.*)$").expect("static regex")
        });
        if let Some(caps) = gm_re.captures(path) {
            let sub = &caps[1];
            return Some(crate::static_files::redirect(
                &format!("/proxy/gamemonetize/{sub}{search}"),
                302,
            ));
        }

        static LUMA_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let luma_re = LUMA_RE.get_or_init(|| {
            regex::Regex::new(r"(?i)^/prox/(?:https?:/?/?|https?/)?lumassets\.pages\.dev/(.*)$")
                .expect("static regex")
        });
        if let Some(caps) = luma_re.captures(path) {
            let sub = &caps[1];
            return Some(crate::static_files::redirect(
                &format!("/proxy/luma/{sub}{search}"),
                302,
            ));
        }

        static CALC_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let calc_re = CALC_RE.get_or_init(|| {
            regex::Regex::new(r"(?i)^/prox/(?:https?:/?/?|https?/)?calculated2\.github\.io/(.*)$")
                .expect("static regex")
        });
        if let Some(caps) = calc_re.captures(path) {
            let sub = &caps[1];
            return Some(crate::static_files::redirect(
                &format!("/proxy/calculated2/{sub}{search}"),
                302,
            ));
        }

        return Some(crate::errors::json_resp(
            410,
            json!({
                "error": "gone",
                "message": "Open proxy access has been removed. Game-specific proxies remain available."
            }),
        ));
    }
    None
}

/// `/proxy/gm-icon/*` caching thumbnail proxy — server.js:10838-10877.
pub async fn gm_icon_proxy(state: &Arc<AppState>, path: &str) -> Option<Response> {
    let sub_path = path.strip_prefix("/proxy/gm-icon/")?;
    static PATH_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let path_re = PATH_RE.get_or_init(|| {
        regex::Regex::new(r"^[a-zA-Z0-9_-]+/[a-zA-Z0-9_.-]+$").expect("static regex")
    });
    if !path_re.is_match(sub_path) {
        return Some(crate::errors::err_resp(
            400,
            Some("invalid image path"),
            None,
        ));
    }
    let icons_dir = state.data_dir().join("game-icons");
    let local_file = icons_dir.join(sub_path.replace('/', "_"));
    if local_file.is_file() {
        if let Ok(bytes) = std::fs::read(&local_file) {
            let ext = local_file
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let mime = match ext.as_str() {
                "png" => "image/png",
                "webp" => "image/webp",
                _ => "image/jpeg",
            };
            return Some(
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", mime)
                    .header("cache-control", "public, max-age=31536000, immutable")
                    .header("access-control-allow-origin", "*")
                    .body(axum::body::Body::from(bytes))
                    .unwrap_or_else(|_| crate::errors::err_resp(500, None, None)),
            );
        }
    }
    let upstream_url = format!("https://img.gamemonetize.com/{sub_path}");
    let client = match reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(_) => {
            return Some(crate::errors::err_resp(
                502,
                Some("image fetch failed"),
                None,
            ))
        }
    };
    match client.get(&upstream_url).send().await {
        Ok(res) if res.status().is_success() => {
            let content_type = res
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("image/jpeg")
                .to_string();
            let bytes = res.bytes().await.unwrap_or_default();
            let _ = std::fs::create_dir_all(&icons_dir);
            let _ = std::fs::write(&local_file, &bytes);
            Some(
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", content_type)
                    .header("cache-control", "public, max-age=31536000, immutable")
                    .header("access-control-allow-origin", "*")
                    .body(axum::body::Body::from(bytes))
                    .unwrap_or_else(|_| crate::errors::err_resp(500, None, None)),
            )
        }
        Ok(_) => Some(crate::errors::err_resp(404, Some("image not found"), None)),
        Err(_) => Some(crate::errors::err_resp(
            502,
            Some("image fetch failed"),
            None,
        )),
    }
}

pub const GAME_PROXY_ORIGINS: &[(&str, &str)] = &[
    ("/proxy/luma/", "https://lumassets.pages.dev"),
    ("/proxy/calculated2/", "https://calculated2.github.io"),
    ("/proxy/gamemonetize/", "https://html5.gamemonetize.co"),
];

/// Fixed-origin game proxy — server.js:10881-10947.
pub async fn game_proxy(
    _state: &Arc<AppState>,
    method: &axum::http::Method,
    path: &str,
    search: &str,
    headers: &HeaderMap,
) -> Option<Response> {
    let mut game_proxy_prefix: Option<&'static str> = None;
    let mut game_proxy_path = String::new();

    for (prefix, _) in GAME_PROXY_ORIGINS {
        if let Some(stripped) = path.strip_prefix(prefix) {
            game_proxy_prefix = Some(prefix);
            game_proxy_path = format!("/{stripped}");
            break;
        }
    }
    if game_proxy_prefix.is_none() {
        if let Some(ref_val) = headers
            .get(axum::http::header::REFERER)
            .and_then(|v| v.to_str().ok())
        {
            if let Ok(u) = url::Url::parse(ref_val) {
                let ref_path = u.path();
                for (prefix, _) in GAME_PROXY_ORIGINS {
                    if ref_path.starts_with(prefix) {
                        game_proxy_prefix = Some(prefix);
                        game_proxy_path = path.to_string();
                        break;
                    }
                }
            }
        }
    }

    let game_proxy_prefix = game_proxy_prefix?;

    if *method != axum::http::Method::GET && *method != axum::http::Method::HEAD {
        return Some(crate::errors::err_resp(
            405,
            Some("method not allowed"),
            None,
        ));
    }

    let target_origin = GAME_PROXY_ORIGINS
        .iter()
        .find(|(prefix, _)| *prefix == game_proxy_prefix)
        .map(|(_, origin)| *origin)
        .unwrap_or("");

    let query_str = if search.is_empty() {
        String::new()
    } else if search.starts_with('?') {
        search.to_string()
    } else {
        format!("?{search}")
    };
    let target_url = format!("{target_origin}{game_proxy_path}{query_str}");

    let req_builder = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(b) => b,
        Err(_) => return Some(game_proxy_error()),
    };

    let req_m = if *method == axum::http::Method::HEAD {
        reqwest::Method::HEAD
    } else {
        reqwest::Method::GET
    };

    let mut req = req_builder.request(req_m, &target_url);
    for (name, val) in headers.iter() {
        let n = name.as_str().to_ascii_lowercase();
        if n == "accept" || n == "accept-language" || n == "range" || n == "user-agent" {
            if let Ok(v) = val.to_str() {
                req = req.header(name.as_str(), v);
            }
        }
    }
    if !headers.contains_key(axum::http::header::USER_AGENT) {
        req = req.header(
            "user-agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36",
        );
    }

    match req.send().await {
        Ok(upstream_res) => {
            let upstream_url_path = upstream_res.url().path().to_string();
            let status = axum::http::StatusCode::from_u16(upstream_res.status().as_u16())
                .unwrap_or(StatusCode::BAD_GATEWAY);
            let mut builder = Response::builder().status(status);

            let mut content_type = String::new();
            for (name, value) in upstream_res.headers().iter() {
                let n = name.as_str().to_ascii_lowercase();
                if n == "set-cookie"
                    || n == "content-security-policy"
                    || n == "content-security-policy-report-only"
                    || n == "x-frame-options"
                    || n == "cross-origin-opener-policy"
                    || n == "cross-origin-embedder-policy"
                    || n == "content-encoding"
                    || n == "content-length"
                    || n == "transfer-encoding"
                    || n == "connection"
                    || n == "keep-alive"
                    || n == "access-control-allow-origin"
                    || n == "cache-control"
                {
                    continue;
                }
                if n == "content-type" {
                    if let Ok(ct) = value.to_str() {
                        content_type = ct.to_ascii_lowercase();
                    }
                }
                builder = builder.header(name.as_str(), value);
            }
            builder = builder
                .header("access-control-allow-origin", "*")
                .header("cache-control", "public, max-age=3600");

            let is_text = content_type.contains("text/html")
                || content_type.contains("text/css")
                || content_type.contains("javascript");

            if !is_text || *method == axum::http::Method::HEAD {
                let body = if *method == axum::http::Method::HEAD {
                    axum::body::Body::empty()
                } else {
                    let bytes = upstream_res.bytes().await.unwrap_or_default();
                    axum::body::Body::from(bytes)
                };
                return Some(
                    builder
                        .body(body)
                        .unwrap_or_else(|_| crate::errors::err_resp(500, None, None)),
                );
            }

            let mut content = upstream_res.text().await.unwrap_or_default();
            content = content.replace(&format!("{target_origin}/"), game_proxy_prefix);

            static ATTR_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
            let attr_re = ATTR_RE.get_or_init(|| {
                regex::Regex::new(r#"(?i)(\b(?:src|href|action|poster)\s*=\s*["'])/([^/])"#)
                    .expect("static regex")
            });
            content = attr_re
                .replace_all(&content, format!("$1{game_proxy_prefix}$2"))
                .to_string();

            static URL_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
            let url_re = URL_RE.get_or_init(|| {
                regex::Regex::new(r#"(?i)url\(\s*(["']?)/([^/])"#).expect("static regex")
            });
            content = url_re
                .replace_all(&content, format!("url($1{game_proxy_prefix}$2"))
                .to_string();

            if content_type.contains("text/html") {
                static BASE_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
                let base_re = BASE_RE
                    .get_or_init(|| regex::Regex::new(r"(?i)<base\b").expect("static regex"));
                if !base_re.is_match(&content) {
                    let mut final_path = if !upstream_url_path.is_empty() {
                        upstream_url_path
                    } else {
                        game_proxy_path
                    };
                    static EXT_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
                    let ext_re = EXT_RE.get_or_init(|| {
                        regex::Regex::new(r"(?i)\.[a-z0-9]+$").expect("static regex")
                    });
                    if !final_path.ends_with('/') && !ext_re.is_match(&final_path) {
                        final_path.push('/');
                    }
                    let directory = if final_path.ends_with('/') {
                        final_path
                    } else {
                        static LAST_SEG_RE: std::sync::OnceLock<regex::Regex> =
                            std::sync::OnceLock::new();
                        let last_seg = LAST_SEG_RE
                            .get_or_init(|| regex::Regex::new(r"[^/]*$").expect("static regex"));
                        last_seg.replace(&final_path, "").to_string()
                    };
                    let dir_clean = directory.trim_start_matches('/');
                    let base_tag = format!(r#"<base href="{game_proxy_prefix}{dir_clean}">"#);
                    static HEAD_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
                    let head_re = HEAD_RE.get_or_init(|| {
                        regex::Regex::new(r"(?i)<head\b[^>]*>").expect("static regex")
                    });
                    content = if let Some(m) = head_re.find(&content) {
                        let idx = m.end();
                        format!("{}{}{}", &content[..idx], base_tag, &content[idx..])
                    } else {
                        format!("{base_tag}{content}")
                    };
                }
            }

            Some(
                builder
                    .body(axum::body::Body::from(content))
                    .unwrap_or_else(|_| crate::errors::err_resp(500, None, None)),
            )
        }
        Err(_) => Some(game_proxy_error()),
    }
}

fn game_proxy_error() -> Response {
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .header("content-type", "text/plain; charset=utf-8")
        .body(axum::body::Body::from("This game could not be reached."))
        .unwrap_or_else(|_| crate::errors::err_resp(502, None, None))
}

/// `/_app/` redirect to pirate voyage if referer matches — server.js:10949-10955.
pub fn pirate_voyage_app_redirect(
    path: &str,
    search: &str,
    headers: &HeaderMap,
) -> Option<Response> {
    if path.starts_with("/_app/") {
        let ref_hdr = headers
            .get(axum::http::header::REFERER)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if ref_hdr.contains("pirate-voyage") || ref_hdr.contains("cinejoy") {
            let dest = format!("/proxy/pirate-voyage{path}{search}");
            return Some(crate::static_files::redirect(&dest, 307));
        }
    }
    None
}

const PIRATE_VOYAGE_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>Premium Required — Pirate Voyage</title>
  <style>
    body { background: #070510; color: #f8fafc; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; display: flex; min-height: 100vh; margin: 0; align-items: center; justify-content: center; text-align: center; padding: 20px; box-sizing: border-box; }
    .card { background: #0f172a; border: 1px solid rgba(168, 85, 247, 0.3); padding: 40px 32px; border-radius: 16px; max-width: 460px; box-shadow: 0 20px 50px rgba(0,0,0,0.6); }
    .icon { font-size: 48px; margin-bottom: 12px; }
    h1 { color: #f8fafc; font-size: 22px; margin: 0 0 12px; font-weight: 800; }
    p { color: #cbd5e1; font-size: 14px; line-height: 1.6; margin: 0 0 24px; }
    .badge { display: inline-block; background: rgba(168, 85, 247, 0.15); color: #c084fc; border: 1px solid rgba(168, 85, 247, 0.3); font-weight: 700; padding: 4px 12px; border-radius: 99px; font-size: 12px; margin-bottom: 16px; }
    .btn { background: linear-gradient(135deg, #a855f7, #6366f1); color: #ffffff; text-decoration: none; padding: 12px 24px; border-radius: 10px; font-weight: 700; display: inline-block; transition: transform 0.15s; }
    .btn:hover { transform: scale(1.04); }
  </style>
</head>
<body>
  <div class="card">
    <div class="icon">🏴‍☠️</div>
    <div class="badge">Premium Feature</div>
    <h1>Pirate Voyage Access Restricted</h1>
    <p>Pirate Voyage is an exclusive feature reserved for <strong>mitch.pro Premium</strong> members. All traffic for Pirate Voyage is proxied through server PIA VPN.</p>
    <a href="/premium.html" target="_top" class="btn">Get Lifetime Premium</a>
  </div>
</body>
</html>"#;

/// Pirate Voyage proxy (premium only) — server.js:10956-11112.
pub async fn pirate_voyage_proxy(
    state: &Arc<AppState>,
    method: &axum::http::Method,
    path: &str,
    search: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Option<Response> {
    if !path.starts_with("/proxy/pirate-voyage") && !path.starts_with("/pirate-voyage") {
        return None;
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
    let email = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid);
    let is_prem = email
        .as_deref()
        .map(|e| mitch_lib::auth::is_premium_email(&state.store, e))
        .unwrap_or(false);

    if !is_prem {
        return Some(
            Response::builder()
                .status(StatusCode::FORBIDDEN)
                .header("content-type", "text/html; charset=utf-8")
                .body(axum::body::Body::from(PIRATE_VOYAGE_HTML))
                .unwrap_or_else(|_| crate::errors::err_resp(403, None, None)),
        );
    }

    let sub_path = if let Some(stripped) = path.strip_prefix("/proxy/pirate-voyage") {
        stripped
    } else if let Some(stripped) = path.strip_prefix("/pirate-voyage") {
        stripped
    } else {
        path
    };
    let sub_path = if sub_path.is_empty() { "/" } else { sub_path };

    let target_url = format!("https://cinejoy.to{sub_path}{search}");

    let mut client_builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .timeout(std::time::Duration::from_secs(20));

    if let Ok(proxy_url) = std::env::var("PIA_PROXY_URL")
        .or_else(|_| std::env::var("PIA_HTTP_PROXY"))
        .or_else(|_| std::env::var("HTTP_PROXY"))
        .or_else(|_| std::env::var("http_proxy"))
    {
        if let Ok(p) = reqwest::Proxy::all(&proxy_url) {
            client_builder = client_builder.proxy(p);
        }
    }

    let client = match client_builder.build() {
        Ok(c) => c,
        Err(_) => {
            return Some(crate::errors::json_resp(
                502,
                json!({ "error": "Bad Gateway", "message": "Failed to proxy Pirate Voyage stream." }),
            ));
        }
    };

    let req_m = match method.as_str() {
        "POST" => reqwest::Method::POST,
        "PUT" => reqwest::Method::PUT,
        "DELETE" => reqwest::Method::DELETE,
        "PATCH" => reqwest::Method::PATCH,
        "HEAD" => reqwest::Method::HEAD,
        _ => reqwest::Method::GET,
    };

    let mut req = client.request(req_m, &target_url);
    for (name, val) in headers.iter() {
        let n = name.as_str().to_ascii_lowercase();
        if n != "host" && n != "cookie" && n != "authorization" && n != "x-mitch-client-ip" {
            if let Ok(v) = val.to_str() {
                req = req.header(name.as_str(), v);
            }
        }
    }
    req = req
        .header("host", "cinejoy.to")
        .header("referer", "https://cinejoy.to/");

    if !method.is_safe() && !body_bytes.is_empty() {
        req = req.body(body_bytes.to_vec());
    }

    match req.send().await {
        Ok(upstream_res) => {
            let status = axum::http::StatusCode::from_u16(upstream_res.status().as_u16())
                .unwrap_or(StatusCode::BAD_GATEWAY);
            let mut builder = Response::builder().status(status);

            let mut content_type = String::new();
            for (name, value) in upstream_res.headers().iter() {
                let n = name.as_str().to_ascii_lowercase();
                if n == "content-security-policy"
                    || n == "x-frame-options"
                    || n == "content-encoding"
                    || n == "content-length"
                    || n == "transfer-encoding"
                    || n == "connection"
                    || n == "keep-alive"
                    || n == "access-control-allow-origin"
                {
                    continue;
                }
                if n == "content-type" {
                    if let Ok(ct) = value.to_str() {
                        content_type = ct.to_ascii_lowercase();
                    }
                }
                builder = builder.header(name.as_str(), value);
            }
            builder = builder.header("access-control-allow-origin", "*");

            if content_type.contains("text/html") {
                let mut html = upstream_res.text().await.unwrap_or_default();
                static INT1_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
                let int1_re = INT1_RE.get_or_init(|| {
                    regex::Regex::new(r#"(?i)\s+integrity="[^"]*""#).expect("static regex")
                });
                html = int1_re.replace_all(&html, "").to_string();

                static INT2_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
                let int2_re = INT2_RE.get_or_init(|| {
                    regex::Regex::new(r#"(?i)\s+integrity='[^']*'"#).expect("static regex")
                });
                html = int2_re.replace_all(&html, "").to_string();

                static CF_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
                let cf_re = CF_RE.get_or_init(|| {
                    regex::Regex::new(
                        r#"(?i)<script[^>]*static\.cloudflareinsights\.com[^>]*>.*?</script>"#,
                    )
                    .expect("static regex")
                });
                html = cf_re.replace_all(&html, "").to_string();

                html = html.replace("navigator.serviceWorker.register", "void");
                html = html.replace("https://cinejoy.to", "/proxy/pirate-voyage");
                html = html.replace(r#"="/_app/"#, r#"="/proxy/pirate-voyage/_app/"#);
                html = html.replace(r#"='/_app/"#, r#"='/proxy/pirate-voyage/_app/"#);
                html = html.replace(r#""/_app/"#, r#""/proxy/pirate-voyage/_app/"#);
                html = html.replace(r#"\'/_app/"#, r#"\'/proxy/pirate-voyage/_app/"#);

                if html.contains("<head>") {
                    html =
                        html.replacen("<head>", "<head><base href=\"/proxy/pirate-voyage/\">", 1);
                }

                Some(
                    builder
                        .body(axum::body::Body::from(html))
                        .unwrap_or_else(|_| crate::errors::err_resp(500, None, None)),
                )
            } else if content_type.contains("javascript") || content_type.contains("json") {
                let mut js = upstream_res.text().await.unwrap_or_default();
                js = js.replace("https://cinejoy.to", "/proxy/pirate-voyage");
                js = js.replace(r#""/_app/"#, r#""/proxy/pirate-voyage/_app/"#);
                js = js.replace(r#"\'/_app/"#, r#"\'/proxy/pirate-voyage/_app/"#);
                js = js.replace("navigator.serviceWorker.register", "void");

                Some(
                    builder
                        .body(axum::body::Body::from(js))
                        .unwrap_or_else(|_| crate::errors::err_resp(500, None, None)),
                )
            } else {
                let bytes = upstream_res.bytes().await.unwrap_or_default();
                Some(
                    builder
                        .body(axum::body::Body::from(bytes))
                        .unwrap_or_else(|_| crate::errors::err_resp(500, None, None)),
                )
            }
        }
        Err(_) => Some(crate::errors::json_resp(
            502,
            json!({ "error": "Bad Gateway", "message": "Failed to proxy Pirate Voyage stream." }),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prox_redirect_or_gone() {
        // Gamemonetize redirect
        let res = prox_redirect_or_gone("/prox/https://html5.gamemonetize.co/game123/", "?test=1")
            .unwrap();
        assert_eq!(res.status(), StatusCode::FOUND);
        let loc = res.headers().get("location").unwrap().to_str().unwrap();
        assert_eq!(loc, "/proxy/gamemonetize/game123/?test=1");

        // Luma redirect
        let res = prox_redirect_or_gone("/prox/lumassets.pages.dev/assets/game.js", "").unwrap();
        assert_eq!(res.status(), StatusCode::FOUND);
        let loc = res.headers().get("location").unwrap().to_str().unwrap();
        assert_eq!(loc, "/proxy/luma/assets/game.js");

        // Calculated2 redirect
        let res = prox_redirect_or_gone("/prox/calculated2.github.io/calc/", "").unwrap();
        assert_eq!(res.status(), StatusCode::FOUND);
        let loc = res.headers().get("location").unwrap().to_str().unwrap();
        assert_eq!(loc, "/proxy/calculated2/calc/");

        // Arbitrary URL is 410 Gone
        let res = prox_redirect_or_gone("/prox/example.com", "").unwrap();
        assert_eq!(res.status(), StatusCode::GONE);
    }

    #[test]
    fn test_pirate_voyage_app_redirect() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::REFERER,
            axum::http::HeaderValue::from_static("https://mitch.pro/proxy/pirate-voyage/watch"),
        );
        let res =
            pirate_voyage_app_redirect("/_app/immutable/entry/start.js", "?v=1", &headers).unwrap();
        assert_eq!(res.status(), StatusCode::TEMPORARY_REDIRECT);
        let loc = res.headers().get("location").unwrap().to_str().unwrap();
        assert_eq!(
            loc,
            "/proxy/pirate-voyage/_app/immutable/entry/start.js?v=1"
        );

        // Non-pirate referer does not redirect
        let mut norm_headers = HeaderMap::new();
        norm_headers.insert(
            axum::http::header::REFERER,
            axum::http::HeaderValue::from_static("https://mitch.pro/games"),
        );
        assert!(pirate_voyage_app_redirect("/_app/chunk.js", "", &norm_headers).is_none());
    }

    #[tokio::test]
    async fn test_luma_slope_fetch() {
        let dir = std::env::temp_dir().join(format!("mitch-proxy-test-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(mitch_lib::data::DataStore::open(&dir, &dir).unwrap());
        let cfg = crate::hosts::SiteConfig::load();
        let state = Arc::new(AppState::new(cfg, store));
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", axum::http::HeaderValue::from_static("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/122.0.0.0"));
        let resp = game_proxy(&state, &axum::http::Method::GET, "/proxy/luma/Slope/index.html", "", &headers).await;
        assert!(resp.is_some());
        let r = resp.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert!(!r.headers().contains_key("transfer-encoding"), "transfer-encoding must be stripped");
        assert!(!r.headers().contains_key("connection"), "connection must be stripped");
    }
}
