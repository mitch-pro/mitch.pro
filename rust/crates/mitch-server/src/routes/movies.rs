//! `/proxy/movies/` — the Movies page: a fixed-origin proxy for `www.movy.sx`.
//!
//! Everything upstream (the site itself, its browser-side APIs) is
//! fetched by this server, not by the visitor's browser, and every one of those
//! requests leaves through one VPN network interface (see [`egress_interface`]).
//! If that interface isn't present the proxy answers 503 instead of quietly
//! falling back to the server's normal connection ("fail closed").
//!
//! What is proxied, and what is not:
//! - `/proxy/movies/<path>`: GET/HEAD to `https://www.movy.sx/<path>`.
//! - `/proxy/movies/__api/<host>/<path>`: GET/HEAD/POST to one of the browser-side
//!   API hosts in [`API_HOSTS`] (an allowlist; this is never an open proxy). The
//!   injected `movies.js` points the app's calls to them here.
//! - Video segments load straight from the player's CDNs in the visitor's
//!   browser. Their hosts are only known at playback time, so they are not part
//!   of the allowlist and don't use the VPN interface.
//!
//! No Mitch cookies or Authorization headers are ever forwarded upstream, and
//! upstream cookies, CSP and frame headers are dropped. DNS lookups use the
//! container's normal resolver; only the connections are bound to the interface.
//!
//! The site is a Next.js app built for the root of its own origin. HTML is
//! rewritten to live under `/proxy/movies/` and the compiled router helpers are
//! patched to use that prefix as their base path. If movy.sx ships a new build
//! the patterns in [`rewrite_js`] are the first thing to check. Details and the
//! eth1 requirements are in `docs/movies.md`.

#![allow(clippy::expect_used)] // the regexes below are static and known-valid

use crate::state::AppState;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::Response;
use serde_json::{json, Value};
use std::sync::Arc;

const MOUNT: &str = "/proxy/movies";
const LEGACY_MOUNT: &str = "/proxy/rivestream";
const API_MOUNT: &str = "/proxy/movies/__api/";
const SITE_HOST: &str = "www.movy.sx";
const SITE_ORIGIN: &str = "https://www.movy.sx";
/// The hosts the movy.sx app calls from the browser. Anything else is refused.
const API_HOSTS: &[&str] = &[
    "backend.movy.sx",
    "db.wecollege.net",
    "api.wecollege.net",
    "trailers.wecollege.net",
    // Country lookup the site makes on load.
    "ipwho.is",
];
const DEFAULT_INTERFACE: &str = "eth1";
const MAX_REQUEST_BODY: usize = 256 * 1024;
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

pub fn is_path(path: &str) -> bool {
    [MOUNT, LEGACY_MOUNT]
        .iter()
        .any(|m| path == *m || path.starts_with(&format!("{m}/")))
}

// ── egress ──────────────────────────────────────────────────────────────────

/// The interface Movies traffic must leave through: `MOVIES_EGRESS_INTERFACE`,
/// default `eth1` (the container's VPN-routed interface).
fn egress_interface() -> String {
    std::env::var("MOVIES_EGRESS_INTERFACE")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_INTERFACE.to_string())
}

/// API hosts the visitor's browser should call itself instead of going through
/// the proxy: `MOVIES_DIRECT_API_HOSTS`, comma separated, default none. A host
/// that Cloudflare refuses to serve to the server can be listed here so that
/// feature keeps working, at the cost of that call not using the VPN interface.
fn direct_api_hosts() -> Vec<&'static str> {
    let raw = std::env::var("MOVIES_DIRECT_API_HOSTS").unwrap_or_default();
    API_HOSTS
        .iter()
        .copied()
        .filter(|host| {
            raw.split(',')
                .any(|item| item.trim().eq_ignore_ascii_case(host))
        })
        .collect()
}

/// Linux interface names: at most 15 bytes, no slashes or spaces.
fn valid_interface_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 15
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn interface_exists(name: &str) -> bool {
    valid_interface_name(name) && std::path::Path::new("/sys/class/net").join(name).exists()
}

enum EgressError {
    /// The configured interface isn't there (or the name is invalid).
    Unavailable,
    /// Anything else went wrong building the client.
    Build,
}

fn egress_client(redirects: reqwest::redirect::Policy) -> Result<reqwest::Client, EgressError> {
    let interface = egress_interface();
    if !interface_exists(&interface) {
        return Err(EgressError::Unavailable);
    }
    let builder = reqwest::Client::builder()
        .redirect(redirects)
        .timeout(std::time::Duration::from_secs(20));
    build_bound(builder, &interface)
}

/// SO_BINDTODEVICE pins every connection to the interface. It's the only way to
/// guarantee the VPN path, so on platforms without it we refuse to run.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn build_bound(
    builder: reqwest::ClientBuilder,
    interface: &str,
) -> Result<reqwest::Client, EgressError> {
    builder
        .interface(interface)
        .build()
        .map_err(|_| EgressError::Build)
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn build_bound(
    _builder: reqwest::ClientBuilder,
    _interface: &str,
) -> Result<reqwest::Client, EgressError> {
    Err(EgressError::Unavailable)
}

// ── target resolution ───────────────────────────────────────────────────────

struct Target {
    /// Full upstream URL, query included.
    url: String,
    /// Host the response should be attributed to (for the redirect policy).
    host: &'static str,
    is_api: bool,
}

fn resolve_target(path: &str, search: &str) -> Option<Target> {
    let rest = path.strip_prefix(MOUNT)?;
    let query = if search.is_empty() {
        String::new()
    } else if search.starts_with('?') {
        search.to_string()
    } else {
        format!("?{search}")
    };
    if rest.contains('\\') || rest.split('/').any(|seg| seg == "." || seg == "..") {
        return None;
    }
    if let Some(api) = path.strip_prefix(API_MOUNT) {
        let (host_key, tail) = api.split_once('/').unwrap_or((api, ""));
        let host = API_HOSTS.iter().find(|h| **h == host_key)?;
        return Some(Target {
            url: format!("https://{host}/{tail}{query}"),
            host,
            is_api: true,
        });
    }
    let site_path = if rest.is_empty() { "/" } else { rest };
    Some(Target {
        url: format!("{SITE_ORIGIN}{site_path}{query}"),
        host: SITE_HOST,
        is_api: false,
    })
}

// ── handler ─────────────────────────────────────────────────────────────────

fn text_response(status: StatusCode, message: &'static str) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .header("cache-control", "no-store")
        .body(axum::body::Body::from(message))
        .unwrap_or_else(|_| crate::errors::err_resp(status.as_u16(), None, None))
}

pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    search: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Option<Response> {
    if !is_path(path) {
        return None;
    }
    // Old links: /proxy/movies (no slash) and the previous Rivestream mount.
    if path == MOUNT || path == LEGACY_MOUNT || path.starts_with(&format!("{LEGACY_MOUNT}/")) {
        return Some(crate::static_files::redirect("/proxy/movies/", 302));
    }
    let Some(target) = resolve_target(path, search) else {
        return Some(text_response(
            StatusCode::NOT_FOUND,
            "That page isn't available.",
        ));
    };
    let allowed = if target.is_api {
        matches!(*method, Method::GET | Method::HEAD | Method::POST)
    } else {
        matches!(*method, Method::GET | Method::HEAD)
    };
    if !allowed {
        return Some(crate::errors::err_resp(
            405,
            Some("method not allowed"),
            None,
        ));
    }
    if body.len() > MAX_REQUEST_BODY {
        return Some(crate::errors::err_resp(413, None, None));
    }

    // Only follow redirects that stay on the host we asked for.
    let host = target.host;
    let policy = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() < 10
            && attempt.url().scheme() == "https"
            && attempt.url().host_str() == Some(host)
        {
            attempt.follow()
        } else {
            attempt.stop()
        }
    });
    let client = match egress_client(policy) {
        Ok(client) => client,
        Err(EgressError::Unavailable) => {
            return Some(text_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Movies are unavailable right now.",
            ))
        }
        Err(EgressError::Build) => return Some(upstream_error()),
    };

    let upstream_method = match *method {
        Method::HEAD => reqwest::Method::HEAD,
        Method::POST => reqwest::Method::POST,
        _ => reqwest::Method::GET,
    };
    let mut request = client.request(upstream_method, &target.url);
    for (name, value) in headers.iter() {
        let n = name.as_str().to_ascii_lowercase();
        let forward = matches!(
            n.as_str(),
            "accept" | "accept-language" | "range" | "user-agent"
        ) || (target.is_api && n == "content-type");
        if forward {
            if let Ok(v) = value.to_str() {
                request = request.header(name.as_str(), v);
            }
        }
    }
    if !headers.contains_key(axum::http::header::USER_AGENT) {
        request = request.header("user-agent", USER_AGENT);
    }
    request = request.header("referer", format!("{SITE_ORIGIN}/"));
    if target.is_api {
        request = request.header("origin", SITE_ORIGIN);
        if *method == Method::POST {
            request = request.body(body.to_vec());
        }
    }

    let upstream = match request.send().await {
        Ok(upstream) => upstream,
        Err(err) => {
            // Which host failed and why (never the full URL or any visitor data).
            eprintln!("[movies] request to {} failed: {err:?}", target.host);
            return Some(upstream_error());
        }
    };
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let final_path = upstream.url().path().to_string();
    let mut builder = Response::builder().status(status);

    let mut content_type = String::new();
    for (name, value) in upstream.headers().iter() {
        let n = name.as_str().to_ascii_lowercase();
        let drop = matches!(
            n.as_str(),
            "set-cookie"
                | "content-security-policy"
                | "content-security-policy-report-only"
                | "x-frame-options"
                | "cross-origin-opener-policy"
                | "cross-origin-embedder-policy"
                | "content-encoding"
                | "content-length"
                | "transfer-encoding"
                | "connection"
                | "keep-alive"
                | "access-control-allow-origin"
                | "access-control-allow-credentials"
                | "cache-control"
                | "speculation-rules"
                | "report-to"
                | "nel"
                | "location"
        );
        if drop {
            continue;
        }
        if n == "content-type" {
            if let Ok(ct) = value.to_str() {
                content_type = ct.to_ascii_lowercase();
            }
        }
        builder = builder.header(name.as_str(), value);
    }
    if status.is_redirection() {
        // A redirect that stayed on the site maps back into the mount; one that
        // left it is not forwarded.
        if let Some(location) = upstream
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .and_then(|l| mount_location(l, &target.url))
        {
            builder = builder.header("location", location);
        }
    }
    let is_html = content_type.contains("text/html");
    builder = builder.header(
        "cache-control",
        if is_html || target.is_api {
            "private, no-store"
        } else {
            "public, max-age=3600"
        },
    );
    if is_html {
        builder = builder.header(
            "content-security-policy",
            "default-src 'self' https: blob: data:; script-src 'self' 'unsafe-inline' 'unsafe-eval' blob:; style-src 'self' 'unsafe-inline' https:; object-src 'none'; worker-src 'none'; frame-ancestors 'self'",
        );
    }

    let is_text =
        is_html || content_type.contains("text/css") || content_type.contains("javascript");
    if *method == Method::HEAD || !is_text || target.is_api {
        let bytes = if *method == Method::HEAD {
            Vec::new()
        } else {
            upstream.bytes().await.unwrap_or_default().to_vec()
        };
        return Some(
            builder
                .body(axum::body::Body::from(bytes))
                .unwrap_or_else(|_| crate::errors::err_resp(500, None, None)),
        );
    }

    let mut content = upstream.text().await.unwrap_or_default();
    content = rewrite_text(&content, is_html, content_type.contains("javascript"));
    if is_html {
        content = ensure_base(&content, &final_path);
        if crate::presence::authenticated_email(state, headers).is_some() {
            content = crate::inject::inject_presence(&content, None);
        }
    }
    Some(
        builder
            .body(axum::body::Body::from(content))
            .unwrap_or_else(|_| crate::errors::err_resp(500, None, None)),
    )
}

fn upstream_error() -> Response {
    text_response(
        StatusCode::BAD_GATEWAY,
        "This movie page could not be reached.",
    )
}

/// Map an upstream `Location` back into the mount, or `None` if it left the site.
fn mount_location(location: &str, request_url: &str) -> Option<String> {
    let base = url::Url::parse(request_url).ok()?;
    let resolved = base.join(location).ok()?;
    if resolved.host_str() != Some(SITE_HOST) || resolved.scheme() != "https" {
        return None;
    }
    let mut out = format!("{MOUNT}{}", resolved.path());
    if let Some(q) = resolved.query() {
        out.push('?');
        out.push_str(q);
    }
    Some(out)
}

// ── rewriting ───────────────────────────────────────────────────────────────

fn rewrite_text(content: &str, is_html: bool, is_js: bool) -> String {
    let mut out = content.to_string();

    // Root-relative references first (HTML and CSS only; JS builds its URLs at
    // runtime and is handled by movies.js), then absolute ones. In that order
    // an absolute link can't end up with the mount prefixed twice.
    if !is_js {
        static ATTR_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let attr_re = ATTR_RE.get_or_init(|| {
            regex::Regex::new(r#"(?i)(\b(?:src|href|action|poster)\s*=\s*["'])/([^/])"#)
                .expect("static regex")
        });
        out = attr_re
            .replace_all(&out, format!("$1{MOUNT}/$2"))
            .into_owned();

        static URL_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let url_re = URL_RE.get_or_init(|| {
            regex::Regex::new(r#"(?i)url\(\s*(["']?)/([^/])"#).expect("static regex")
        });
        out = url_re
            .replace_all(&out, format!("url($1{MOUNT}/$2"))
            .into_owned();
    }
    out = out.replace(&format!("{SITE_ORIGIN}/"), &format!("{MOUNT}/"));

    if is_html {
        out = rewrite_html(&out);
    } else if is_js {
        out = rewrite_js(&out);
    }
    out
}

/// `srcset`/`imagesrcset` hold comma-separated candidates ("/img/a.webp 480w, ...").
fn rewrite_srcset(html: &str) -> String {
    static SRCSET_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = SRCSET_RE.get_or_init(|| {
        regex::Regex::new(r#"(?i)(\b(?:srcset|imagesrcset)\s*=\s*")([^"]*)(")"#)
            .expect("static regex")
    });
    re.replace_all(html, |caps: &regex::Captures<'_>| {
        let list = caps[2]
            .split(',')
            .map(|candidate| {
                let lead = candidate.len() - candidate.trim_start().len();
                let trimmed = &candidate[lead..];
                if trimmed.starts_with('/')
                    && !trimmed.starts_with("//")
                    && !trimmed.starts_with(&format!("{MOUNT}/"))
                {
                    format!("{}{MOUNT}{trimmed}", &candidate[..lead])
                } else {
                    candidate.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(",");
        format!("{}{}{}", &caps[1], list, &caps[3])
    })
    .into_owned()
}

fn rewrite_html(html: &str) -> String {
    let html = rewrite_srcset(html);
    static DATA_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let data_re = DATA_RE.get_or_init(|| {
        regex::Regex::new(
            r#"(?s)(<script\b[^>]*\bid=["']__NEXT_DATA__["'][^>]*>)(.*?)(</script\s*>)"#,
        )
        .expect("static regex")
    });
    let html = data_re.replace(&html, |caps: &regex::Captures<'_>| {
        let Ok(mut data) = serde_json::from_str::<Value>(&caps[2]) else {
            return caps[0].to_string();
        };
        let Some(object) = data.as_object_mut() else {
            return caps[0].to_string();
        };
        // Next loads its chunks relative to assetPrefix.
        object.insert("assetPrefix".to_string(), json!(MOUNT));
        format!(
            "{}{}{}",
            &caps[1],
            data.to_string().replace('<', "\\u003c"),
            &caps[3]
        )
    });
    let runtime = include_str!("movies.js");
    let direct = serde_json::to_string(&direct_api_hosts()).unwrap_or_else(|_| "[]".into());
    html.replacen(
        "<head>",
        &format!(
            r#"<head><base href="{MOUNT}/"><script>window.MOVIES_DIRECT_HOSTS={direct};{runtime}</script>"#
        ),
        1,
    )
}

/// Next's compiled build assumes it owns `/`. Give its router the mount as a
/// base path while internal page names (such as `/watch`) stay unchanged.
fn rewrite_js(js: &str) -> String {
    let mut js = js.replace("this.basePath=\"\"", &format!("this.basePath=\"{MOUNT}\""));
    // addBasePath / hasBasePath: addPathPrefix(path,"") and pathHasPrefix(path,"")
    static ADD_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let add_re = ADD_RE.get_or_init(|| {
        regex::Regex::new(r#"(\b(?:addPathPrefix|pathHasPrefix)\)\([\w$]+,\s*)"""#)
            .expect("static regex")
    });
    js = add_re
        .replace_all(&js, |caps: &regex::Captures<'_>| {
            format!(r#"{}"{MOUNT}""#, &caps[1])
        })
        .into_owned();
    // Turbopack's runtime names each chunk by stripping a hard-coded "/_next/"
    // from its <script src>. The chunks load from under the mount, so tell the
    // runtime that is where they live; otherwise no chunk ever matches what the
    // runtime is waiting for and the app silently never starts.
    static RUNTIME_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let runtime_re = RUNTIME_RE.get_or_init(|| {
        regex::Regex::new(
            r#"(Array\.isArray\(globalThis\.TURBOPACK\)\)return;var [\w$]+,[\w$]+=)"/_next/""#,
        )
        .expect("static regex")
    });
    js = runtime_re
        .replace_all(&js, |caps: &regex::Captures<'_>| {
            format!(r#"{}"{MOUNT}/_next/""#, &caps[1])
        })
        .into_owned();
    // removeBasePath is an identity function with the export defined a little
    // further on: `function a(e){return e}...Object.defineProperty(r,"removeBasePath"`.
    static REMOVE_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let remove_re = REMOVE_RE.get_or_init(|| {
        regex::Regex::new(r#"function ([\w$]+)\(([\w$]+)\)\{return ([\w$]+)\}([^;{}]*?Object\.defineProperty\([\w$]+,"__esModule",\{value:!0\}\),Object\.defineProperty\([\w$]+,"removeBasePath"|Object\.defineProperty\([^;]+?"removeBasePath")"#)
            .expect("static regex")
    });
    remove_re
        .replace_all(&js, |caps: &regex::Captures<'_>| {
            let name = &caps[1];
            let arg = &caps[2];
            format!(
                r#"function {name}({arg}){{return {arg}==="{MOUNT}"?"/":{arg}.startsWith("{MOUNT}/")?{arg}.slice({}):{arg}}}{}"#,
                MOUNT.len(),
                &caps[4]
            )
        })
        .into_owned()
}

/// Pages need a `<base>` so relative links resolve inside the mount.
fn ensure_base(html: &str, upstream_path: &str) -> String {
    static BASE_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let base_re = BASE_RE.get_or_init(|| regex::Regex::new(r"(?i)<base\b").expect("static regex"));
    if base_re.is_match(html) {
        return html.to_string();
    }
    let dir = match upstream_path.rfind('/') {
        Some(i) => &upstream_path[..=i],
        None => "/",
    };
    let tag = format!(r#"<base href="{MOUNT}{dir}">"#);
    static HEAD_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let head_re =
        HEAD_RE.get_or_init(|| regex::Regex::new(r"(?i)<head\b[^>]*>").expect("static regex"));
    match head_re.find(html) {
        Some(m) => format!("{}{}{}", &html[..m.end()], tag, &html[m.end()..]),
        None => format!("{tag}{html}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_resolve_to_the_site_or_an_allowlisted_api() {
        let t = resolve_target("/proxy/movies/", "").unwrap();
        assert_eq!(t.url, "https://www.movy.sx/");
        assert!(!t.is_api);
        let t = resolve_target("/proxy/movies/watch/movie/1", "?a=1").unwrap();
        assert_eq!(t.url, "https://www.movy.sx/watch/movie/1?a=1");
        let t = resolve_target("/proxy/movies/__api/db.wecollege.net/3/configuration", "").unwrap();
        assert_eq!(t.url, "https://db.wecollege.net/3/configuration");
        assert!(t.is_api);
        let t = resolve_target("/proxy/movies/__api/backend.movy.sx/v1/x", "?q=2").unwrap();
        assert_eq!(t.url, "https://backend.movy.sx/v1/x?q=2");
    }

    #[test]
    fn direct_api_hosts_come_from_the_env_and_only_from_the_allowlist() {
        std::env::set_var(
            "MOVIES_DIRECT_API_HOSTS",
            " api.wecollege.net , evil.example,TRAILERS.wecollege.net",
        );
        let hosts = direct_api_hosts();
        std::env::remove_var("MOVIES_DIRECT_API_HOSTS");
        assert_eq!(hosts, vec!["api.wecollege.net", "trailers.wecollege.net"]);
        assert!(direct_api_hosts().is_empty());
    }

    #[test]
    fn unlisted_hosts_and_traversal_are_refused() {
        assert!(resolve_target("/proxy/movies/__api/evil.example/x", "").is_none());
        assert!(
            resolve_target("/proxy/movies/__api/db.wecollege.net.evil.example/x", "").is_none()
        );
        assert!(resolve_target("/proxy/movies/__api/", "").is_none());
        assert!(resolve_target("/proxy/movies/../etc/passwd", "").is_none());
        assert!(resolve_target("/proxy/movies/a\\b", "").is_none());
        assert!(resolve_target("/other/path", "").is_none());
    }

    #[test]
    fn scoped_to_the_mount() {
        assert!(is_path("/proxy/movies"));
        assert!(is_path("/proxy/movies/watch"));
        assert!(is_path("/proxy/rivestream/watch"));
        for p in [
            "/",
            "/api/me",
            "/proxy/moviesx/",
            "/proxy/movies.evil/x",
            "/proxy/luma/",
        ] {
            assert!(!is_path(p), "{p}");
        }
    }

    #[test]
    fn interface_names_are_validated() {
        assert!(valid_interface_name("eth1"));
        assert!(valid_interface_name("wg-pia.0"));
        for bad in ["", "eth 1", "../eth1", "a/b", "waytoolonginterfacename"] {
            assert!(!valid_interface_name(bad), "{bad}");
        }
        assert!(!interface_exists("definitely-not-a-nic0"));
    }

    #[test]
    fn missing_interface_fails_closed() {
        std::env::set_var("MOVIES_EGRESS_INTERFACE", "definitely-not-a-nic0");
        let client = egress_client(reqwest::redirect::Policy::none());
        std::env::remove_var("MOVIES_EGRESS_INTERFACE");
        assert!(matches!(client, Err(EgressError::Unavailable)));
    }

    #[test]
    fn redirects_map_back_into_the_mount() {
        let base = "https://www.movy.sx/old";
        assert_eq!(
            mount_location("/new?x=1", base).as_deref(),
            Some("/proxy/movies/new?x=1")
        );
        assert_eq!(
            mount_location("https://www.movy.sx/a/b", base).as_deref(),
            Some("/proxy/movies/a/b")
        );
        assert!(mount_location("https://evil.example/x", base).is_none());
    }

    #[test]
    fn html_gets_prefix_base_runtime_and_asset_prefix() {
        let html = r#"<html><head><link rel="stylesheet" href="/_next/static/a.css"><link rel="preload" as="image" imageSrcSet="/img/a.webp 480w, https://www.movy.sx/img/b.webp 960w"></head><body><img src="/img/x.jpg"><a href="https://www.movy.sx/watch">w</a><script id="__NEXT_DATA__" type="application/json">{"page":"/","buildId":"b"}</script></body></html>"#;
        let out = rewrite_text(html, true, false);
        assert!(out.contains(r#"<base href="/proxy/movies/">"#));
        assert!(out.contains(r#"href="/proxy/movies/_next/static/a.css""#));
        assert!(out.contains(r#"src="/proxy/movies/img/x.jpg""#));
        assert!(out.contains(
            r#"imageSrcSet="/proxy/movies/img/a.webp 480w, /proxy/movies/img/b.webp 960w""#
        ));
        assert!(out.contains(r#"href="/proxy/movies/watch""#));
        assert!(out.contains("const prefix = '/proxy/movies'"));
        let data = out
            .split(r#"type="application/json">"#)
            .nth(1)
            .and_then(|s| s.split("</script>").next())
            .unwrap();
        let data: Value = serde_json::from_str(data).unwrap();
        assert_eq!(data["assetPrefix"], "/proxy/movies");
        assert_eq!(data["page"], "/");
    }

    #[test]
    fn router_base_path_helpers_are_patched() {
        let add =
            r#"function i(e,t){return(0,a.normalizePathTrailingSlash)((0,n.addPathPrefix)(e,""))}"#;
        assert!(rewrite_js(add).contains(r#"addPathPrefix)(e,"/proxy/movies")"#));
        let has = r#"(0,n.pathHasPrefix)(e,"")"#;
        assert!(rewrite_js(has).contains(r#"pathHasPrefix)(e,"/proxy/movies")"#));
        assert!(rewrite_js(r#"if(this.basePath="",this.sub=c)"#)
            .contains(r#"this.basePath="/proxy/movies""#));
        // movy.sx build: identity removeBasePath with other statements before the export.
        let remove = r#"function a(e){return e}e.i(50461),Object.defineProperty(r,"__esModule",{value:!0}),Object.defineProperty(r,"removeBasePath",{enumerable:!0,get:function(){return a}})"#;
        let out = rewrite_js(remove);
        assert!(out.contains(
            r#"return e==="/proxy/movies"?"/":e.startsWith("/proxy/movies/")?e.slice(13):e"#
        ));
        assert!(out.contains(r#""removeBasePath",{enumerable"#));
        // Turbopack runtime chunk base.
        let runtime = r#"(()=>{let e;if(!Array.isArray(globalThis.TURBOPACK))return;var t,r="/_next/",n=1})()"#;
        assert!(rewrite_js(runtime).contains(r#"var t,r="/proxy/movies/_next/",n=1"#));
        // Older Rivestream-style shape still works.
        let old =
            r#"function a(e){return e}Object.defineProperty(r,"removeBasePath",{enumerable:!0})"#;
        assert!(rewrite_js(old).contains("e.slice(13)"));
    }
}
