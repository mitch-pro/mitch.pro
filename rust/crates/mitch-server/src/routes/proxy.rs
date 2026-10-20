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
    headers: &HeaderMap,
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
    let revoked = mitch_lib::revoked::revoked_id_get(&state.store, hash);
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

    let html = if crate::presence::authenticated_email(state, headers).is_some() {
        crate::inject::inject_presence(&html, Some(&format!("/{}", rel.trim_start_matches('/'))))
    } else {
        html
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
                "{}<script src=\"/broadcast.js?v=11\" defer></script>{}",
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
        let mut user = mitch_lib::user_stats::user_stats_get(&state.store, &norm);
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
        mitch_lib::user_stats::user_stats_set(&state.store, &norm, &user);
    }

    // Presence leg (server.js:15617-15639): the playingGame ladder keyed off
    // the lowercased page, then touchUserPresence.
    if let Some(ref em) = email {
        let page_lower = page.to_lowercase();
        let playing_game = if page_lower.contains("/games/chess/") {
            "Chess".to_string()
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
    ("/proxy/rivestream/", "https://www.rivestream.app"),
];

fn fixed_proxy_target(path: &str, headers: &HeaderMap) -> Option<(&'static str, String)> {
    for (prefix, _) in GAME_PROXY_ORIGINS {
        if let Some(stripped) = path.strip_prefix(prefix) {
            return Some((prefix, format!("/{stripped}")));
        }
    }
    let referer = headers.get(axum::http::header::REFERER)?.to_str().ok()?;
    let url = url::Url::parse(referer).ok()?;
    GAME_PROXY_ORIGINS.iter().find_map(|(prefix, _)| {
        // Rivestream rewrites its own paths; it must not intercept
        // navigation back to Home or requests to Mitch APIs.
        (*prefix != "/proxy/rivestream/" && url.path().starts_with(prefix))
            .then(|| (*prefix, path.to_string()))
    })
}

/// Fixed-origin game and movie proxies.
pub async fn game_proxy(
    state: &Arc<AppState>,
    method: &axum::http::Method,
    path: &str,
    search: &str,
    headers: &HeaderMap,
) -> Option<Response> {
    if path == "/proxy/rivestream" {
        return Some(crate::static_files::redirect(
            &format!("/proxy/rivestream/{search}"),
            302,
        ));
    }
    let (game_proxy_prefix, game_proxy_path) = fixed_proxy_target(path, headers)?;

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

    let is_rivestream = game_proxy_prefix == "/proxy/rivestream/";
    let redirect_policy = if is_rivestream {
        reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() < 10
                && attempt.url().scheme() == "https"
                && attempt.url().host_str() == Some("www.rivestream.app")
            {
                attempt.follow()
            } else {
                attempt.stop()
            }
        })
    } else {
        reqwest::redirect::Policy::limited(10)
    };
    let req_builder = match reqwest::Client::builder()
        .redirect(redirect_policy)
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(b) => b,
        Err(_) => return Some(game_proxy_error(is_rivestream)),
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
    if is_rivestream {
        req = req.header("referer", "https://www.rivestream.app/");
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
                if is_rivestream && ["speculation-rules", "report-to", "nel"].contains(&n.as_str())
                {
                    continue;
                }
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
                .header(
                    "cache-control",
                    if content_type.contains("text/html") {
                        "private, no-store"
                    } else {
                        "public, max-age=3600"
                    },
                )
                .header("vary", "Cookie");

            let is_text = content_type.contains("text/html")
                || content_type.contains("text/css")
                || content_type.contains("javascript");

            if is_rivestream && content_type.contains("text/html") {
                builder = builder.header(
                    "content-security-policy",
                    "default-src 'self' https: blob: data:; script-src 'self' 'unsafe-inline' 'unsafe-eval' blob:; style-src 'self' 'unsafe-inline' https:; object-src 'none'; worker-src 'none'; frame-ancestors 'self'",
                );
            }

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

            if is_rivestream {
                content = rewrite_rivestream(&content, content_type.contains("text/html"));
            }

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

            if content_type.contains("text/html")
                && crate::presence::authenticated_email(state, headers).is_some()
            {
                content = crate::inject::inject_presence(&content, None);
            }

            Some(
                builder
                    .body(axum::body::Body::from(content))
                    .unwrap_or_else(|_| crate::errors::err_resp(500, None, None)),
            )
        }
        Err(_) => Some(game_proxy_error(is_rivestream)),
    }
}

fn rewrite_rivestream(content: &str, is_html: bool) -> String {
    const PREFIX: &str = "/proxy/rivestream";
    if is_html {
        static DATA_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let data_re = DATA_RE.get_or_init(|| {
            regex::Regex::new(
                r#"(?s)(<script\b[^>]*\bid=["']__NEXT_DATA__["'][^>]*>)(.*?)(</script\s*>)"#,
            )
            .expect("static regex")
        });
        let html = data_re.replace(content, |caps: &regex::Captures<'_>| {
            let Ok(mut data) = serde_json::from_str::<Value>(&caps[2]) else {
                return caps[0].to_string();
            };
            let Some(object) = data.as_object_mut() else {
                return caps[0].to_string();
            };
            object.insert("assetPrefix".to_string(), json!(PREFIX));
            format!(
                "{}{}{}",
                &caps[1],
                data.to_string().replace('<', "\\u003c"),
                &caps[3]
            )
        });
        let runtime = include_str!("rivestream.js");
        return html.replacen(
            "<head>",
            &format!(r#"<head><base href="{PREFIX}/"><script>{runtime}</script>"#),
            1,
        );
    }

    // Next's exported build assumes it owns /. Give its router the proxy base
    // path while keeping internal page names (such as /watch) unchanged.
    let mut js = content.replace("this.basePath=\"\"", &format!("this.basePath=\"{PREFIX}\""));
    static ADD_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let add_re = ADD_RE.get_or_init(|| {
        regex::Regex::new(r#"(\b(?:addPathPrefix|pathHasPrefix)\)\([\w$]+,\s*)"""#)
            .expect("static regex")
    });
    js = add_re
        .replace_all(&js, |caps: &regex::Captures<'_>| {
            format!(r#"{}"{PREFIX}""#, &caps[1])
        })
        .into_owned();
    static REMOVE_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let remove_re = REMOVE_RE.get_or_init(|| {
        regex::Regex::new(r#"function ([\w$]+)\(([\w$]+)\)\{return [\w$]+\}(Object\.defineProperty\([^;]+?"removeBasePath")"#)
            .expect("static regex")
    });
    remove_re
        .replace_all(&js, |caps: &regex::Captures<'_>| {
            let name = &caps[1];
            let arg = &caps[2];
            let tail = &caps[3];
            format!(r#"function {name}({arg}){{return {arg}==="{PREFIX}"?"/":{arg}.startsWith("{PREFIX}/")?{arg}.slice({}):{arg}}}{tail}"#, PREFIX.len())
        })
        .into_owned()
}

fn game_proxy_error(is_rivestream: bool) -> Response {
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .header("content-type", "text/plain; charset=utf-8")
        .body(axum::body::Body::from(if is_rivestream {
            "This movie page could not be reached."
        } else {
            "This game could not be reached."
        }))
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
                builder = builder
                    .header("cache-control", "private, no-store")
                    .header("vary", "Cookie");
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

                if crate::presence::authenticated_email(state, headers).is_some() {
                    html = crate::inject::inject_presence(&html, None);
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
    fn rivestream_routing_stays_scoped() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "referer",
            "https://mitchdog.com/proxy/rivestream/watch?type=movie&id=969681"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            fixed_proxy_target("/proxy/rivestream/watch", &headers),
            Some(("/proxy/rivestream/", "/watch".to_string()))
        );
        for path in ["/", "/api/me", "/enroll/", "/proxy/rivestream.evil/watch"] {
            assert_eq!(fixed_proxy_target(path, &headers), None);
        }
        headers.insert(
            "referer",
            "https://mitchdog.com/proxy/luma/Slope/index.html"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            fixed_proxy_target("/assets/game.js", &headers),
            Some(("/proxy/luma/", "/assets/game.js".to_string()))
        );
    }

    #[test]
    fn rivestream_keeps_next_page_names_and_sets_asset_base() {
        let html = r#"<html><head></head><body><script id="__NEXT_DATA__" type="application/json">{"page":"/watch","query":{"type":"movie","id":"969681"}}</script></body></html>"#;
        let rewritten = rewrite_rivestream(html, true);
        assert!(rewritten.contains(r#"<base href="/proxy/rivestream/">"#));
        let data = rewritten
            .split(r#"type="application/json">"#)
            .nth(1)
            .unwrap()
            .split("</script>")
            .next()
            .unwrap();
        let data: Value = serde_json::from_str(data).unwrap();
        assert_eq!(data["assetPrefix"], "/proxy/rivestream");
        assert_eq!(data["page"], "/watch");
        assert_eq!(data["query"]["id"], "969681");
        assert!(rewritten.contains("window.fetch ="));
    }

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
        let dir = std::env::temp_dir().join(format!(
            "mitch-proxy-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(mitch_lib::data::DataStore::open(&dir, &dir).unwrap());
        let cfg = crate::hosts::SiteConfig::load();
        let state = Arc::new(AppState::new(cfg, store));
        let mut headers = HeaderMap::new();
        headers.insert(
            "user-agent",
            axum::http::HeaderValue::from_static(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/122.0.0.0",
            ),
        );
        let resp = game_proxy(
            &state,
            &axum::http::Method::GET,
            "/proxy/luma/Slope/index.html",
            "",
            &headers,
        )
        .await;
        assert!(resp.is_some());
        let r = resp.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert!(
            !r.headers().contains_key("transfer-encoding"),
            "transfer-encoding must be stripped"
        );
        assert!(
            !r.headers().contains_key("connection"),
            "connection must be stripped"
        );
    }
}
