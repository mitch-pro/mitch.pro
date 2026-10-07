//! The request flow — port of `handleRequest`'s GET path (server.js), in the
//! exact order: bell/blooket redirects → CSRF → /api/bg → rate limit →
//! /swift → ban/maintenance gates → page block (manifests, site hubs, site
//! page roots, redirects, HTML gates, injection pipeline, public assets) →
//! serveStatic. Non-GET falls to 405.
//!
//! Session-dependent pieces (`checkPasswordCookie`, bans, rate limits, SSO
//! bridge tokens) are stubbed unauthenticated until Step 6 — parity tests run
//! without cookies, so the unauthenticated paths match bun exactly.
#![allow(clippy::expect_used)] // infallible static responses + static regexes

use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::Response;
use std::sync::Arc;

use crate::errors::{err_resp, json_resp};
use crate::hosts::{is_pickle_host, is_rjuhsd_host, request_host, sso_back_allowed, SiteConfig};
use crate::inject::{inject_readability, inject_shared_head, recaptcha_loader_str};
use crate::state::AppState;
use crate::static_files::{pickle_asset_response, redirect, safe_webroot_path, serve_static};

/// `HTML_OPEN` — pages public on every host.
pub const HTML_OPEN: &[&str] = &[
    "/roblox",
    "/enroll",
    "/claim",
    "/password",
    "/appeal",
    "/unsubscribe",
    "/admin",
    "/faq",
    "/faq/index",
    "/use-agreement",
    "/use-agreement/index",
    "/privacy",
    "/privacy/index",
    "/bell",
    "/bell/index",
    "/preferences",
    "/preferences/index",
    "/swift",
    "/swift/index",
    "/larp",
    "/larp/index",
    "/larp/rezero",
    "/larp/rezero/index",
    "/games",
    "/games/index",
    "/game-portal",
    "/game-portal/index",
    "/msn-games",
    "/msn-games/index",
    "/matrix",
    "/matrix/index",
    "/tor",
    "/tor/index",
    "/index-sales",
    "/index-sales/index",
    "/rjuhsd",
    "/rjuhsd/index",
    "/sexypickleclub",
    "/sexypickleclub/index",
];

/// `PROTECTED_FILES`.
pub const PROTECTED_FILES: &[&str] = &["senpai-cafe.webp", "adrian-lopez.webp"];

/// `PUBLIC_ASSETS` allowlist.
pub const PUBLIC_ASSETS: &[&str] = &[
    "/onboarding.css",
    "/onboarding.js",
    "/onboarding-theme.js",
    "/media/site-tour-v1.mp4",
    "/media/site-tour-poster-v1.webp",
    "/guest-preview.js",
    "/home-friends.js",
    "/home.css",
    "/home-dayboard.js",
    "/auth.js",
    "/sync.js",
    "/auth-non-enrolled.js",
    "/assistant.js",
    "/broadcast.js",
    "/cookie-consent.js",
    "/api.js",
    "/app-shell.js",
    "/site-theme.js",
    "/mitch-coins.js",
    "/mitch-coins.css",
    "/mitchcoin.png",
    "/mitchcoin.webp",
    "/app.css",
    "/auth-liquid.css",
    "/encrypt-galaxy.css",
    "/vendor/simplewebauthn.browser.min.js",
    "/rjuhsd-assets/app.js",
    "/rjuhsd-assets/styles.css",
    "/rjuhsd-assets/reference-theme.css",
    "/rjuhsd-assets/redesign.css",
    "/preferences/preferences-school.css",
    "/preferences/preferences-galaxy.css",
    "/rjuhsd-assets/woodcreek.png",
    "/rjuhsd-assets/calendar.js",
    "/rjuhsd-assets/woodcreek-logo.png",
    "/rjuhsd-assets/roseville-logo.png",
    "/rjuhsd-assets/granitebay-logo.png",
    "/rjuhsd-assets/antelope-logo.png",
    "/rjuhsd-assets/westpark-logo.png",
    "/rjuhsd-assets/oakmont-logo.png",
    "/rjuhsd-assets/pathways-logo.png",
    "/rjuhsd-assets/rosevilleadult-logo.png",
    "/rjuhsd-assets/district-logo.png",
    "/rjuhsd-assets/icon-192.png",
    "/rjuhsd-assets/icon-512.png",
    "/rjuhsd-assets/maskable-512.png",
    "/rjuhsd-assets/apple-touch-icon.png",
    "/rjuhsd-assets/favicon-32.png",
    "/liquid-glass.js",
    "/jsmpeg.min.js",
    "/open.css",
    "/theme.js",
    "/sw.js",
    "/popup.js",
    "/pwa-install.js",
    "/games/chess-bot/chessboard.min.js",
    "/games/chess-bot/chessboard.min.css",
    "/bell/schedule.js",
    "/favicon.ico",
    "/manifest.json",
    "/apple-touch-icon.png",
    "/icon-192.png",
    "/icon-512.png",
    "/home-burning-cherry.webp",
    "/robots.txt",
    "/sitemap.xml",
    "/verify-open.json",
    "/casino/casino-refresh.css",
];

/// Query-string map (URLSearchParams-like, first value wins).
pub fn query(search: &str) -> std::collections::HashMap<String, String> {
    use form_urlencoded::parse;
    parse(search.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

/// `encodeURIComponent` — leaves A-Za-z0-9 and `-_.!~*'()` unescaped.
pub fn encode_uri_component(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for b in v.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(b as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Port of `prepareRjuhsdHtml` from server.js:25812-25940.
pub fn prepare_rjuhsd_html(
    raw_html: &str,
    req_host: Option<&str>,
    is_rjuhsd: bool,
    search: &str,
    cfg: &SiteConfig,
) -> String {
    let mut html = inject_shared_head(raw_html);
    let mut primary_origin = "https://mitch.pro".to_string();
    let mut primary_host = "mitch.pro".to_string();
    if let Ok(pu) = url::Url::parse(&cfg.primary) {
        primary_origin = pu.origin().ascii_serialization();
        primary_host = pu.host_str().unwrap_or("mitch.pro").to_string();
    }

    let mut alt_origin = String::new();
    let mut alt_host = String::new();
    if !cfg.alternate.is_empty() {
        if let Ok(au) = url::Url::parse(&cfg.alternate) {
            alt_origin = au.origin().ascii_serialization();
            alt_host = au.host_str().unwrap_or("").to_string();
        }
    }

    let req_host_str = req_host.filter(|h| !h.is_empty()).unwrap_or(if is_rjuhsd {
        "rjuhsd.school"
    } else {
        &primary_host
    });
    let is_preview = !is_rjuhsd;
    let back_path = if is_preview { "/rjuhsd/" } else { "/" };
    let back_url = format!("https://{req_host_str}{back_path}");

    let effective_origin = if !alt_origin.is_empty() {
        &alt_origin
    } else {
        &primary_origin
    };

    if !alt_host.is_empty() && alt_host != primary_host {
        // 1. Injected alternate domain by default into sign-in buttons
        let signin_pattern = format!("Sign in with {}", primary_host);
        let signin_replacement = format!("Sign in with {}", alt_host);
        html = html.replace(&signin_pattern, &signin_replacement);

        // Update href of js-signin-link / SSO bridge
        let alt_bridge_url = format!(
            "{effective_origin}/api/sso/bridge?back={}",
            encode_uri_component(&back_url)
        );
        html = html.replace(
            "href=\"/api/sso/bridge?back=%2F\"",
            &format!("href=\"{alt_bridge_url}\""),
        );

        if html.contains("</body>") {
            html = html.replace("</body>", "\n</body>");
        }
    }

    // SEO & School personalization for server-rendered HTML. The bare URL
    // (no ?school=) renders Woodcreek's schedule by default — it always
    // has, per the raw HTML's own hero/H1 content — so it needs to be
    // treated as "woodcreek" here too, or the title/description Google
    // actually indexes for that URL stays the generic multi-school copy
    // while the page itself is Woodcreek-specific. That mismatch is bad
    // for CTR and relevance on exactly the query this is meant to rank
    // for ("woodcreek bell schedule").
    let req_school = query(search)
        .into_iter()
        .find(|(k, _)| k == "school")
        .map(|(_, v)| v.to_lowercase().trim().to_string())
        .unwrap_or_default();
    let effective_school = if req_school.is_empty() {
        "woodcreek"
    } else {
        req_school.as_str()
    };
    let is_default_school_view = req_school.is_empty() || req_school == "woodcreek";

    let school_meta = match effective_school {
        "woodcreek" => Some(("Woodcreek High School", "Woodcreek", "Timberwolves")),
        "roseville" => Some(("Roseville High School", "Roseville", "Tigers")),
        "granitebay" => Some(("Granite Bay High School", "Granite Bay", "Grizzlies")),
        "antelope" => Some(("Antelope High School", "Antelope", "Titans")),
        "westpark" => Some(("West Park High School", "West Park", "Panthers")),
        "oakmont" => Some(("Oakmont High School", "Oakmont", "Vikings")),
        _ => None,
    };

    if let Some((name, short, mascot)) = school_meta {
        let school_title = format!("{name} Bell Schedule | RJUHSD Hub");
        let school_desc = format!("Live {name} bell schedule, period countdowns, daily times, and calendar for the {mascot} in Roseville Joint Union High School District (RJUHSD).");
        // Woodcreek's schedule lives at the bare URL too, so both must
        // canonicalize to the same clean URL instead of splitting ranking
        // signal between "/" and "/?school=woodcreek".
        let school_canonical = if is_default_school_view {
            format!("https://{req_host_str}{back_path}")
        } else {
            format!("https://{req_host_str}{back_path}?school={effective_school}")
        };

        static TITLE_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let title_re = TITLE_RE
            .get_or_init(|| regex::Regex::new(r"(?i)<title>.*?</title>").expect("static regex"));
        html = title_re
            .replace(&html, format!("<title>{school_title}</title>"))
            .to_string();

        static META_DESC_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let meta_desc_re = META_DESC_RE.get_or_init(|| {
            regex::Regex::new(r#"(?i)(<meta\s+name="description"\s+content=")[^"]*(")"#)
                .expect("static regex")
        });
        html = meta_desc_re
            .replace(&html, format!("${{1}}{school_desc}${{2}}"))
            .to_string();

        static LINK_CANONICAL_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let link_canonical_re = LINK_CANONICAL_RE.get_or_init(|| {
            regex::Regex::new(r#"(?i)(<link\s+rel="canonical"\s+href=")[^"]*(")"#)
                .expect("static regex")
        });
        html = link_canonical_re
            .replace(&html, format!("${{1}}{school_canonical}${{2}}"))
            .to_string();

        static OG_TITLE_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let og_title_re = OG_TITLE_RE.get_or_init(|| {
            regex::Regex::new(r#"(?i)(<meta\s+property="og:title"\s+content=")[^"]*(")"#)
                .expect("static regex")
        });
        html = og_title_re
            .replace(&html, format!("${{1}}{school_title}${{2}}"))
            .to_string();

        static OG_DESC_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let og_desc_re = OG_DESC_RE.get_or_init(|| {
            regex::Regex::new(r#"(?i)(<meta\s+property="og:description"\s+content=")[^"]*(")"#)
                .expect("static regex")
        });
        html = og_desc_re
            .replace(&html, format!("${{1}}{school_desc}${{2}}"))
            .to_string();

        static OG_URL_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let og_url_re = OG_URL_RE.get_or_init(|| {
            regex::Regex::new(r#"(?i)(<meta\s+property="og:url"\s+content=")[^"]*(")"#)
                .expect("static regex")
        });
        html = og_url_re
            .replace(&html, format!("${{1}}{school_canonical}${{2}}"))
            .to_string();

        static TW_TITLE_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let tw_title_re = TW_TITLE_RE.get_or_init(|| {
            regex::Regex::new(r#"(?i)(<meta\s+name="twitter:title"\s+content=")[^"]*(")"#)
                .expect("static regex")
        });
        html = tw_title_re
            .replace(&html, format!("${{1}}{school_title}${{2}}"))
            .to_string();

        static TW_DESC_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let tw_desc_re = TW_DESC_RE.get_or_init(|| {
            regex::Regex::new(r#"(?i)(<meta\s+name="twitter:description"\s+content=")[^"]*(")"#)
                .expect("static regex")
        });
        html = tw_desc_re
            .replace(&html, format!("${{1}}{school_desc}${{2}}"))
            .to_string();

        static HERO_OVERLINE_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let hero_overline_re = HERO_OVERLINE_RE.get_or_init(|| {
            regex::Regex::new(r#"<p class="hero-overline" id="hero-overline">.*?</p>"#)
                .expect("static regex")
        });
        html = hero_overline_re
            .replace(
                &html,
                format!(
                    r#"<p class="hero-overline" id="hero-overline">{}</p>"#,
                    name.to_uppercase()
                ),
            )
            .to_string();

        static SCHOOL_HEADING_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let school_heading_re = SCHOOL_HEADING_RE.get_or_init(|| {
            regex::Regex::new(r#"<span id="school-heading">.*?</span>"#).expect("static regex")
        });
        html = school_heading_re
            .replace(
                &html,
                format!(r#"<span id="school-heading">{short}</span>"#),
            )
            .to_string();

        static CURRENT_RANGE_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let current_range_re = CURRENT_RANGE_RE.get_or_init(|| {
            regex::Regex::new(r#"<span class="period-range" id="current-range">.*?</span>"#)
                .expect("static regex")
        });
        html = current_range_re
            .replace(
                &html,
                format!(r#"<span class="period-range" id="current-range">{name}</span>"#),
            )
            .to_string();
    }

    html
}

fn manifest_response(manifest: serde_json::Value) -> Response {
    Response::builder()
        .status(axum::http::StatusCode::OK)
        .header(
            axum::http::header::CONTENT_TYPE,
            "application/manifest+json; charset=utf-8",
        )
        .header(axum::http::header::CACHE_CONTROL, "public, max-age=3600")
        .body(axum::body::Body::from(
            serde_json::to_string_pretty(&manifest).unwrap_or_default(),
        ))
        .expect("static response")
}

/// lib/site_redirects.js bellScheduleRedirect — GET/HEAD only.
fn bell_schedule_redirect(path: &str, method: &Method, search: &str) -> Option<Response> {
    if method != Method::GET && method != Method::HEAD {
        return None;
    }
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"(?i)^/(?:rjuhsd/)?bell(?:\.html?|/(?:index(?:\.html?)?/?)?)?$")
            .expect("static regex")
    });
    if re.is_match(path) {
        let q = if search.is_empty() {
            String::new()
        } else {
            format!("?{search}")
        };
        return Some(redirect(&format!("https://rjuhsd.school/{q}"), 302));
    }
    None
}

/// lib/site_redirects.js blooketBotRedirect — GET/HEAD only.
fn blooket_bot_redirect(path: &str, method: &Method, search: &str) -> Option<Response> {
    if method != Method::GET && method != Method::HEAD {
        return None;
    }
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"(?i)^/blooket-bot(?:\.html?|/(?:index(?:\.html?)?/?)?)?$")
            .expect("static regex")
    });
    if re.is_match(path) {
        let q = if search.is_empty() {
            String::new()
        } else {
            format!("?{search}")
        };
        return Some(redirect(&format!("https://woodcreek.site/{q}"), 302));
    }
    None
}

/// `getRealIp(req)` — precedence: public socket peer -> CF-Connecting-IP ->
/// True-Client-IP -> X-Mitch-Client-IP -> first public XFF -> X-Real-IP ->
/// X-Client-IP -> RFC 7239 Forwarded -> private fallbacks -> 127.0.0.1.
/// `peer_ip` comes from the socket (None when behind a proxy).
pub fn get_real_ip(headers: &HeaderMap, peer_ip: Option<&str>) -> String {
    fn valid_ip(v: &str) -> bool {
        v.parse::<std::net::IpAddr>().is_ok()
    }
    fn is_private(v: &str) -> bool {
        v.parse::<std::net::IpAddr>()
            .map(|ip| match ip {
                std::net::IpAddr::V4(v4) => v4.is_private(),
                std::net::IpAddr::V6(v6) => v6.is_loopback(),
            })
            .unwrap_or(false)
    }
    let header_ip = |name: &str| -> Option<String> {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty() && valid_ip(v) && !is_private(v))
    };

    // 0. Public socket peer.
    if let Some(peer) = peer_ip {
        if valid_ip(peer) && !is_private(peer) {
            return peer.to_string();
        }
    }
    for name in ["CF-Connecting-IP", "True-Client-IP", "X-Mitch-Client-IP"] {
        if let Some(ip) = header_ip(name) {
            return ip;
        }
    }
    // XFF: first public entry.
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        for part in xff.split(',') {
            let p = part.trim();
            if !p.is_empty() && valid_ip(p) && !is_private(p) {
                return p.to_string();
            }
        }
    }
    for name in ["X-Real-IP", "X-Client-IP"] {
        if let Some(ip) = header_ip(name) {
            return ip;
        }
    }
    // RFC 7239 Forwarded: for=
    if let Some(fwd) = headers.get("forwarded").and_then(|v| v.to_str().ok()) {
        static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let re = RE.get_or_init(|| {
            regex::Regex::new(r#"for=(?:"?\[?)([a-zA-Z0-9:.]+)(?:\]?"?)"#).expect("static regex")
        });
        if let Some(m) = re.captures(fwd) {
            let ip = m[1].to_string();
            if valid_ip(&ip) && !is_private(&ip) {
                return ip;
            }
        }
    }
    // Private/bridge fallback chain.
    const BRIDGE: &str = r"^172\.(1[6-9]|2[0-9]|3[0-1])\.";
    static BRIDGE_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let bridge = BRIDGE_RE.get_or_init(|| regex::Regex::new(BRIDGE).expect("static regex"));
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
    let is_usable = |v: &str| -> bool {
        valid_ip(v) && !v.eq_ignore_ascii_case("127.0.0.1") && v != "::1" && !bridge.is_match(v)
    };
    if !raw_mitch.is_empty() && is_usable(raw_mitch) {
        return raw_mitch.to_string();
    }
    if !raw_real.is_empty() && is_usable(raw_real) {
        return raw_real.to_string();
    }
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        for part in xff.split(',') {
            let p = part.trim();
            if !p.is_empty() && is_usable(p) {
                return p.to_string();
            }
        }
    }
    if let Some(peer) = peer_ip {
        if is_usable(peer) {
            return peer.to_string();
        }
    }
    if !raw_mitch.is_empty() && valid_ip(raw_mitch) {
        return raw_mitch.to_string();
    }
    if !raw_real.is_empty() && valid_ip(raw_real) {
        return raw_real.to_string();
    }
    "127.0.0.1".to_string()
}

/// The full ported flow. `authenticated` is the session stub (Step 6 wires
/// the real `checkPasswordCookie`).
pub async fn handle(
    state: Arc<AppState>,
    method: Method,
    uri: &axum::http::Uri,
    headers: &HeaderMap,
    body_bytes: &[u8],
    ws_upgrade: Option<axum::extract::ws::WebSocketUpgrade>,
) -> Response {
    let path = uri.path().to_string();
    let search = uri.query().unwrap_or("").to_string();

    // 0. mitch.pro canonical redirect (server.js:9285-9295).
    let incoming_host = request_host(headers)
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let is_compatibility_endpoint = path.starts_with("/api/")
        || path.starts_with("/_matrix/")
        || path.starts_with("/.well-known/matrix/")
        // LiveKit calls (token endpoints, the SDK's /rtc/.../validate
        // reachability check, the /rtc WS signaling proxy) are a backend API
        // surface, not a page to canonicalize — the call widget is given
        // "https://mitch.pro/livekit" as its focus URL regardless of which
        // mitch.pro-family domain it's embedded in, so redirecting those
        // requests to mitchdog.com just turns them into a cross-origin
        // redirect with no CORS headers, which the browser blocks outright.
        || path.starts_with("/livekit/")
        || path == "/ws"
        || path == "/health"
        || path == "/healthz";
    if incoming_host == "mitch.pro"
        && (method == Method::GET || method == Method::HEAD)
        && !is_compatibility_endpoint
    {
        let query_part = if search.is_empty() {
            String::new()
        } else {
            format!("?{search}")
        };
        let canonical_destination = format!("https://mitchdog.com{path}{query_part}");
        if state.check_password_cookie(headers, None) {
            let encoded = encode_uri_component(&canonical_destination);
            return redirect(
                &format!("https://mitch.pro/api/sso/bridge?back={encoded}"),
                302,
            );
        }
        return redirect(&canonical_destination, 308);
    }

    // 1. Bell/blooket redirects.
    if let Some(resp) = bell_schedule_redirect(&path, &method, &search) {
        return resp;
    }
    if let Some(resp) = blooket_bot_redirect(&path, &method, &search) {
        return resp;
    }

    // 2. CSRF for mutating /api/ calls (GET is skipped inside csrf_check).
    if let Some(resp) = csrf_check(state.as_ref(), headers, &path, &method) {
        return resp;
    }

    // 3. /swift → 301.
    if path == "/swift" {
        return redirect(&format!("/swift/{search}"), 301);
    }

    let node_env_test = std::env::var("NODE_ENV").unwrap_or_default() == "test";
    let body: serde_json::Value =
        serde_json::from_slice(body_bytes).unwrap_or(serde_json::json!({}));
    // 3b. Rate limiting — every /api/ path (server.js:9130-9134: the global
    // gate calls checkRateLimit unconditionally for /api/*, including the
    // chess-vs block; the RATE_LIMITS entries in the chess-vs block itself
    // are dead config but the global gate still applies). The parity
    // harness hits unlisted paths which get the __default__ [100, 60] per
    // ip+anon bucket; sequential runs stay under it. Includes the anti-bot
    // timing check (server.js:5992-5999) with the same exempt suffixes
    // (/state, /inbox, /heartbeat, /groups, /dm/send, /canvas/,
    // /blooket-bot/status).
    if path.starts_with("/api/") && method != Method::OPTIONS && !node_env_test {
        let ip = get_real_ip(headers, None);
        // getIdKey (server.js:5915-5920): studentId || id from the
        // session-restoring cookie map, 'id:<val>' when the HMAC validates,
        // else the shared 'anon' bucket.
        let cookies = crate::routes::me::cookies_of(&state, headers);
        let val = cookies
            .get("studentId")
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| cookies.get("id").unwrap_or(""));
        let id_key = if mitch_lib::auth::valid_id(val, &state.id_secret) {
            format!("id:{val}")
        } else {
            "anon".to_string()
        };
        if let Some((code, message)) = state.rate_limit_check(&ip, &id_key, &path) {
            return json_resp(code, serde_json::json!({ "error": message }));
        }
    }

    // 3b1. LiveKit HTTP surface — server.js:9637-9698. The JS runs the
    // /livekit* OPTIONS preflight and the token endpoints (9636) BEFORE the
    // maintenance (10500), ban (10525) and password (10557) gates, so this
    // must dispatch before the gates below: anonymous token generation
    // succeeds (live-verified divergence bun 200 / rust 302 otherwise).
    if path.starts_with("/livekit") {
        if let Some(resp) = crate::routes::livekit::handle_http(
            &state, &method, &path, headers, &search, body_bytes,
        ) {
            return resp;
        }
        // Anything else under /livekit that isn't a WebSocket upgrade (that
        // case is handled later, post-gate, by handle_rtc_upgrade) — e.g.
        // the LiveKit client SDK's /rtc/.../validate reachability check.
        // Proxied here, pre-gate and with CORS headers, so it works the
        // same for anonymous visitors and across the mitch.pro-family
        // domains that all share this one LiveKit instance.
        let is_upgrade = headers
            .get("upgrade")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.eq_ignore_ascii_case("websocket"))
            .unwrap_or(false);
        if !is_upgrade {
            if let Some(resp) = crate::routes::livekit::handle_http_fallback(
                &method, &path, &search, body_bytes,
            )
            .await
            {
                return resp;
            }
        }
    }

    // Matrix client/server discovery, Conduit reverse-proxy, and Cinny config
    // (server.js:7433-7667). Runs before IP ban and password gates so clients
    // and federation peers can connect.
    if path.starts_with("/.well-known/matrix/")
        || path.starts_with("/_matrix/")
        || path == "/matrix/config.json"
        || path == "/matrix/public/element-call/config.json"
        || path == "/matrix/public/element-call/config.json/"
    {
        if let Some(resp) = crate::routes::matrix::handle_matrix_gateway(
            &state, &method, &path, headers, &search, body_bytes,
        )
        .await
        {
            return resp;
        }
    }

    // 3b2. IP ban gate — server.js:7677-7684, immediately after getRealIp.
    // Bans apply to every path except the appeal set.
    {
        let ip = get_real_ip(headers, None);
        if let Some(ip_ban) = mitch_lib::auth::banned_info_for_ip(&state.store, &ip) {
            if !BAN_OPEN_PATHS.contains(&path.as_str()) {
                let reason = ip_ban
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("This IP address is banned from the website.");
                let by = ip_ban
                    .get("by")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("site admin");
                return banned_response(reason, by);
            }
        }

        // Dynamically track the user's last known IP address (server.js:10478-10495)
        let cookie_header = headers
            .get(axum::http::header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let cookies = mitch_lib::auth::get_cookies_from_header_value(
            cookie_header,
            &state.store,
            &state.id_secret,
            node_env_test,
        );
        let sid = cookies
            .get("studentId")
            .filter(|s| !s.is_empty())
            .or_else(|| cookies.get("id"))
            .unwrap_or("");
        if !sid.is_empty() && mitch_lib::auth::valid_id(sid, &state.id_secret) {
            if let Some(email) =
                mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, sid)
            {
                let norm = mitch_lib::auth::normalize_email(&email);
                if !norm.is_empty() {
                    let ips_file = state.data_dir().join("last_known_ips.json");
                    let mut ips = state.store.read_document(&ips_file, serde_json::json!({}));
                    if ips.get(&norm).and_then(|v| v.as_str()) != Some(&ip) {
                        if let Some(obj) = ips.as_object_mut() {
                            obj.insert(norm, serde_json::json!(ip));
                            let _ = state.store.write_document(&ips_file, &ips);
                        }
                    }
                }
            }
        }
    }

    // Direct unsubscribe links by token (server.js:10291-10476)
    if method == Method::GET && path.starts_with("/unsubscribe/") {
        if let Some(resp) = handle_unsubscribe(&state, &path) {
            return resp;
        }
    }

    // Open verification endpoint (server.js:10517-10548)
    if path == "/verify-open.json" {
        return crate::routes::misc::verify_open(&method);
    }

    // Mad Libs CORS-friendly API (cross-origin friendly for Pyodide, curl, etc.)
    if path == "/api/madlibs" || path.starts_with("/api/madlibs/") {
        return crate::routes::madlibs::handle(&state, &method, &path, &search);
    }

    // Direct .onion URL or /tor/<url> navigation
    if path.contains(".onion") {
        let clean = path.trim_start_matches('/');
        let target_url = if let Some(stripped) = clean.strip_prefix("tor/") {
            stripped.to_string()
        } else {
            clean.to_string()
        };
        let target_url =
            if !target_url.starts_with("http://") && !target_url.starts_with("https://") {
                format!("http://{target_url}")
            } else {
                target_url
            };
        let target_url = if search.is_empty() {
            target_url
        } else if search.starts_with('?') {
            format!("{target_url}{search}")
        } else {
            format!("{target_url}?{search}")
        };
        let encoded: String = form_urlencoded::byte_serialize(target_url.as_bytes()).collect();
        return redirect(&format!("/tor/?url={encoded}"), 302);
    }

    if path.starts_with("/tor/")
        && path != "/tor/"
        && path != "/tor/index.html"
        && path != "/tor/view"
        && path != "/tor/view/"
        && path != "/tor/resource"
        && path != "/tor/resource/"
    {
        let sub = path.trim_start_matches("/tor/").trim_start_matches('/');
        if !sub.is_empty() {
            let target_url = if !sub.starts_with("http://") && !sub.starts_with("https://") {
                format!("http://{sub}")
            } else {
                sub.to_string()
            };
            let target_url = if search.is_empty() {
                target_url
            } else if search.starts_with('?') {
                format!("{target_url}{search}")
            } else {
                format!("{target_url}?{search}")
            };
            let encoded: String = form_urlencoded::byte_serialize(target_url.as_bytes()).collect();
            return redirect(&format!("/tor/?url={encoded}"), 302);
        }
    }

    // Open general proxy removed -> game proxy redirects & 410 gone (server.js:10777-10791)
    if let Some(resp) = crate::routes::proxy::prox_redirect_or_gone(&path, &search) {
        return resp;
    }

    // GameMonetize thumbnail proxy (server.js:10838-10877)
    if let Some(resp) = crate::routes::proxy::gm_icon_proxy(&state, &path).await {
        return resp;
    }

    // Fixed-origin game proxy (server.js:10878-10947)
    if let Some(resp) =
        crate::routes::proxy::game_proxy(&state, &method, &path, &search, headers).await
    {
        return resp;
    }

    // SvelteKit _app/ redirect to pirate voyage (server.js:10949-10955)
    if let Some(resp) = crate::routes::proxy::pirate_voyage_app_redirect(&path, &search, headers) {
        return resp;
    }

    // Pirate Voyage proxy (server.js:10956-11112)
    if let Some(resp) = crate::routes::proxy::pirate_voyage_proxy(
        &state, &method, &path, &search, headers, body_bytes,
    )
    .await
    {
        return resp;
    }

    // 3c. Admin gate — server.js:7945-7973, which sits BEFORE the captcha
    // proxy, the maintenance gate, the banned check, and password enforcement.
    // Every /api/admin/* path except passphrase-status requires a valid sid,
    // isAnyAdminId, and (for full admins) the X-Admin-Passphrase header.
    if path.starts_with("/api/admin/")
        && path != "/api/admin/passphrase-status"
        && path != "/api/admin/cache/refresh"
    {
        if let Some(resp) = crate::routes::admin::admin_gate(&state, headers) {
            return resp;
        }
    }

    // 4. Soft-maintenance gate.
    if state.soft_maintenance_active() {
        let exempt = path == "/maintenance.html"
            || path == "/cookie-consent.js"
            || path == "/favicon.ico"
            || path == "/favicon.webp"
            || path.starts_with("/api/admin")
            || path.starts_with("/api/moderator")
            || path == "/api/login"
            || path == "/api/userdata"
            || (path.contains('.') && !path.ends_with(".html"));
        if !exempt {
            if path.starts_with("/api/") {
                return json_resp(
                    503,
                    serde_json::json!({
                        "error": "maintenance",
                        "message": "System is currently undergoing offline maintenance."
                    }),
                );
            }
            return redirect("/maintenance.html", 302);
        }
    }

    // 4a. Account-ban gate — server.js:8256-8271, between maintenance and
    // password enforcement. Banned sessions get the ban page (HTML) or a JSON
    // error for /api/ paths, except on the appeal paths.
    {
        let cookie_header = headers
            .get(axum::http::header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let cookies = mitch_lib::auth::get_cookies_from_header_value(
            cookie_header,
            &state.store,
            &state.id_secret,
            node_env_test,
        );
        let sid = cookies
            .get("studentId")
            .filter(|s| !s.is_empty())
            .or_else(|| cookies.get("id"))
            .unwrap_or("")
            .to_string();
        if let Some(ban) =
            mitch_lib::auth::banned_info_for_sid(&state.store, &state.id_secret, &sid)
        {
            if !BAN_OPEN_PATHS.contains(&path.as_str()) {
                if path.starts_with("/api/") {
                    let reason = ban
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .unwrap_or("This account is banned from the website.");
                    let code = if path == "/api/pass" { 200 } else { 403 };
                    return json_resp(
                        code,
                        serde_json::json!({
                            "success": false,
                            "banned": true,
                            "error": "account banned",
                            "reason": reason,
                        }),
                    );
                }
                let reason = ban
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("This account is banned from the website.");
                let by = ban
                    .get("by")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| {
                        ban.get("admin")
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                            .unwrap_or("site admin")
                    });
                return banned_response(reason, by);
            }
        }
    }

    let cfg = &state.cfg;
    let webroot = cfg.webroot.clone();

    // 4a2. Public overlay directory: anything dropped in webserver/public/
    // is served at the site root with no login required — a file at
    // webserver/public/foo.html becomes reachable, unauthenticated, at
    // /foo.html. Checked ahead of the password gate below (so it can
    // actually bypass it) and falls through to the normal, possibly
    // auth-gated, webroot lookup when nothing matches under public/. "/"
    // itself is excluded — public/ always exists as a directory, which
    // would otherwise shadow the real homepage; a directory only counts
    // as a match when it actually has an index.html/.htm to serve, so an
    // empty or partial public/<dir>/ doesn't shadow the real one either.
    let public_root = webroot.join("public");
    if path != "/" {
        if let Some(public_candidate) = safe_webroot_path(&public_root, &path) {
            let matches = std::fs::metadata(&public_candidate)
                .map(|m| {
                    m.is_file()
                        || (m.is_dir()
                            && (public_candidate.join("index.html").is_file()
                                || public_candidate.join("index.htm").is_file()))
                })
                .unwrap_or(false);
            if matches {
                return serve_static(
                    &state.static_cache,
                    &public_root,
                    &path,
                    Some(headers),
                    |html| crate::pipeline::serve_static_html(&state, headers, &path, html),
                );
            }
        }
    }

    // 4b. Password Enforcement (Unified) — server.js ~8300-8330 (merged
    // tree: games/matrix/game-portal/msn-games/rjuhsd/sexypickleclub are
    // public, plus /api/me/coins).
    let clean_path = {
        let trimmed = path.trim_end_matches('/');
        if path.ends_with('/') && path != "/" {
            trimmed.to_string()
        } else {
            path.clone()
        }
    };
    // Help and consent documents must be readable before account creation.
    let public_help_base = clean_path.strip_suffix(".html").unwrap_or(&clean_path);
    let public_help_base = public_help_base
        .strip_suffix("/index")
        .unwrap_or(public_help_base);
    let is_exempt = matches!(
        public_help_base,
        "/faq" | "/privacy" | "/use-agreement" | "/cookies" | "/copyright"
    )
        || clean_path == "/enroll"
        || clean_path == "/api/me/coins"
        || clean_path == "/larp"
        || clean_path == "/larp/rezero"
        || clean_path == "/bell"
        || clean_path == "/preferences"
        || clean_path == "/api/bell/override"
        || clean_path == "/claim"
        || clean_path == "/unsubscribe"
        || path.starts_with("/unsubscribe/")
        || crate::state::PUBLIC_API_PATHS.contains(&clean_path.as_str())
        || clean_path.starts_with("/games")
        || clean_path == "/matrix"
        || clean_path.starts_with("/matrix/")
        || clean_path.starts_with("/api/matrix/")
        || clean_path == "/livekit"
        || clean_path.starts_with("/livekit/")
        || clean_path.starts_with("/game-portal")
        || clean_path.starts_with("/msn-games")
        || clean_path == "/rjuhsd"
        || clean_path.starts_with("/rjuhsd/")
        || clean_path == "/sexypickleclub"
        || clean_path.starts_with("/sexypickleclub/")
        || path.starts_with("/api/puzzle/")
        || path == "/api/sms-reply"
        || path.starts_with("/admin")
        || path.starts_with("/moderator");
    let is_asset = path.contains('.') && !path.ends_with(".html");
    if !is_exempt
        && !is_asset
        && path != "/ws"
        && path != "/"
        && !state.check_password_cookie(headers, None)
        && !is_pickle_host(headers)
        && !is_rjuhsd_host(headers)
    {
        if path.starts_with("/api/") {
            return json_resp(
                403,
                serde_json::json!({
                    "error": "password required",
                    "message": "Please set a password at /enroll/ to continue."
                }),
            );
        }
        return redirect("/enroll/", 302);
    }

    // 4b2. /ws global broadcast upgrade — server.js:11711-11723. The password
    // gate exempts /ws, so this arm's own origin + sid ladder is the gate,
    // exactly like the JS. GET /ws without an upgrade header falls through
    // to the page block below.
    if path == "/ws" {
        let is_upgrade = headers
            .get("upgrade")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_lowercase())
            .as_deref()
            == Some("websocket");
        if is_upgrade {
            return crate::ws::handle_upgrade(&state, headers, ws_upgrade);
        }
    }

    // 4b3. WS bridge upgrades — server.js:11725-12000, the upgrade block
    // after the password gate: /livekit/rtc (11725), /ssh/ws (11738), the
    // /vnc/ws 410 (11792), and /api/blooket-bot/ws (11797). A matching path
    // without an upgrade header falls through, exactly like the JS.
    let is_upgrade = headers
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_lowercase())
        .as_deref()
        == Some("websocket");
    // The two arms are an else-if chain so the conditional move of
    // `ws_upgrade` in the first arm can't poison the second — rustc cannot
    // reason about the disjointness of the string conditions. Safe because
    // handle_rtc_upgrade returns Some for every /livekit/rtc path.
    if is_upgrade && path.starts_with("/livekit/rtc") {
        if let Some(resp) =
            crate::routes::livekit::handle_rtc_upgrade(&state, &path, &search, ws_upgrade)
        {
            return resp;
        }
    } else if is_upgrade && path == "/ssh/ws" {
        if let Some(resp) =
            crate::routes::ssh_ws::handle_ws_upgrade(&state, &path, headers, ws_upgrade)
        {
            return resp;
        }
        // The JS falls through on an upgrade failure — reach the static/404
        // handling below.
    } else if is_upgrade && path == "/vnc/ws" {
        // server.js:11792-11794 — the legacy noVNC bridge is gone.
        return crate::routes::me::json_response(
            410,
            serde_json::json!({ "error": "This desktop connection method is no longer available." }),
        );
    } else if is_upgrade && path == "/api/blooket-bot/ws" {
        if let Some(resp) =
            crate::routes::blooket::handle_ws_upgrade(&state, &path, &search, headers, ws_upgrade)
        {
            return resp;
        }
        // The JS falls through on an upgrade failure — reach the /api/ 404
        // below rather than the static pages.
    } else if is_upgrade && path == "/api/vm/desktop/ws" {
        if let Some(resp) =
            crate::routes::vm::handle_desktop_ws_upgrade(&state, headers, &search, ws_upgrade)
        {
            return resp;
        }
    }

    // 4c. API route dispatch — the ported route groups (plan Step 7+).
    if path.starts_with("/api/") {
        // VM family (Step 13 batch 4, server.js:18849-19808).
        if path.starts_with("/api/vm/") {
            if let Some(resp) =
                crate::routes::vm::handle(&state, &method, &path, headers, body_bytes, &search)
                    .await
            {
                return resp;
            }
        }
        // Blooket-bot premium endpoints (server.js:11834-11992).
        if path.starts_with("/api/blooket-bot/") {
            if let Some(resp) =
                crate::routes::blooket::handle(&state, &method, &path, headers, body_bytes, &search)
                    .await
            {
                return resp;
            }
        }
        // Team gmail/support bridge (server.js:20094-20145, 22984-23096,
        // 23814-23867) — Bearer team-token auth, behind the password gate.
        if path.starts_with("/api/team/") {
            if let Some(resp) =
                crate::routes::team::handle(&state, &method, &path, headers, body_bytes, &search)
                    .await
            {
                return resp;
            }
        }
        // Matrix SSO login, status, notifications read, report room, and moderation suite.
        if path.starts_with("/api/matrix/") {
            if let Some(resp) = crate::routes::matrix::handle_api(
                &state, &method, &path, headers, &search, body_bytes,
            )
            .await
            {
                return resp;
            }
        }
        // Tor Browser & Onion Gateway API, View, and Resource
        if path == "/api/tor"
            || path.starts_with("/api/tor/")
            || path == "/tor/view"
            || path == "/tor/view/"
            || path == "/tor/resource"
            || path == "/tor/resource/"
        {
            if let Some(resp) =
                crate::routes::tor::handle(&state, &method, &path, headers, body_bytes, &search)
                    .await
            {
                return resp;
            }
        }
        // Web push subscription and ntfy routes
        if path.starts_with("/api/push/") || path == "/api/ntfy/topic" {
            if let Some(resp) =
                crate::routes::push::handle_push_routes(&state, &method, &path, headers, body_bytes)
            {
                return resp;
            }
        }
        // Captcha proxy (solve/submit/stats/token/next/puzzle/images).
        if let Some(resp) = crate::routes::proxy::captcha_proxy(
            &state, &method, &path, headers, &search, body_bytes,
        )
        .await
        {
            return resp;
        }
        if path == "/api/ping" && method == Method::POST {
            if let Some(resp) = crate::routes::proxy::ping(&state, &body, headers).await {
                return resp;
            }
        }
        if path == "/api/content" && (method == Method::GET || method == Method::POST) {
            if let Some(resp) = crate::routes::proxy::content(&state, &body, headers).await {
                return resp;
            }
        }
        if path.starts_with("/api/admin/") && path != "/api/admin/cache/refresh" {
            if let Some(resp) =
                crate::routes::admin::handle(&state, &method, &path, headers, &search, &body).await
            {
                return resp;
            }
            // Unmatched admin paths fall through to the static 404 below.
        }
        if let Some(resp) =
            crate::routes::misc::handle(&state, &method, &path, headers, &search, &body, body_bytes)
                .await
        {
            return resp;
        }
        // auth, signup, login, verify, reset, invite, newsletter, SSO
        if let Some(resp) =
            crate::routes::auth::handle(&state, &method, &path, headers, &search, body_bytes).await
        {
            return resp;
        }
        // webauthn / passkeys group (server.js:16288-16503, 26046-26058).
        if path.starts_with("/api/webauthn/") {
            if let Some(resp) =
                crate::routes::webauthn::handle(&state, &method, &path, headers, body_bytes).await
            {
                return resp;
            }
        }
        // Polling fallback for networks that block or interrupt WebSockets (server.js:13745-13755).
        if path == "/api/broadcast/latest" && method == Method::GET {
            let cookie_header = headers
                .get(axum::http::header::COOKIE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            let cookies = mitch_lib::auth::get_cookies_from_header_value(
                cookie_header,
                &state.store,
                &state.id_secret,
                node_env_test,
            );
            let sid = cookies
                .get("studentId")
                .filter(|s| !s.is_empty())
                .or_else(|| cookies.get("id"))
                .unwrap_or("");
            if sid.is_empty()
                || !mitch_lib::auth::valid_id(sid, &state.id_secret)
                || state.is_revoked_id(sid)
                || !state.check_password_cookie(headers, Some(sid))
            {
                return json_resp(
                    401,
                    serde_json::json!({ "error": "authentication required" }),
                );
            }
            let active_event = state.active_admin_broadcast();
            let is_active = active_event.is_some();
            let mut resp = json_resp(
                200,
                serde_json::json!({
                    "ok": true,
                    "active": is_active,
                    "event": active_event,
                }),
            );
            resp.headers_mut().insert(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("private, no-store, max-age=0"),
            );
            return resp;
        }
        // me/* group (Step 9). Runs after misc so /api/me/coins (ported in
        // the merge adaptation) keeps its existing match.
        if let Some(resp) =
            crate::routes::me::handle(&state, &method, &path, headers, &body, body_bytes).await
        {
            return resp;
        }
        // backgrounds group (server.js:9500-9544, 11370-11684).
        if path.starts_with("/api/backgrounds/") || path.starts_with("/api/bg/") {
            if let Some(resp) =
                crate::routes::backgrounds::handle(&state, &method, &path, headers, body_bytes)
                    .await
            {
                return resp;
            }
        }
        // blog group (server.js:11308-11369, 11685-11943).
        if path.starts_with("/api/blog/") {
            if let Some(resp) =
                crate::routes::blog::handle(&state, &method, &path, headers, body_bytes).await
            {
                return resp;
            }
        }
        // marketplace group (server.js:12040-12390).
        if let Some(resp) =
            crate::routes::marketplace::handle(&state, &method, &path, headers, body_bytes).await
        {
            return resp;
        }
        // friends/* group (Step 9 batch 4).
        if let Some(resp) =
            crate::routes::friends::handle(&state, &method, &path, headers, body_bytes).await
        {
            return resp;
        }
        // pickle-* group (Step 9 batch 5): The Barrel, Clubhouse, Bulletin.
        if let Some(resp) =
            crate::routes::pickle::handle(&state, &method, &path, headers, body_bytes).await
        {
            return resp;
        }
        // games group (Step 12): game-portal rewards + game stats/categories
        // (game-portal at server.js:16320, game-categories at 19081,
        // game-stats at 19107 — all before dm at 19452). Idle games land
        // here; chess-vs in a later batch.
        if let Some(resp) =
            crate::routes::games::handle(&state, &method, &path, headers, &body, body_bytes)
        {
            return resp;
        }
        // chess-vs group (Step 12): the 10 /api/chess-vs/* endpoints
        // (server.js:20483 — file order puts it before battleship at 20754).
        // Needs the raw query string for /game?id=.
        if let Some(resp) =
            crate::routes::chess_vs::handle(&state, &method, &path, headers, &search, body_bytes)
                .await
        {
            return resp;
        }
        // battleship group (Step 12): the 8 /api/battleship/* endpoints
        // (server.js:20754 — file order puts it before jeopardy at 21010).
        // Needs the raw query string for /state?id=.
        if let Some(resp) =
            crate::routes::battleship::handle(&state, &method, &path, headers, &search, body_bytes)
                .await
        {
            return resp;
        }
        // jeopardy group (Step 12): the 11 /api/jeopardy/* endpoints
        // (server.js:21010 — file order puts it before the casino group at
        // 23077). Needs the raw query string for /state?id=.
        if let Some(resp) =
            crate::routes::jeopardy::handle(&state, &method, &path, headers, &search, body_bytes)
                .await
        {
            return resp;
        }
        // casino group (Step 12): the shared prelude + instant games first;
        // blackjack/poker/slots/… land in later commits.
        if let Some(resp) =
            crate::routes::casino::handle(&state, &method, &path, headers, &body, body_bytes).await
        {
            return resp;
        }
        // members + userdata group (Step 9 batch 6).
        if let Some(resp) =
            crate::routes::members::handle(&state, &method, &path, headers, body_bytes).await
        {
            return resp;
        }
        // daily-login group (server.js:19279, 21081).
        if path.starts_with("/api/daily-login/") {
            if let Some(resp) =
                crate::routes::daily_login::handle(&state, &method, &path, headers, body_bytes)
                    .await
            {
                return resp;
            }
        }
        // dm group (Step 11) — wired before canvas (JS file order:
        // dm at 19452, canvas at 21634+).
        if let Some(resp) =
            crate::routes::dm::handle(&state, &method, &path, headers, body_bytes, &search).await
        {
            return resp;
        }
        // e2e group (Step 11 batch 4) — get-key at server.js:13739 (before dm
        // at 19452), the rest at 15371-15795; paths are disjoint from dm so
        // the wiring order only mirrors the JS file position.
        if let Some(resp) =
            crate::routes::e2e::handle(&state, &method, &path, headers, body_bytes, &search).await
        {
            return resp;
        }
        // canvas group (Step 10 batch 1).
        if let Some(resp) = crate::routes::canvas::handle(
            &state, &method, &path, headers, &search, &body, body_bytes,
        )
        .await
        {
            return resp;
        }
        // Unmatched /api/ paths fall through to static 404 (same as bun).
    }

    // 4d. Non-GET fallthrough — server.js:18619/24851. The entire page/static
    // section lives inside `if (method === 'GET')`, so any request method no
    // top-level route block claimed (POST/PUT/DELETE/HEAD/OPTIONS on an
    // unclaimed path) skips the page section entirely and lands on the final
    // errResp(405). Everything before this point — CSRF, rate limits, ban
    // gates, the password gate — applies to every method, which is why a
    // CSRF-exempt upload POST surfaces the password gate in dev, not 405.
    if method == Method::HEAD && path == crate::static_files::SITE_TOUR_PATH {
        return crate::static_files::serve_tour_video(
            &state.static_cache,
            &state.cfg.webroot,
            headers,
            true,
        );
    }
    if method != Method::GET {
        return err_resp(405, None, None);
    }

    if public_help_base == "/faq" && !is_rjuhsd_host(headers) && !is_pickle_host(headers) {
        return redirect("/#help", 302);
    }

    // 5. /team route (GET) — injectReadability of the team page.
    if path == "/team" || path == "/team/" || path == "/team/index.html" {
        if let Ok(html) = std::fs::read_to_string(webroot.join("team/index.html")) {
            return html_response(inject_readability(&html, &path));
        }
    }

    let req_host = request_host(headers);
    let pickle_host = is_pickle_host(headers);
    let rjuhsd_host = is_rjuhsd_host(headers);

    // 6. Per-host manifests (default host falls through to static).
    if path == "/manifest.json" {
        if pickle_host {
            return manifest_response(crate::manifests::pickle_manifest());
        }
        if rjuhsd_host {
            return manifest_response(crate::manifests::rjuhsd_manifest());
        }
    }

    // 7. Site hubs and previews.
    let pickle_hub_html = || -> Option<String> {
        std::fs::read_to_string(webroot.join("sexypickleclub").join("index.html"))
            .ok()
            .map(|h| inject_shared_head(&h))
    };
    let rjuhsd_hub_html = |req_host_val: Option<&str>, is_rjuhsd_val: bool| -> Option<String> {
        std::fs::read_to_string(webroot.join("rjuhsd").join("index.html"))
            .ok()
            .map(|h| prepare_rjuhsd_html(&h, req_host_val, is_rjuhsd_val, &search, &state.cfg))
    };

    if (path == "/" || path == "/index.html") && pickle_host {
        if let Some(html) = pickle_hub_html() {
            return html_response(html);
        }
    }
    if (path == "/" || path == "/index.html") && rjuhsd_host {
        if let Some(html) = rjuhsd_hub_html(Some(&req_host), true) {
            return html_response(html);
        }
    }
    if path == "/sexypickleclub"
        || path == "/sexypickleclub/"
        || path == "/sexypickleclub/index.html"
    {
        if let Some(html) = pickle_hub_html() {
            return html_response(html);
        }
        return err_resp(404, Some("Not found"), None);
    }
    if pickle_host && path != "/" && path != "/index.html" {
        if let Some(resp) = site_page_block(
            &webroot.join("sexypickleclub"),
            &path, &search, true, "sexypickleclub.com",
            "pickle-bridge",
            "This page isn't part of the Sexy Pickle Club.",
            "The whole club lives on one page — sexypickleclub.com/ — and the rest of the network is over on mitch.pro. Sign in from the front door and you're in.",
            headers, state.clone(),
        ) {
            return resp;
        }
        // Site-local assets; everything else falls through to the shared root.
        if let Some(resp) = pickle_asset_response(&webroot, &path) {
            return resp;
        }
    }
    if path == "/rjuhsd" || path == "/rjuhsd/" || path == "/rjuhsd/index.html" {
        if let Some(html) = rjuhsd_hub_html(Some(&req_host), rjuhsd_host) {
            return html_response(html);
        }
        return err_resp(404, Some("Not found"), None);
    }
    if rjuhsd_host && path != "/" {
        if let Some(resp) = site_page_block(
            &webroot.join("rjuhsd"),
            &path, &search, false, "rjuhsd.school",
            "rjuhsd-bridge",
            "This page isn't part of rjuhsd.school.",
            "Looking for something else? The rjuhsd.school hub lives here — mitch.pro pages have their own home at mitch.pro.",
            headers, state.clone(),
        ) {
            return resp;
        }
    }

    // 8. Trailing-slash redirect for directories under WEBROOT.
    if path != "/" && !path.ends_with('/') {
        if let Some(disk) = safe_webroot_path(&webroot, &path) {
            if std::fs::metadata(&disk)
                .map(|m| m.is_dir())
                .unwrap_or(false)
            {
                let q = if search.is_empty() {
                    String::new()
                } else {
                    format!("?{search}")
                };
                return redirect(&format!("{path}/{q}"), 302);
            }
        }
    }

    // 9. Enroll: signed-in ?next= skip (SSO bridge; real gate at Step 6).
    if path == "/enroll/" || path == "/enroll/index.html" || path == "/enroll.html" {
        let has_enroll_param = ["ref", "email", "code", "token", "claim", "reset"]
            .iter()
            .any(|k| query(&search).contains_key(*k));
        let next_raw = query(&search).get("next").cloned().unwrap_or_default();
        if method == Method::GET && !next_raw.is_empty() && !has_enroll_param {
            if let Some(_next) = sso_back_allowed(&state.cfg, &next_raw, &req_host) {
                // checkPasswordCookie stub is false — no redirect yet (Step 6).
            }
        }
    }

    // 10. HTML auth gate — server.js:24625-24643 (merged tree: /games and
    // /matrix are open; banned sessions get the ban page; /admin/vms needs a
    // fully-authenticated VM actor).
    let mut html_base = path.clone();
    if html_base.ends_with(".html") {
        html_base = html_base[..html_base.len() - 5].to_string();
    }
    if html_base.ends_with('/') && html_base.len() > 1 {
        html_base = html_base[..html_base.len() - 1].to_string();
    }
    let is_html_request = path.ends_with(".html") || path.ends_with('/');
    let is_open_html_page = is_html_request
        && (HTML_OPEN.contains(&html_base.as_str())
            || html_base.starts_with("/games")
            || html_base.starts_with("/matrix"));
    if is_html_request && (html_base == "/admin/vms" || html_base.starts_with("/admin/vms/")) {
        match authenticated_vm_actor(&state, headers, node_env_test) {
            None => return redirect("/enroll/", 302),
            Some((_, _, is_admin)) if !is_admin => {
                return err_resp(403, Some("Admin access required."), None)
            }
            Some(_) => {}
        }
    }
    if is_html_request && !is_open_html_page && !path.starts_with("/unsubscribe/") {
        let cookie_header = headers
            .get(axum::http::header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let cookies = mitch_lib::auth::get_cookies_from_header_value(
            cookie_header,
            &state.store,
            &state.id_secret,
            node_env_test,
        );
        let sid = cookies
            .get("studentId")
            .filter(|s| !s.is_empty())
            .or_else(|| cookies.get("id"))
            .unwrap_or("");
        if let Some(ban) = mitch_lib::auth::banned_info_for_sid(&state.store, &state.id_secret, sid)
        {
            let reason = ban
                .get("reason")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("This account is banned from the website.");
            let by = ban
                .get("by")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("site admin");
            return banned_response(reason, by);
        }
        if !state.check_password_cookie(headers, None)
            && path != "/"
            && path != "/index.html"
            && path != "/index-sales.html"
            && path != "/index-sales"
        {
            return redirect("/enroll/", 302);
        }
    }
    // .html → sibling-directory 302 (file missing but <dir>/index.html exists).
    if path.ends_with(".html") {
        let missing = safe_webroot_path(&webroot, &path)
            .map(|p| !p.exists())
            .unwrap_or(true);
        if missing {
            let dir_name = &path[1..path.len() - 5];
            if !dir_name.is_empty() {
                if let Some(index_path) =
                    safe_webroot_path(&webroot, &format!("{dir_name}/index.html"))
                {
                    if index_path.exists() {
                        return redirect(&format!("/{dir_name}/{search}"), 302);
                    }
                }
            }
        }
    }

    // SPA route fallback for Matrix Chat (/matrix/*) — server.js:24647-24657.
    if path.starts_with("/matrix/") {
        let rel_path = path.strip_prefix("/matrix/").unwrap_or("");
        if rel_path == "version" {
            let candidate = std::path::Path::new(&webroot).join("matrix").join(rel_path);
            if let Ok(content) = std::fs::read(&candidate) {
                return Response::builder()
                    .status(StatusCode::OK)
                    .header(
                        axum::http::header::CONTENT_TYPE,
                        "text/plain; charset=utf-8",
                    )
                    .body(axum::body::Body::from(content))
                    .expect("static file response");
            }
        } else if rel_path == "apple-app-site-association" {
            let candidate = std::path::Path::new(&webroot).join("matrix").join(rel_path);
            if let Ok(content) = std::fs::read(&candidate) {
                return Response::builder()
                    .status(StatusCode::OK)
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(axum::body::Body::from(content))
                    .expect("static file response");
            }
        }

        // If it's a real file that exists on disk (e.g. css, js, images), DO NOT intercept here!
        // Allow it to fall through to serve_static() which inspects MIME types, adds caching headers, etc.
        let candidate = std::path::Path::new(&webroot).join("matrix").join(rel_path);
        if !candidate.is_file() {
            let is_asset = path.starts_with("/matrix/assets/")
                || path.starts_with("/matrix/public/")
                || [
                    ".js", ".css", ".png", ".jpg", ".jpeg", ".gif", ".svg", ".ico", ".wasm",
                    ".map", ".woff", ".woff2", ".ttf", ".json",
                ]
                .iter()
                .any(|ext| path.ends_with(ext));
            if !is_asset {
                let index_path = std::path::Path::new(&webroot)
                    .join("matrix")
                    .join("index.html");
                if index_path.exists() {
                    if let Ok(content) = std::fs::read_to_string(&index_path) {
                        return Response::builder()
                            .status(StatusCode::OK)
                            .header(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")
                            .body(axum::body::Body::from(content))
                            .expect("static response");
                    }
                }
            }
        }
    }

    // 11. Main injection pipeline for .html / / pages.
    if (path.ends_with(".html") || path == "/" || (path.ends_with('/') && path.len() > 1))
        && path != "/admin.html"
        && path != "/roblox.html"
    {
        let mut is_sales_page = false;
        let mut set_trial_cookie = false;
        let file_path: Option<std::path::PathBuf> = if path == "/" || path == "/index.html" {
            let cookie_header = headers
                .get(axum::http::header::COOKIE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            let cookies = mitch_lib::auth::get_cookies_from_header_value(
                cookie_header,
                &state.store,
                &state.id_secret,
                node_env_test,
            );
            let sid = cookies
                .get("studentId")
                .filter(|s| !s.is_empty())
                .or_else(|| cookies.get("id"))
                .unwrap_or("");
            let is_authenticated = !sid.is_empty()
                && mitch_lib::auth::valid_id(sid, &state.id_secret)
                && !state.is_revoked_id(sid)
                && state.check_password_cookie(headers, Some(sid));
            let wants_trial =
                query(&search).contains_key("trial") || cookies.get("mitch_trial") == Some("1");
            if is_authenticated || wants_trial {
                if query(&search).contains_key("trial") {
                    set_trial_cookie = true;
                }
                safe_webroot_path(&webroot, "index.html")
            } else {
                is_sales_page = true;
                safe_webroot_path(&webroot, "index-sales.html")
            }
        } else if path.ends_with('/') {
            safe_webroot_path(
                &webroot,
                &format!("{}index.html", path.trim_start_matches('/')),
            )
        } else {
            safe_webroot_path(&webroot, &path)
        };
        if let Some(ref fp) = file_path {
            if fp.to_string_lossy().ends_with("index-sales.html") {
                is_sales_page = true;
            }
        }
        if let Some(fp) = file_path {
            if fp.exists() && std::fs::metadata(&fp).map(|m| !m.is_dir()).unwrap_or(false) {
                if let Ok(raw) = std::fs::read(&fp) {
                    if path == "/vms/desktop"
                        || path == "/vms/desktop/"
                        || path == "/vms/desktop/index.html"
                        || path.starts_with("/matrix/public/element-call/")
                    {
                        return Response::builder()
                            .status(StatusCode::OK)
                            .header(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")
                            .header(axum::http::header::CACHE_CONTROL, "public, max-age=86400")
                            .body(axum::body::Body::from(raw))
                            .expect("vm desktop response");
                    }
                    if let Ok(mut html) = String::from_utf8(raw) {
                        let body_str = crate::pipeline::inject_page(
                            &state,
                            headers,
                            &path,
                            &html_base,
                            &mut html,
                            is_sales_page,
                        );
                        let mut resp = html_response(body_str);
                        if set_trial_cookie {
                            let node_env_production =
                                std::env::var("NODE_ENV").unwrap_or_default() == "production";
                            let secure_flag =
                                std::env::var("SESSION_COOKIE_SECURE").unwrap_or_default();
                            let cookie = mitch_lib::auth::set_cookie_header(
                                "mitch_trial",
                                "1",
                                &secure_flag,
                                node_env_production,
                                86400,
                                false,
                            );
                            if let Ok(hv) = axum::http::HeaderValue::from_str(&cookie) {
                                resp.headers_mut()
                                    .append(axum::http::header::SET_COOKIE, hv);
                            }
                        }
                        return resp;
                    }
                }
            }
        }
        // fall through to static serving on failure
    }

    // 12. Protected theme files (unauthenticated → bare 403).
    let base_name = path.rsplit('/').next().unwrap_or("");
    if PROTECTED_FILES.contains(&base_name) {
        return Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body(axum::body::Body::empty())
            .expect("static response");
    }

    // 13. Public assets gate (unauthenticated) — server.js:24838 (merged
    // tree: /matrix/, /games, /game-portal/ are public; banned sessions get
    // the ban page before the redirect).
    let clean_path = html_base.clone();
    let is_piece_svg = path.starts_with("/games/chess-bot/pieces-svg/") && path.ends_with(".svg");
    let public_api = crate::state::PUBLIC_API_PATHS.contains(&clean_path.as_str());
    let is_larp = path == "/larp" || path.starts_with("/larp/");
    if !is_open_html_page
        && !public_api
        && !PUBLIC_ASSETS.contains(&path.as_str())
        && !is_piece_svg
        && !path.starts_with("/matrix/")
        && !path.starts_with("/tor/")
        && path != "/tor"
        && !path.starts_with("/unsubscribe/")
        && !path.starts_with("/images/")
        && !path.starts_with("/backgrounds/")
        && !is_larp
        && !path.starts_with("/games")
        && !path.starts_with("/game-portal/")
        && !state.check_password_cookie(headers, None)
    {
        let cookie_header = headers
            .get(axum::http::header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let cookies = mitch_lib::auth::get_cookies_from_header_value(
            cookie_header,
            &state.store,
            &state.id_secret,
            node_env_test,
        );
        let sid = cookies
            .get("studentId")
            .filter(|s| !s.is_empty())
            .or_else(|| cookies.get("id"))
            .unwrap_or("");
        if let Some(ban) = mitch_lib::auth::banned_info_for_sid(&state.store, &state.id_secret, sid)
        {
            let reason = ban
                .get("reason")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("This account is banned from the website.");
            let by = ban
                .get("by")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("site admin");
            return banned_response(reason, by);
        }
        return redirect("/enroll/", 302);
    }

    // The tour supports byte ranges so native video controls and chapters
    // can seek without downloading the entire recording first.
    if path == crate::static_files::SITE_TOUR_PATH {
        return crate::static_files::serve_tour_video(
            &state.static_cache,
            &webroot,
            headers,
            false,
        );
    }

    // 14. Static.
    serve_static(
        &state.static_cache,
        &webroot,
        &path,
        Some(headers),
        |html| crate::pipeline::serve_static_html(&state, headers, &path, html),
    )
}

fn html_response(html: String) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(axum::body::Body::from(html))
        .expect("static response")
}

/// `banOpenPaths` — server.js:7605. Exact-path matches only.
const BAN_OPEN_PATHS: &[&str] = &["/appeal.html", "/api/appeal", "/api/pass"];

/// `authenticatedVmActor(req)` — server.js:25606. Valid non-revoked sid whose
/// password cookie matches, with a normalized email; returns (sid, email,
/// is_admin).
fn authenticated_vm_actor(
    state: &AppState,
    headers: &HeaderMap,
    node_env_test: bool,
) -> Option<(String, String, bool)> {
    let cookie_header = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let cookies = mitch_lib::auth::get_cookies_from_header_value(
        cookie_header,
        &state.store,
        &state.id_secret,
        node_env_test,
    );
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("")
        .to_string();
    if sid.is_empty() || !mitch_lib::auth::valid_id(&sid, &state.id_secret) {
        return None;
    }
    let revoked: serde_json::Value = state.store.read_document(
        &state.cfg.data_dir.join("revoked.json"),
        serde_json::json!({}),
    );
    if revoked.get(&sid).is_some() {
        return None;
    }
    if !state.check_password_cookie(headers, Some(&sid)) {
        return None;
    }
    let email = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid)?;
    let norm = mitch_lib::auth::normalize_email(&email);
    if norm.is_empty() {
        return None;
    }
    let is_admin =
        mitch_lib::auth::is_admin_id(&state.store, &state.id_secret, &sid, node_env_test);
    Some((sid, norm, is_admin))
}

/// `bannedResponse(info)` — server.js:5004. 403 HTML ban page with an alert
/// script; `reason`/`by` are HTML-escaped, the alert text is JSON-escaped
/// with `<` rewritten to the `<` sequence (matching the JS).
fn banned_response(reason: &str, by: &str) -> Response {
    let reason_esc = crate::errors::html_esc(reason);
    let by_esc = crate::errors::html_esc(by);
    let alert_raw = format!("This account is banned from the website. Reason: {reason}");
    let alert_json = serde_json::to_string(&alert_raw).unwrap_or("\"\"".into());
    let alert_text = alert_json.replace('<', "\\u003c");
    let html = format!(
        r#"<!DOCTYPE html><html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Account Banned</title>
<style>
body{{margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;background:#111318;color:#f8fafc;font-family:system-ui,sans-serif;padding:24px;}}
.modal{{width:min(440px,100%);background:#1b1f2a;border:1px solid rgba(248,113,113,.35);border-radius:14px;box-shadow:0 24px 80px rgba(0,0,0,.45);padding:24px;text-align:center;}}
.badge{{display:inline-flex;align-items:center;justify-content:center;width:44px;height:44px;border-radius:999px;background:rgba(239,68,68,.14);color:#fecaca;font-weight:900;margin-bottom:14px;}}
h1{{font-size:1.35rem;margin:0 0 10px;}}
p{{color:#cbd5e1;line-height:1.5;margin:0 0 12px;font-size:.94rem;}}
.reason{{background:#111827;border:1px solid rgba(148,163,184,.18);border-radius:10px;padding:12px;margin:14px 0;color:#e5e7eb;text-align:left;}}
.small{{font-size:.78rem;color:#94a3b8;}}
button{{margin-top:8px;border:0;border-radius:9px;background:#ef4444;color:white;padding:10px 16px;font-weight:700;cursor:pointer;}}
</style></head><body>
<div class="modal" role="dialog" aria-modal="true" aria-labelledby="ban-title">
  <div class="badge">!</div>
  <h1 id="ban-title">This account is banned from the website</h1>
  <p>Your account cannot access mitch.pro right now.</p>
  <div class="reason"><strong>Reason:</strong><br>{reason_esc}</div>
  <p class="small">Issued by {by_esc}. Contact site staff if you think this was a mistake.</p>
  <button onclick="location.href='/appeal.html'">Appeal ban</button>
</div>
<script>alert({alert_text});</script>
</body></html>"#,
    );
    Response::builder()
        .status(StatusCode::FORBIDDEN)
        .header(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(axum::body::Body::from(html))
        .expect("static response")
}

/// `csrfFailureIfUnsafe` — only mutating /api/ requests, minus the exempt set.
fn csrf_check(
    state: &AppState,
    headers: &HeaderMap,
    path: &str,
    method: &Method,
) -> Option<Response> {
    if std::env::var("NODE_ENV").unwrap_or_default() == "test" {
        return None;
    }
    if !path.starts_with("/api/") {
        return None;
    }
    if method == Method::GET || method == Method::HEAD || method == Method::OPTIONS {
        return None;
    }
    if crate::state::CSRF_EXEMPT_PATHS.contains(&path) {
        return None;
    }
    if path == "/api/presence/heartbeat" || path == "/api/admin/passphrase-status" {
        if !crate::hosts::same_origin_request(headers, Some(&state.cfg)) {
            return Some(json_resp(403, serde_json::json!({"error": "csrf_blocked"})));
        }
        return None;
    }
    let requested_with = headers
        .get("x-mitch-requested-with")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim();
    if requested_with != "1" {
        return Some(json_resp(403, serde_json::json!({"error": "csrf_blocked"})));
    }
    if !crate::hosts::same_origin_request(headers, Some(&state.cfg)) {
        return Some(json_resp(403, serde_json::json!({"error": "csrf_blocked"})));
    }
    None
}

/// The pickle/rjuhsd site page blocks (shared shape in server.js ~20990-21125).
#[allow(clippy::too_many_arguments)]
fn site_page_block(
    site_webroot: &std::path::Path,
    path: &str,
    query: &str,
    members_open: bool,
    domain: &str,
    bridge_kind: &str,
    not_found_title: &str,
    not_found_detail: &str,
    headers: &HeaderMap,
    state: Arc<AppState>,
) -> Option<Response> {
    static PAGE_EXT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let is_page_path = path.ends_with('/')
        || PAGE_EXT
            .get_or_init(|| regex::Regex::new(r"(?i)\.html?$").expect("static regex"))
            .is_match(path);
    if !is_page_path {
        return None;
    }
    let mut rel = if path == "/index.html" {
        "/".to_string()
    } else {
        path.to_string()
    };
    if !rel.ends_with('/') {
        let stripped = rel.trim_start_matches('/');
        let as_dir = format!(
            "/{}",
            PAGE_EXT
                .get()
                .map(|re| re.replace_all(stripped, "").to_string())
                .unwrap_or_else(|| stripped.to_string())
        );
        let is_dir = std::fs::metadata(site_webroot.join(as_dir.trim_start_matches('/')))
            .map(|m| m.is_dir())
            .unwrap_or(false);
        if is_dir {
            let q = if query.is_empty() {
                String::new()
            } else {
                format!("?{query}")
            };
            return Some(redirect(&format!("{rel}/{q}"), 302));
        }
        rel = format!("{as_dir}/");
    }
    if rel.contains("..") {
        return Some(err_resp(404, None, None));
    }
    let file = site_webroot.join(format!("{}/index.html", rel.trim_start_matches('/')));
    let stat = std::fs::metadata(&file).ok();
    if stat.is_none() || stat.map(|s| s.is_dir()).unwrap_or(true) {
        return Some(err_resp(404, Some(not_found_title), Some(not_found_detail)));
    }
    // Gate: open pages pass; everything else redirects.
    let page_base = format!("/{}", rel.trim_start_matches('/').trim_end_matches('/'));
    let mut open =
        page_base.is_empty() || page_base == "/" || (members_open && page_base == "/members");
    if !open {
        let open_index = format!("{page_base}/index");
        open = HTML_OPEN.contains(&page_base.as_str()) || HTML_OPEN.contains(&open_index.as_str());
    }
    if !open {
        // ban + session checks (bans stubbed; sessions come at Step 6).
        if !state.check_password_cookie(headers, None) {
            let _ = bridge_kind;
            if domain == "rjuhsd.school" {
                return Some(redirect(
                    &format!("/enroll/?next={}", encode_uri_component(&rel)),
                    302,
                ));
            } else {
                // encodeURIComponent(origin + rel), like the JS.
                let origin = format!("https://{domain}");
                let encoded = encode_uri_component(&format!("{origin}{rel}"));
                return Some(redirect(&format!("/api/sso/bridge?back={encoded}"), 302));
            }
        }
    }
    match std::fs::read_to_string(&file) {
        Ok(mut html) => {
            if domain == "rjuhsd.school" {
                let req_host = request_host(headers);
                html = prepare_rjuhsd_html(&html, Some(&req_host), true, query, &state.cfg);
            } else {
                html = inject_shared_head(&html);
            }
            let rc_key = std::env::var("RECAPTCHA_SITE_KEY")
                .unwrap_or_default()
                .trim()
                .to_string();
            let recaptcha_host = std::env::var("RECAPTCHA_SCRIPT_HOST")
                .unwrap_or_else(|_| "www.recaptcha.net".into())
                .trim()
                .to_string();
            if !rc_key.is_empty() && !html.contains("recaptcha/api.js") {
                html = html.replacen(
                    "</head>",
                    &format!(
                        "{}{}",
                        recaptcha_loader_str(&recaptcha_host, &rc_key),
                        "</head>"
                    ),
                    1,
                );
            }
            Some(html_response(html))
        }
        Err(_) => Some(err_resp(404, None, None)),
    }
}

fn is_hex_32_to_64(s: &str) -> bool {
    (32..=64).contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn handle_unsubscribe(state: &Arc<AppState>, path: &str) -> Option<Response> {
    let token = path.strip_prefix("/unsubscribe/")?.trim();
    if token.is_empty() || token == "index.html" || !is_hex_32_to_64(token) {
        return None;
    }
    let tokens_file = state.data_dir().join("unsubscribe_tokens.json");
    let tokens = state
        .store
        .read_document(&tokens_file, serde_json::json!({}));
    let mut matched_email: Option<String> = None;
    if let Some(obj) = tokens.as_object() {
        for (k, v) in obj {
            if v.as_str() == Some(token) {
                matched_email = Some(k.clone());
                break;
            }
        }
    }
    if let Some(email) = matched_email {
        let norm_email = email.to_ascii_lowercase().trim().to_string();
        let unsub_file = state.data_dir().join("newsletter_unsub.json");
        let unsub = state
            .store
            .read_document(&unsub_file, serde_json::json!([]));
        let mut list: Vec<String> = unsub
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        if !list.contains(&norm_email) {
            list.push(norm_email);
            list.sort();
            list.dedup();
            let _ = state
                .store
                .write_document(&unsub_file, &serde_json::json!(list));
            let _ = std::fs::write(
                &unsub_file,
                mitch_lib::data::js_stringify_pretty(&serde_json::json!(list)),
            );
        }
        Some(unsubscribe_success_html(&email))
    } else {
        Some(unsubscribe_invalid_html())
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn unsubscribe_success_html(email: &str) -> Response {
    let clean_email = html_escape(email);
    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>Unsubscribed successfully — mitch.pro</title>
  <link rel="stylesheet" href="/open.css">
  <script src="/theme.js"></script>
  <style>
    body {{
      background: var(--bg);
      color: var(--t-fg);
      font-family: system-ui, -apple-system, sans-serif;
      display: flex;
      align-items: center;
      justify-content: center;
      min-height: 100vh;
      margin: 0;
      padding: 20px;
      box-sizing: border-box;
    }}
    .glass-card {{
      background: var(--panel);
      backdrop-filter: blur(24px);
      -webkit-backdrop-filter: blur(24px);
      border: 1px solid var(--line);
      border-radius: 16px;
      padding: 40px 30px;
      max-width: 480px;
      width: 100%;
      box-shadow: 0 20px 50px rgba(0,0,0,0.4);
      text-align: center;
    }}
    h1 {{
      font-family: 'Syne', sans-serif;
      font-size: 2rem;
      margin: 0 0 10px 0;
      color: var(--t-fg);
    }}
    p {{
      color: var(--t-fg2);
      font-size: 0.95rem;
      line-height: 1.6;
      margin: 0 0 24px 0;
    }}
    .email-display {{
      background: rgba(255,255,255,0.02);
      border: 1px solid var(--line);
      padding: 12px;
      border-radius: 8px;
      font-family: monospace;
      font-size: 0.95rem;
      color: var(--t-ac);
      font-weight: bold;
      margin-bottom: 24px;
      word-break: break-all;
    }}
    .btn {{
      display: inline-block;
      width: 100%;
      background: var(--t-ac);
      color: #fff;
      border: none;
      padding: 12px;
      border-radius: 8px;
      font-weight: bold;
      font-size: 0.95rem;
      cursor: pointer;
      text-decoration: none;
      transition: opacity 0.2s;
    }}
    .btn:hover {{
      opacity: 0.9;
    }}
    .icon {{
      font-size: 3.5rem;
      margin-bottom: 15px;
    }}
  </style>
</head>
<body>
  <div class="glass-card">
    <div class="icon">👋</div>
    <h1>Unsubscribed</h1>
    <p>You have been successfully removed from our newsletter updates.</p>
    <div class="email-display">{clean_email}</div>
    <a class="btn" href="/">Return Home</a>
  </div>
</body>
</html>"#
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(axum::body::Body::from(html))
        .unwrap_or_else(|_| crate::errors::err_resp(500, None, None))
}

fn unsubscribe_invalid_html() -> Response {
    let html = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>Invalid Unsubscribe Link — mitch.pro</title>
  <link rel="stylesheet" href="/open.css">
  <script src="/theme.js"></script>
  <style>
    body {
      background: var(--bg);
      color: var(--t-fg);
      font-family: system-ui, -apple-system, sans-serif;
      display: flex;
      align-items: center;
      justify-content: center;
      min-height: 100vh;
      margin: 0;
      padding: 20px;
      box-sizing: border-box;
    }
    .glass-card {
      background: var(--panel);
      backdrop-filter: blur(24px);
      -webkit-backdrop-filter: blur(24px);
      border: 1px solid var(--line);
      border-radius: 16px;
      padding: 40px 30px;
      max-width: 480px;
      width: 100%;
      box-shadow: 0 20px 50px rgba(0,0,0,0.4);
      text-align: center;
    }
    h1 {
      font-family: 'Syne', sans-serif;
      font-size: 2rem;
      margin: 0 0 10px 0;
      color: var(--rose);
    }
    p {
      color: var(--t-fg2);
      font-size: 0.95rem;
      line-height: 1.6;
      margin: 0 0 24px 0;
    }
    .btn {
      display: inline-block;
      width: 100%;
      background: var(--t-ac);
      color: #fff;
      border: none;
      padding: 12px;
      border-radius: 8px;
      font-weight: bold;
      font-size: 0.95rem;
      cursor: pointer;
      text-decoration: none;
      transition: opacity 0.2s;
    }
    .btn:hover {
      opacity: 0.9;
    }
    .icon {
      font-size: 3.5rem;
      margin-bottom: 15px;
    }
  </style>
</head>
<body>
  <div class="glass-card">
    <div class="icon">⚠️</div>
    <h1>Invalid Link</h1>
    <p>This unsubscribe link is invalid or has expired.</p>
    <a class="btn" href="/">Return Home</a>
  </div>
</body>
</html>"#;
    Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(axum::body::Body::from(html))
        .unwrap_or_else(|_| crate::errors::err_resp(500, None, None))
}
