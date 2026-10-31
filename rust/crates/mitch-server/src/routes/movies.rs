//! `/proxy/movies/` — the Movies page: a fixed-origin proxy for `https://cno.to`.
//!
//! Everything upstream (the site itself, its API, its HLS streams) is fetched
//! by this server, not by the visitor's browser, and every one of those
//! requests leaves through the VPN: via the egress sidecar (`MOVIES_EGRESS_PROXY`,
//! see `egress-proxy/`) or, where the VPN interface exists in this container,
//! by binding to it (see [`egress_interface`]).
//! The requests are made with the `curl` binary (in the production image), not
//! an in-process HTTP client: Cloudflare turns away reqwest's TLS handshake on
//! the site's stream endpoints (403) but accepts curl's, and curl can pin the
//! connection to a network interface. Requests always carry a fixed desktop
//! Chrome User-Agent (the stream endpoints also 403 `node`/headless ones).
//! If neither is configured/present the proxy answers 503 instead of quietly
//! falling back to the server's normal connection ("fail closed").
//!
//! What is proxied, and what is not:
//! - `/proxy/movies/<path>` goes to `https://cno.to/<path>`: GET/HEAD anywhere,
//!   POST under `/api/` (the site's playback pass and watch-state calls).
//! - `/proxy/movies/__api/<host>/<path>` goes to one host in [`API_HOSTS`] (an
//!   allowlist; this is never an open proxy), used for the stream mirror
//!   (`v.cno.to`) and the intro-skip API.
//! - Poster art (TMDB) and the player's segment CDNs are loaded by the visitor's
//!   browser directly; the CDN hosts rotate and aren't known ahead of time.
//!
//! Cookies: the site hands out a pass cookie (`__Host-sv`) that later stream
//! requests need. Upstream cookies are stored in the browser under a `mv_`
//! prefix scoped to this mount, and only cookies carrying that prefix are ever
//! sent upstream (prefix stripped). Mitch's own cookies and Authorization
//! headers never leave the server. Upstream CSP and frame headers are dropped.
//! DNS lookups use the container's normal resolver; only connections are bound.
//!
//! cno.to is a Vite/React single-page app built for the root of its origin.
//! HTML is rewritten to live under `/proxy/movies/` and its React Router is
//! given that mount as `basename` (see [`rewrite_js`]). Details and the eth1
//! requirements are in `docs/movies.md`.

#![allow(clippy::expect_used)] // the regexes below are static and known-valid

use crate::state::AppState;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::Response;
use std::sync::Arc;

const MOUNT: &str = "/proxy/movies";
const LEGACY_MOUNT: &str = "/proxy/rivestream";
const API_MOUNT: &str = "/proxy/movies/__api/";
const SITE_HOST: &str = "cno.to";
const SITE_ORIGIN: &str = "https://cno.to";
/// Other hosts the cno.to app calls from the browser. Anything else is refused.
const API_HOSTS: &[&str] = &["v.cno.to", "api.theintrodb.org"];
/// Upstream cookies live in the browser under this prefix (see module docs).
const COOKIE_PREFIX: &str = "mv_";
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

/// Interface names present in this container, for the log line that explains a 503.
fn available_interfaces() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir("/sys/class/net")
        .map(|dir| {
            dir.flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// How upstream connections are pinned to the VPN: through the egress sidecar
/// (`MOVIES_EGRESS_PROXY`, the production setup, since the VPN interface lives
/// on the host, not in this container) or by binding to a local interface.
#[derive(Clone, Debug, PartialEq)]
enum Egress {
    Proxy(String),
    Interface(String),
}

/// `MOVIES_EGRESS_PROXY` (e.g. `http://host.docker.internal:3128`): an HTTP
/// CONNECT proxy that dials out over the VPN. Must be plain `http://host[:port]`.
fn egress_proxy() -> Option<String> {
    let value = std::env::var("MOVIES_EGRESS_PROXY").ok()?;
    let value = value.trim();
    let rest = value.strip_prefix("http://")?;
    let ok = !rest.is_empty()
        && rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_'));
    ok.then(|| value.to_string())
}

/// Why a request couldn't be started.
enum EgressError {
    /// The configured interface isn't there (or the name is invalid).
    Unavailable,
}

/// The interface to bind to, or `Unavailable` (fail closed) if it's missing.
fn egress_ready() -> Result<Egress, EgressError> {
    if let Some(proxy) = egress_proxy() {
        return Ok(Egress::Proxy(proxy));
    }
    if std::env::var("MOVIES_EGRESS_PROXY").is_ok_and(|v| !v.trim().is_empty()) {
        eprintln!("[movies] MOVIES_EGRESS_PROXY must look like http://host:port; refusing to go out the normal connection");
        return Err(EgressError::Unavailable);
    }
    let interface = egress_interface();
    if interface_exists(&interface) {
        return Ok(Egress::Interface(interface));
    }
    // Say why, once per request, so a missing eth1 is obvious in the logs.
    eprintln!(
        "[movies] egress interface {interface:?} not found in this container (present: {}); refusing to go out the normal connection",
        available_interfaces().join(", ")
    );
    Err(EgressError::Unavailable)
}

// ── curl ────────────────────────────────────────────────────────────────────

/// A header value that can't break out of its line.
fn header_value_ok(value: &str) -> bool {
    !value.contains(['\r', '\n', '\0'])
}

/// The curl command line for one upstream request. Redirects are not followed
/// (the handler maps them), `--globoff` stops `{}`/`[]` in paths being treated
/// as ranges, and `--proto =https` refuses anything but HTTPS.
fn curl_args(
    egress: &Egress,
    method: &Method,
    url: &str,
    headers: &[(String, String)],
    has_body: bool,
) -> Vec<String> {
    let mut args: Vec<String> = [
        "--silent",
        "--show-error",
        "--globoff",
        "--include",
        "--http2",
        "--proto",
        "=https",
        "--connect-timeout",
        "10",
        // Give up on a stalled transfer (under 1 byte/s for 30s), but let a
        // healthy video segment take as long as it needs.
        "--speed-limit",
        "1",
        "--speed-time",
        "30",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    match egress {
        Egress::Interface(interface) => {
            args.push("--interface".into());
            args.push(interface.clone());
        }
        Egress::Proxy(proxy) => {
            args.push("--proxy".into());
            args.push(proxy.clone());
        }
    }
    if *method == Method::HEAD {
        args.push("--head".into());
    } else if *method == Method::POST {
        args.push("--request".into());
        args.push("POST".into());
        if has_body {
            args.push("--data-binary".into());
            args.push("@-".into());
        }
    }
    for (name, value) in headers {
        if header_value_ok(name) && header_value_ok(value) && !name.contains(':') {
            args.push("--header".into());
            args.push(format!("{name}: {value}"));
        }
    }
    args.push("--url".into());
    args.push(url.to_string());
    args
}

struct UpstreamHead {
    status: u16,
    /// Header names are lowercased; repeated headers (Set-Cookie) repeat.
    headers: Vec<(String, String)>,
}

impl UpstreamHead {
    fn get(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Parses one header block as printed by `curl --include` (`HTTP/2 200` or
/// `HTTP/1.1 200 OK`, then `name: value` lines).
fn parse_head(block: &str) -> Option<UpstreamHead> {
    let mut lines = block.lines();
    let status = lines
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse::<u16>()
        .ok()?;
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    Some(UpstreamHead { status, headers })
}

type BodyReader = tokio::io::BufReader<tokio::process::ChildStdout>;

struct Upstream {
    head: UpstreamHead,
    reader: BodyReader,
    child: tokio::process::Child,
}

impl Upstream {
    /// The whole body (for the pages that get rewritten).
    async fn text(mut self) -> String {
        use tokio::io::AsyncReadExt;
        let mut bytes = Vec::new();
        let _ = self.reader.read_to_end(&mut bytes).await;
        let _ = self.child.wait().await;
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// The body as it arrives, for images, playlists and video segments. The
    /// stream owns the curl process, so it is killed (`kill_on_drop`) if the
    /// visitor goes away before the download finishes.
    fn into_stream(self) -> axum::body::Body {
        use tokio::io::AsyncReadExt;
        let Upstream { reader, child, .. } = self;
        let stream =
            futures_util::stream::unfold((reader, child), |(mut reader, child)| async move {
                let mut buf = vec![0u8; 64 * 1024];
                match reader.read(&mut buf).await {
                    Ok(0) | Err(_) => None,
                    Ok(n) => {
                        buf.truncate(n);
                        Some((
                            Ok::<_, std::io::Error>(axum::body::Bytes::from(buf)),
                            (reader, child),
                        ))
                    }
                }
            });
        axum::body::Body::from_stream(stream)
    }
}

async fn curl_fetch(
    egress: &Egress,
    method: &Method,
    url: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<Upstream, String> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
    let has_body = *method == Method::POST;
    let mut command = tokio::process::Command::new("curl");
    command
        .args(curl_args(egress, method, url, headers, has_body))
        .stdin(if has_body {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|e| format!("could not start curl: {e}"))?;
    if has_body {
        if let Some(mut stdin) = child.stdin.take() {
            let payload = body.to_vec();
            tokio::spawn(async move {
                let _ = stdin.write_all(&payload).await;
            });
        }
    }
    let stdout = child.stdout.take().ok_or("curl gave no output")?;
    let mut reader = tokio::io::BufReader::new(stdout);

    // Read header blocks until a final (non-1xx) status.
    loop {
        let mut block = String::new();
        loop {
            let mut line = Vec::new();
            let n = reader
                .read_until(b'\n', &mut line)
                .await
                .map_err(|e| e.to_string())?;
            if n == 0 {
                let mut err = String::new();
                if let Some(stderr) = child.stderr.take() {
                    let mut raw = Vec::new();
                    let _ = stderr.take(400).read_to_end(&mut raw).await;
                    err = String::from_utf8_lossy(&raw).trim().to_string();
                }
                return Err(format!("curl ended before sending headers: {err}"));
            }
            let text = String::from_utf8_lossy(&line).into_owned();
            if text.trim_end_matches(['\r', '\n']).is_empty() {
                break;
            }
            block.push_str(&text);
        }
        let head = parse_head(&block).ok_or("unreadable response headers")?;
        if head.status >= 200 {
            return Ok(Upstream {
                head,
                reader,
                child,
            });
        }
    }
}

// ── target resolution ───────────────────────────────────────────────────────

struct Target {
    /// Full upstream URL, query included.
    url: String,
    /// Host the response should be attributed to (for the redirect policy).
    host: &'static str,
    is_api: bool,
    /// Path on the upstream host, used to decide if POST is allowed.
    upstream_path: String,
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
            upstream_path: format!("/{tail}"),
        });
    }
    let site_path = if rest.is_empty() { "/" } else { rest };
    Some(Target {
        url: format!("{SITE_ORIGIN}{site_path}{query}"),
        host: SITE_HOST,
        is_api: false,
        upstream_path: site_path.to_string(),
    })
}

/// GET/HEAD anywhere; POST only to an `/api/` path on the site, or to a
/// listed API host.
fn method_allowed(method: &Method, target: &Target) -> bool {
    match *method {
        Method::GET | Method::HEAD => true,
        Method::POST => target.is_api || target.upstream_path.starts_with("/api/"),
        _ => false,
    }
}

// ── cookies ─────────────────────────────────────────────────────────────────

/// Turns an upstream `Set-Cookie` into one scoped to this mount under
/// [`COOKIE_PREFIX`]. The name keeps its original form after the prefix, so a
/// `__Host-` cookie (which can't have a non-root path) becomes an ordinary one.
fn rewrite_set_cookie(value: &str) -> Option<String> {
    let mut parts = value.split(';').map(str::trim);
    let (name, val) = parts.next()?.split_once('=')?;
    if name.is_empty() {
        return None;
    }
    let mut out = format!("{COOKIE_PREFIX}{name}={val}; Path={MOUNT}; SameSite=Lax");
    for attr in parts {
        let lower = attr.to_ascii_lowercase();
        if lower.starts_with("max-age=") || lower.starts_with("expires=") {
            out.push_str("; ");
            out.push_str(attr);
        } else if lower == "httponly" {
            out.push_str("; HttpOnly");
        } else if lower == "secure" {
            out.push_str("; Secure");
        }
    }
    Some(out)
}

/// The cookies to send upstream: only the ones this proxy stored, prefix removed.
fn upstream_cookie_header(browser_cookies: &str) -> Option<String> {
    let kept: Vec<String> = browser_cookies
        .split(';')
        .map(str::trim)
        .filter_map(|pair| pair.strip_prefix(COOKIE_PREFIX))
        .filter(|pair| pair.contains('='))
        .map(str::to_string)
        .collect();
    (!kept.is_empty()).then(|| kept.join("; "))
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
    if !method_allowed(method, &target) {
        return Some(crate::errors::err_resp(
            405,
            Some("method not allowed"),
            None,
        ));
    }
    if body.len() > MAX_REQUEST_BODY {
        return Some(crate::errors::err_resp(413, None, None));
    }

    let egress = match egress_ready() {
        Ok(egress) => egress,
        Err(EgressError::Unavailable) => {
            return Some(text_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Movies are unavailable right now.",
            ))
        }
    };

    let mut upstream_headers: Vec<(String, String)> = Vec::new();
    for (name, value) in headers.iter() {
        let n = name.as_str().to_ascii_lowercase();
        // The visitor's own User-Agent is deliberately not forwarded: the site's
        // Cloudflare rules reject unusual ones (node, headless browsers), and
        // there's no reason to hand a visitor's browser details upstream.
        let forward = matches!(n.as_str(), "accept" | "accept-language" | "range")
            || (*method == Method::POST && n == "content-type");
        if forward {
            if let Ok(v) = value.to_str() {
                upstream_headers.push((n, v.to_string()));
            }
        }
    }
    upstream_headers.push(("user-agent".into(), USER_AGENT.into()));
    upstream_headers.push(("referer".into(), format!("{SITE_ORIGIN}/")));
    if *method == Method::POST {
        upstream_headers.push(("origin".into(), SITE_ORIGIN.into()));
    }
    // Upstream cookies are only meaningful for the site itself.
    if !target.is_api {
        let cookie = headers
            .get(axum::http::header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(upstream_cookie_header);
        if let Some(cookie) = cookie {
            upstream_headers.push(("cookie".into(), cookie));
        }
    }

    let upstream = match curl_fetch(&egress, method, &target.url, &upstream_headers, body).await {
        Ok(upstream) => upstream,
        Err(err) => {
            // Which host failed and why (never the full URL or any visitor data).
            eprintln!("[movies] request to {} failed: {err}", target.host);
            return Some(upstream_error());
        }
    };
    let status = StatusCode::from_u16(upstream.head.status).unwrap_or(StatusCode::BAD_GATEWAY);
    if status == StatusCode::FORBIDDEN {
        // Cloudflare turns away clients it doesn't like; the blocking ray id is
        // what a bug report needs.
        eprintln!(
            "[movies] {} answered 403 (cf-ray {})",
            target.host,
            upstream.head.get("cf-ray").unwrap_or("-")
        );
    }
    let mut builder = Response::builder().status(status);

    let content_type = upstream
        .head
        .get("content-type")
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let is_html = content_type.contains("text/html");
    let is_text =
        is_html || content_type.contains("text/css") || content_type.contains("javascript");
    let rewritten = is_text && *method != Method::HEAD;

    for (name, value) in &upstream.head.headers {
        let drop = matches!(
            name.as_str(),
            "set-cookie"
                | "content-security-policy"
                | "content-security-policy-report-only"
                | "x-frame-options"
                | "cross-origin-opener-policy"
                | "cross-origin-embedder-policy"
                | "content-encoding"
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
        ) || (name == "content-length" && rewritten);
        if !drop {
            builder = builder.header(name.as_str(), value.as_str());
        }
    }
    if !target.is_api {
        for (name, value) in &upstream.head.headers {
            if name == "set-cookie" {
                if let Some(cookie) = rewrite_set_cookie(value) {
                    builder = builder.header("set-cookie", cookie);
                }
            }
        }
    }
    if status.is_redirection() {
        // A redirect within the host maps back into the mount; one to another
        // HTTPS host (an image CDN, say) is passed on for the browser to follow.
        if let Some(location) = upstream
            .head
            .get("location")
            .and_then(|l| mount_location(l, &target.url))
        {
            builder = builder.header("location", location);
        }
    }
    builder = builder.header(
        "cache-control",
        if is_html || target.is_api || *method == Method::POST {
            "private, no-store"
        } else {
            "public, max-age=3600"
        },
    );
    if is_html {
        builder = builder.header(
            "content-security-policy",
            "default-src 'self' https: blob: data:; script-src 'self' 'unsafe-inline' 'unsafe-eval' blob:; style-src 'self' 'unsafe-inline' https:; object-src 'none'; worker-src 'self' blob:; frame-ancestors 'self'",
        );
    }

    if *method == Method::HEAD {
        return Some(
            builder
                .body(axum::body::Body::empty())
                .unwrap_or_else(|_| crate::errors::err_resp(500, None, None)),
        );
    }
    if !rewritten {
        // Images, video segments, playlists and API JSON: pass the bytes through
        // as they arrive instead of holding a whole segment in memory.
        return Some(
            builder
                .body(upstream.into_stream())
                .unwrap_or_else(|_| crate::errors::err_resp(500, None, None)),
        );
    }

    let mut content = upstream.text().await;
    content = rewrite_text(&content, is_html, content_type.contains("javascript"));
    if is_html {
        let page_path = target.upstream_path.split('?').next().unwrap_or("/");
        content = ensure_base(&content, page_path);
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

/// Map an upstream `Location`: back into the mount if it stays on the same
/// host, unchanged if it goes to another HTTPS host, `None` otherwise.
fn mount_location(location: &str, request_url: &str) -> Option<String> {
    let base = url::Url::parse(request_url).ok()?;
    let resolved = base.join(location).ok()?;
    if resolved.scheme() != "https" {
        return None;
    }
    if resolved.host_str() != base.host_str() {
        return Some(resolved.to_string());
    }
    let prefix = if base.host_str() == Some(SITE_HOST) {
        MOUNT.to_string()
    } else {
        format!("{MOUNT}/__api/{}", base.host_str()?)
    };
    let mut out = format!("{prefix}{}", resolved.path());
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
    let mut html = rewrite_srcset(html);
    // Cloudflare's edge injects a bot-detection loader that fetches from
    // /cdn-cgi/ on the page's own origin; it has no purpose here.
    static CF_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let cf_re = CF_RE.get_or_init(|| {
        regex::Regex::new(
            r"(?s)<script>\(function\(\)\{function c\(\)\{var b=a\.contentDocument.*?</script>",
        )
        .expect("static regex")
    });
    html = cf_re.replace_all(&html, "").into_owned();
    let runtime = include_str!("movies.js");
    html.replacen(
        "<head>",
        &format!(r#"<head><base href="{MOUNT}/"><script>{runtime}</script>"#),
        1,
    )
}

/// cno.to's React Router is created without a `basename`, so under the mount it
/// would see `/proxy/movies/...` as an unknown route. React Router's
/// `createBrowserRouter(routes, opts)` forwards `opts.basename`; default it to
/// the mount.
fn rewrite_js(js: &str) -> String {
    static ROUTER_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let router_re = ROUTER_RE.get_or_init(|| {
        regex::Regex::new(r"\{basename:([\w$]+)\?\.basename,getContext:").expect("static regex")
    });
    router_re
        .replace_all(js, |caps: &regex::Captures<'_>| {
            format!(
                r#"{{basename:{}?.basename??"{MOUNT}",getContext:"#,
                &caps[1]
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
        assert_eq!(t.url, "https://cno.to/");
        assert!(!t.is_api);
        let t = resolve_target("/proxy/movies/movie/1-toy-story", "?a=1").unwrap();
        assert_eq!(t.url, "https://cno.to/movie/1-toy-story?a=1");
        let t = resolve_target("/proxy/movies/__api/v.cno.to/hls/x.m3u8", "").unwrap();
        assert_eq!(t.url, "https://v.cno.to/hls/x.m3u8");
        assert!(t.is_api);
        let t = resolve_target("/proxy/movies/__api/api.theintrodb.org/v3/media", "?q=2").unwrap();
        assert_eq!(t.url, "https://api.theintrodb.org/v3/media?q=2");
    }

    #[test]
    fn unlisted_hosts_and_traversal_are_refused() {
        assert!(resolve_target("/proxy/movies/__api/evil.example/x", "").is_none());
        assert!(resolve_target("/proxy/movies/__api/v.cno.to.evil.example/x", "").is_none());
        assert!(resolve_target("/proxy/movies/__api/", "").is_none());
        assert!(resolve_target("/proxy/movies/../etc/passwd", "").is_none());
        assert!(resolve_target("/proxy/movies/a\\b", "").is_none());
        assert!(resolve_target("/other/path", "").is_none());
    }

    #[test]
    fn methods_are_limited() {
        let page = resolve_target("/proxy/movies/movie/1", "").unwrap();
        let api = resolve_target("/proxy/movies/api/p", "").unwrap();
        let host = resolve_target("/proxy/movies/__api/api.theintrodb.org/v3/x", "").unwrap();
        assert!(method_allowed(&Method::GET, &page));
        assert!(method_allowed(&Method::HEAD, &page));
        assert!(!method_allowed(&Method::POST, &page));
        assert!(method_allowed(&Method::POST, &api));
        assert!(method_allowed(&Method::POST, &host));
        for m in [Method::PUT, Method::DELETE, Method::PATCH] {
            assert!(!method_allowed(&m, &api));
        }
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

    fn eth1() -> Egress {
        Egress::Interface("eth1".into())
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
    fn egress_settings_are_validated_and_fail_closed() {
        // One test: these mutate process-wide env vars.
        std::env::set_var("MOVIES_EGRESS_INTERFACE", "definitely-not-a-nic0");
        let missing = egress_ready();
        std::env::remove_var("MOVIES_EGRESS_INTERFACE");
        assert!(matches!(missing, Err(EgressError::Unavailable)));
        std::env::set_var("MOVIES_EGRESS_PROXY", "http://host.docker.internal:3128");
        let ok = egress_ready();
        std::env::set_var("MOVIES_EGRESS_PROXY", "socks5://evil");
        let bad = egress_ready();
        std::env::remove_var("MOVIES_EGRESS_PROXY");
        assert!(matches!(ok, Ok(Egress::Proxy(p)) if p == "http://host.docker.internal:3128"));
        assert!(matches!(bad, Err(EgressError::Unavailable)));
        let args = curl_args(
            &Egress::Proxy("http://h:3128".into()),
            &Method::GET,
            "https://cno.to/",
            &[],
            false,
        );
        let joined = args.join(" ");
        assert!(joined.contains("--proxy http://h:3128") && !joined.contains("--interface"));
    }

    #[test]
    fn curl_is_pinned_to_the_interface_and_locked_down() {
        let args = curl_args(
            &eth1(),
            &Method::GET,
            "https://cno.to/api/s/1?x={y}",
            &[
                ("accept".into(), "*/*".into()),
                ("cookie".into(), "sv=1".into()),
            ],
            false,
        );
        let joined = args.join(" ");
        assert!(joined.contains("--interface eth1"));
        assert!(joined.contains("--proto =https"));
        assert!(joined.contains("--http2") && joined.contains("--globoff"));
        assert!(
            joined.contains("--header accept: */*") && joined.contains("--header cookie: sv=1")
        );
        assert!(args.ends_with(&["--url".into(), "https://cno.to/api/s/1?x={y}".into()]));
        // Never follows redirects or reads config/header files.
        assert!(!args
            .iter()
            .any(|a| a == "--location" || a == "-L" || a == "--config" || a == "-K"));
        assert!(!args.contains(&"--data-binary".to_string()));
        let post = curl_args(&eth1(), &Method::POST, "https://cno.to/api/p", &[], true);
        assert!(post.join(" ").contains("--request POST --data-binary @-"));
        assert!(
            curl_args(&eth1(), &Method::HEAD, "https://cno.to/", &[], false)
                .contains(&"--head".to_string())
        );
    }

    #[test]
    fn header_values_cannot_inject_extra_headers() {
        assert!(header_value_ok("text/html"));
        assert!(!header_value_ok("a\r\nX-Evil: 1"));
        assert!(!header_value_ok("a\nb"));
        let args = curl_args(
            &eth1(),
            &Method::GET,
            "https://cno.to/",
            &[
                ("accept".into(), "ok\r\nX-Evil: 1".into()),
                ("bad:name".into(), "v".into()),
            ],
            false,
        );
        assert!(!args
            .iter()
            .any(|a| a.contains("X-Evil") || a.contains("bad:name")));
    }

    #[test]
    fn response_headers_are_parsed_for_http2_and_http1() {
        let h2 = parse_head("HTTP/2 200 \r\ncontent-type: application/json\r\nSet-Cookie: a=1\r\nset-cookie: b=2\r\n").unwrap();
        assert_eq!(h2.status, 200);
        assert_eq!(h2.get("content-type"), Some("application/json"));
        assert_eq!(
            h2.headers.iter().filter(|(n, _)| n == "set-cookie").count(),
            2
        );
        let h1 = parse_head("HTTP/1.1 302 Found\r\nLocation: /x\r\n").unwrap();
        assert_eq!((h1.status, h1.get("location")), (302, Some("/x")));
        assert!(parse_head("garbage").is_none());
        assert!(parse_head("").is_none());
    }

    #[test]
    fn redirects_map_back_into_the_mount() {
        let base = "https://cno.to/old";
        assert_eq!(
            mount_location("/new?x=1", base).as_deref(),
            Some("/proxy/movies/new?x=1")
        );
        assert_eq!(
            mount_location("https://cno.to/a/b", base).as_deref(),
            Some("/proxy/movies/a/b")
        );
        // Another HTTPS host (an image CDN) is for the browser to follow itself.
        assert_eq!(
            mount_location("https://i.ytimg.com/vi/x.jpg", base).as_deref(),
            Some("https://i.ytimg.com/vi/x.jpg")
        );
        assert!(mount_location("http://cno.to/x", base).is_none());
        assert!(mount_location("javascript:alert(1)", base).is_none());
        assert_eq!(
            mount_location("/y", "https://v.cno.to/hls/x").as_deref(),
            Some("/proxy/movies/__api/v.cno.to/y")
        );
    }

    #[test]
    fn upstream_cookies_are_namespaced_both_ways() {
        let set = rewrite_set_cookie(
            "__Host-sv=abc.def; Max-Age=21600; Path=/; Expires=Sat, 31 Oct 2026 19:37:03 GMT; HttpOnly; Secure; SameSite=Strict; Domain=cno.to",
        )
        .unwrap();
        assert!(set.starts_with("mv___Host-sv=abc.def; Path=/proxy/movies; SameSite=Lax"));
        assert!(
            set.contains("Max-Age=21600") && set.contains("HttpOnly") && set.contains("Secure")
        );
        assert!(!set.contains("Domain") && !set.contains("Strict"));
        assert!(rewrite_set_cookie("=oops").is_none());
        assert!(rewrite_set_cookie("novalue").is_none());
        // Only proxy-made cookies go upstream; Mitch's own never do.
        let sent = upstream_cookie_header(
            "mitch_session=SECRET; studentId=abc; mv___Host-sv=abc.def; theme=dark; mv_x=1",
        )
        .unwrap();
        assert_eq!(sent, "__Host-sv=abc.def; x=1");
        assert!(!sent.contains("SECRET") && !sent.contains("studentId"));
        assert!(upstream_cookie_header("mitch_session=SECRET; studentId=abc").is_none());
    }

    #[test]
    fn html_gets_prefix_base_runtime_and_no_cloudflare_loader() {
        let html = r#"<html><head><link rel="stylesheet" href="/assets/a.css"><link rel="preload" as="image" imageSrcSet="/img/a.webp 480w, https://cno.to/img/b.webp 960w"></head><body><img src="/img/x.jpg"><a href="https://cno.to/movie/1">m</a><script type="module" src="/assets/index-1.js"></script><script>(function(){function c(){var b=a.contentDocument||x;}c()})();</script></body></html>"#;
        let out = rewrite_text(html, true, false);
        assert!(out.contains(r#"<base href="/proxy/movies/">"#));
        assert!(out.contains(r#"href="/proxy/movies/assets/a.css""#));
        assert!(out.contains(r#"src="/proxy/movies/img/x.jpg""#));
        assert!(out.contains(r#"src="/proxy/movies/assets/index-1.js""#));
        assert!(out.contains(
            r#"imageSrcSet="/proxy/movies/img/a.webp 480w, /proxy/movies/img/b.webp 960w""#
        ));
        assert!(out.contains(r#"href="/proxy/movies/movie/1""#));
        assert!(out.contains("const prefix = '/proxy/movies'"));
        assert!(
            !out.contains("contentDocument"),
            "Cloudflare loader should be stripped"
        );
        assert!(
            !out.contains("/proxy/movies/proxy/movies"),
            "no double prefix"
        );
    }

    #[test]
    fn react_router_gets_the_mount_as_its_default_basename() {
        let js = r#"function $a(e,t){return Kn({basename:t?.basename,getContext:t?.getContext,future:t?.future,history:at({window:t?.window})}).initialize()}"#;
        let out = rewrite_js(js);
        assert!(out.contains(r#"{basename:t?.basename??"/proxy/movies",getContext:t?.getContext"#));
        // Unrelated code is untouched.
        assert_eq!(rewrite_js("let a=1;"), "let a=1;");
    }
}
