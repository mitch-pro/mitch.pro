//! Multi-tenant host identity + site config, ported from server.js
//! (`requestHost`, `isRjuhsdHost`, `isPickleHost`, `mitchSsoOrigins`, `site()`).

use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Clone)]
pub struct SiteConfig {
    pub base_dir: PathBuf,
    pub webroot: PathBuf,
    pub data_dir: PathBuf,
    /// site.json values with JS fallbacks ({primary, alternate, name}).
    pub primary: String,
    pub alternate: String,
}

impl SiteConfig {
    pub fn load() -> Self {
        let base_dir = std::env::var("MITCH_BASE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
        crate::env_file::load(&base_dir.join(".env"));
        let data_dir = std::env::var("DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| base_dir.join("data"));
        let webroot = base_dir.join("webserver");
        let (mut primary, mut alternate) = (
            "https://mitchdog.com".to_string(),
            "https://mitch.88chan.me".to_string(),
        );
        if let Ok(raw) = std::fs::read_to_string(data_dir.join("site.json")) {
            if let Ok(site) = serde_json::from_str::<Value>(&raw) {
                if let Some(p) = site.get("primary").and_then(|v| v.as_str()) {
                    primary = p.to_string();
                }
                if let Some(a) = site.get("alternate").and_then(|v| v.as_str()) {
                    alternate = a.to_string();
                }
            }
        }
        Self {
            base_dir,
            webroot,
            data_dir,
            primary,
            alternate,
        }
    }
}

/// `requestHost()`: X-Forwarded-Host (first comma value, trimmed) else Host
/// else the URL host. No port stripping, no lowercasing — parity with the JS.
pub fn request_host(headers: &axum::http::HeaderMap) -> String {
    if let Some(fwd) = headers
        .get("x-forwarded-host")
        .and_then(|v| v.to_str().ok())
    {
        return fwd.split(',').next().unwrap_or("").trim().to_string();
    }
    if let Some(host) = headers.get("host").and_then(|v| v.to_str().ok()) {
        return host.trim().to_string();
    }
    String::new()
}

/// `isRjuhsdHost()` / `isPickleHost()` — port-stripped, lowercased, exact or
/// subdomain match.
pub fn is_host(host: &str, domain: &str) -> bool {
    let lowered = host.to_lowercase();
    let h = lowered.split(':').next().unwrap_or("");
    h == domain || h.ends_with(&format!(".{domain}"))
}

pub fn is_rjuhsd_host(headers: &axum::http::HeaderMap) -> bool {
    is_host(&request_host(headers), "rjuhsd.school")
}

pub fn is_pickle_host(headers: &axum::http::HeaderMap) -> bool {
    is_host(&request_host(headers), "sexypickleclub.com")
}

/// `mitchSsoOrigins()`: https://mitch.pro plus site.json primary/alternate
/// origins (https only).
pub fn mitch_sso_origins(cfg: &SiteConfig) -> HashSet<String> {
    let mut origins = HashSet::new();
    origins.insert("https://mitch.pro".to_string());
    for raw in [&cfg.primary, &cfg.alternate] {
        if let Ok(u) = raw.parse::<url::Url>() {
            if u.scheme() == "https" {
                origins.insert(u.origin().ascii_serialization());
            }
        }
    }
    origins
}

/// `isMitchSsoHost`.
pub fn is_mitch_sso_host(cfg: &SiteConfig, hostname: &str) -> bool {
    let h = hostname.to_lowercase();
    for origin in mitch_sso_origins(cfg) {
        if let Ok(u) = origin.parse::<url::Url>() {
            if h == u.host_str().unwrap_or("").to_lowercase() {
                return true;
            }
        }
    }
    false
}

/// `sameOriginRequest()`: Origin or Referer hostname equals the request host
/// (port-stripped, lowercased), or both belong to the trusted Mitch SSO host set.
pub fn same_origin_request(headers: &axum::http::HeaderMap, cfg: Option<&SiteConfig>) -> bool {
    let host = request_host(headers);
    if host.is_empty() {
        return false;
    }
    let host_name = host.split(':').next().unwrap_or("").to_lowercase();
    for header_name in ["origin", "referer"] {
        if let Some(raw) = headers.get(header_name).and_then(|v| v.to_str().ok()) {
            if let Ok(u) = url::Url::parse(raw) {
                let target_host = u.host_str().unwrap_or("").to_lowercase();
                if target_host == host_name {
                    return true;
                }
                if let Some(c) = cfg {
                    if is_mitch_sso_host(c, &host_name) && is_mitch_sso_host(c, &target_host) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// `ssoBackAllowed()`: resolve `rawBack` against the request host origin
/// (exactly `new URL(rawBack, 'https://' + requestHost)`), require https and
/// an allowed host (pickle/mitch-SSO). rjuhsd.school is deliberately not a
/// valid bridge target: it only ever gets a local `/enroll/` account, never
/// a cross-domain SSO session bridged in from mitchdog.com.
pub fn sso_back_allowed(cfg: &SiteConfig, raw_back: &str, req_host: &str) -> Option<url::Url> {
    let host_name = req_host.split(':').next().unwrap_or("");
    let host_name = if host_name.is_empty() {
        "rjuhsd.school"
    } else {
        host_name
    };
    let base = url::Url::parse(&format!("https://{host_name}")).ok()?;
    let back = base.join(raw_back).ok()?;
    if back.scheme() != "https" {
        return None;
    }
    let h = back.host_str().unwrap_or("").to_lowercase();
    if is_host(&h, "sexypickleclub.com") || is_mitch_sso_host(cfg, &h) {
        return Some(back);
    }
    None
}
