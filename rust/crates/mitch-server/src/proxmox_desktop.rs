//! Proxmox desktop service — port of `lib/proxmox_desktop.js` (the graphical
//! VM "Computer" backend behind `/api/vm/*`).
//!
//! Parity contract:
//! - The service is a process-wide singleton built from the environment
//!   (`ProxmoxDesktopService.fromEnv(...)` in the JS) → [`desktop`].
//! - Every call maps failures to [`ProxmoxServiceError`] with the JS code /
//!   status pair (401/403 upstream → 503, timeouts → 504, network → 502), so
//!   route bodies can render the exact JS error responses.
//! - `tls: { rejectUnauthorized }` maps to `danger_accept_invalid_certs`.
//!   The optional `PROXMOX_TLS_SERVERNAME` SNI override is carried in
//!   [`ProxmoxDesktopService::tls_server_name`] (and exposed in
//!   `createConsole`'s `tlsOptions`), but reqwest has no per-request SNI
//!   override — the env var is unset in every environment we deploy.
//! - `redirect: 'error'` → a redirect response aborts with UNREACHABLE, not
//!   a follow.
//! - The four module-level caches (`guestIpCache`, `guestHostnameCache`,
//!   `guestAgentLastAttempt`, `optimizedVmids`) live behind one shared
//!   `Arc<Mutex<Caches>>` so `Clone` (used for the detached `setTimeout` /
//!   `void`-style tasks) keeps mutating the same state.
//! - `cloneDesktop`/`updateHardware` take numbers that are already
//!   `Number()`-coerced by the caller (the JS destructures then coerces at
//!   the clamp sites; see [`js_num_or`]/[`jsval::number`]).

// The service is only reachable from routes/vm.rs, which this batch is still
// extending — drop this allow once the last call site lands.
#![allow(dead_code)]

use mitch_lib::auth::encode_uri_component;
use mitch_lib::jsval;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::error::Error as _;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const DEFAULT_TIMEOUT_MS: u64 = 12_000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn unreachable_regex() -> Regex {
    Regex::new("$^").unwrap_or_else(|_| unreachable!())
}

/// JS `ProxmoxServiceError` — `code` / `message` / HTTP `status`.
#[derive(Debug, Clone)]
pub struct ProxmoxServiceError {
    pub code: &'static str,
    pub message: String,
    pub status: u16,
}

impl ProxmoxServiceError {
    pub fn new(code: &'static str, message: impl Into<String>, status: u16) -> Self {
        Self {
            code,
            message: message.into(),
            status,
        }
    }
}

impl std::fmt::Display for ProxmoxServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {} ({})", self.code, self.message, self.status)
    }
}

impl std::error::Error for ProxmoxServiceError {}

/// `Number(v || fallback)` — a truthy non-numeric value stays NaN (unlike
/// `Number(v) || fallback`, which maps NaN onto the fallback).
fn js_num_or(v: Option<&Value>, fallback: f64) -> f64 {
    match v {
        Some(raw) if jsval::truthy(raw) => jsval::number(raw).unwrap_or(f64::NAN),
        _ => fallback,
    }
}

/// `Number(a || b || 0)` — the first truthy operand coerces.
fn js_num_chain(vals: &[Option<&Value>], fallback: f64) -> f64 {
    for raw in vals.iter().flatten() {
        if jsval::truthy(raw) {
            return jsval::number(raw).unwrap_or(f64::NAN);
        }
    }
    fallback
}

/// `Math.max(lo, Math.min(x, hi))` — but NaN-propagating like the JS
/// builtins (Rust's `f64::min`/`f64::max` silently drop NaN).
fn clamp_js(x: f64, lo: f64, hi: f64) -> f64 {
    if x.is_nan() {
        f64::NAN
    } else {
        x.min(hi).max(lo)
    }
}

/// A JS number rendered for a request form — NaN stringifies to `"NaN"`
/// (serde would render null, which is `String(null)` instead).
fn js_num_value(n: f64) -> Value {
    if n.is_nan() {
        json!("NaN")
    } else {
        jsval::num_value(n)
    }
}

/// `parseBoolean(value, fallback)` — `''`/null fall back; everything else is
/// false only for the deny list.
fn parse_boolean(value: Option<&str>, fallback: bool) -> bool {
    match value {
        None => fallback,
        Some("") => fallback,
        Some(raw) => !["0", "false", "no", "off"].contains(&raw.trim().to_lowercase().as_str()),
    }
}

/// `normalizedApiBase(host, port)`.
fn normalized_api_base(host: &str, port: f64) -> String {
    let raw = host.trim();
    if raw.is_empty() {
        return String::new();
    }
    let lower = raw.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        let trimmed = raw.trim_end_matches('/');
        // JS: `.replace(/\/+$/, '').replace(/\/api2\/json$/i, '')`.
        let without_api = trimmed
            .strip_suffix("/api2/json")
            .or_else(|| trimmed.strip_suffix("/API2/JSON"))
            .unwrap_or(trimmed);
        return format!("{}/api2/json", without_api);
    }
    let port_num = if port.is_nan() { 8006.0 } else { port };
    format!(
        "https://{}:{}/api2/json",
        raw.trim_end_matches('/'),
        jsval::num_value(port_num)
    )
}

/// `valueFromLegacyUrl(rawUrl)` — WHATWG `new URL` + hostname/port pull.
fn value_from_legacy_url(raw_url: &str) -> (String, f64) {
    match url::Url::parse(raw_url.trim()) {
        Ok(parsed) => {
            let host = parsed.host_str().unwrap_or("").to_string();
            let port = parsed.port().map(|p| p as f64).unwrap_or(8006.0);
            (host, port)
        }
        Err(_) => (String::new(), 8006.0),
    }
}

/// `parseDiskGb(config)` — the first of `scsi0`/`virtio0`/`sata0` carrying a
/// `size=` wins.
fn parse_disk_gb(config: &Value) -> f64 {
    static SIZE_RE: OnceLock<Regex> = OnceLock::new();
    let size_re = SIZE_RE.get_or_init(|| {
        Regex::new(r"(?:^|,)size=(\d+(?:\.\d+)?)([TGMK])").unwrap_or_else(|_| unreachable_regex())
    });
    for key in ["scsi0", "virtio0", "sata0"] {
        let value = jsval::str_or(config.get(key), "");
        if let Some(caps) = size_re.captures(&value) {
            let number = caps
                .get(1)
                .and_then(|m| m.as_str().parse::<f64>().ok())
                .unwrap_or(f64::NAN);
            let unit = caps
                .get(2)
                .map(|m| m.as_str().to_uppercase())
                .unwrap_or_default();
            if unit == "T" {
                return (number * 1024.0).round();
            }
            if unit == "G" {
                return number.round();
            }
            if unit == "M" {
                return 1.0f64.max((number / 1024.0).round());
            }
            return 1.0f64.max((number / 1024.0 / 1024.0).round());
        }
    }
    0.0
}

/// `firstPrivateIpv4(agentResult)`.
fn first_private_ipv4(agent_result: &Value) -> String {
    let result_field = &agent_result["result"];
    let chosen: &Value = if jsval::truthy(result_field) {
        result_field
    } else {
        agent_result
    };
    let empty: [Value; 0] = [];
    let interfaces: &[Value] = match chosen {
        Value::Array(items) => items.as_slice(),
        _ => &empty,
    };
    for iface in interfaces {
        let addresses: &[Value] = match iface.get("ip-addresses") {
            Some(Value::Array(items)) => items.as_slice(),
            _ => &empty,
        };
        for address in addresses {
            let value = jsval::str_or(address.get("ip-address"), "");
            if address.get("ip-address-type").and_then(Value::as_str) == Some("ipv4")
                && !value.is_empty()
                && !value.starts_with("127.")
                && value != "0.0.0.0"
            {
                return value;
            }
        }
    }
    String::new()
}

/// `cloneDesktop`'s `cleanHostname` ladder.
fn clean_hostname(hostname: &str, vmid: i64) -> String {
    static BAD_RE: OnceLock<Regex> = OnceLock::new();
    static DASH_RE: OnceLock<Regex> = OnceLock::new();
    let bad_re =
        BAD_RE.get_or_init(|| Regex::new(r"[^a-z0-9-]").unwrap_or_else(|_| unreachable_regex()));
    let dash_re = DASH_RE.get_or_init(|| Regex::new(r"-+").unwrap_or_else(|_| unreachable_regex()));
    let fallback = format!("computer-{}", vmid);
    let base = if hostname.is_empty() {
        fallback.clone()
    } else {
        hostname.to_string()
    };
    let substituted = bad_re.replace_all(&base.to_lowercase(), "-").to_string();
    let collapsed = dash_re.replace_all(&substituted, "-").to_string();
    let sliced = jsval::js_slice_utf16(collapsed.trim_matches('-'), 48);
    if sliced.is_empty() {
        fallback
    } else {
        sliced
    }
}

/// The four module-level caches, behind one shared lock.
#[derive(Default)]
struct Caches {
    guest_ip: HashMap<i64, String>,
    guest_hostname: HashMap<i64, String>,
    guest_agent_last_attempt: HashMap<i64, u64>,
    optimized_vmids: HashSet<i64>,
}

/// `ProxmoxDesktopService` — shared state via `Arc<Mutex<Caches>>` so the
/// detached background tasks spawned by [`ProxmoxDesktopService::power`] /
/// [`ProxmoxDesktopService::clone_desktop`] keep mutating the same state.
#[derive(Clone)]
pub struct ProxmoxDesktopService {
    host: String,
    port: i64,
    node: String,
    base_url: String,
    authorization: String,
    verify_tls: bool,
    tls_server_name: String,
    template_vmids: Vec<i64>,
    client: reqwest::Client,
    caches: Arc<Mutex<Caches>>,
    /// Storage ID ISOs get uploaded to for BYO-OS (must have the "iso"
    /// content type enabled in Proxmox) — `PROXMOX_ISO_STORAGE`, default
    /// `"local"` (Proxmox's usual default ISO storage).
    iso_storage: String,
    /// Storage ID a BYO-OS VM's blank disk gets created on —
    /// `PROXMOX_VM_STORAGE`, default `"local-lvm"` (Proxmox's usual default
    /// thin-provisioned VM storage; override if this host uses ZFS or
    /// something else).
    vm_disk_storage: String,
    /// Bridge a BYO-OS VM's NIC attaches to — `PROXMOX_VM_BRIDGE`, default
    /// `"vmbr0"` (Proxmox's usual default bridge). The template-clone path
    /// doesn't need this: it inherits whatever bridge the template's own
    /// net0 already uses. A from-scratch VM has no template to inherit
    /// from, so this needs its own setting.
    vm_bridge: String,
}

/// Constructor inputs (`{ host, port, node, tokenId, tokenSecret, legacyToken,
/// verifyTls, tlsServerName, templateVmids }` in the JS).
pub struct ProxmoxOptions {
    pub host: String,
    pub port: f64,
    pub node: String,
    pub token_id: Option<String>,
    pub token_secret: Option<String>,
    pub legacy_token: Option<String>,
    pub verify_tls: Option<String>,
    pub tls_server_name: Option<String>,
    pub template_vmids: Vec<f64>,
    pub iso_storage: Option<String>,
    pub vm_disk_storage: Option<String>,
    pub vm_bridge: Option<String>,
}

impl Default for ProxmoxOptions {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 8006.0,
            node: String::new(),
            token_id: None,
            token_secret: None,
            legacy_token: None,
            verify_tls: None,
            tls_server_name: None,
            template_vmids: vec![9010.0],
            iso_storage: None,
            vm_disk_storage: None,
            vm_bridge: None,
        }
    }
}

/// `cloneDesktop` inputs (destructured object in the JS). The numeric fields
/// are already `Number()`-coerced by the caller.
pub struct CloneDesktopParams {
    pub template_vmid: Option<f64>,
    pub vmid: Option<f64>,
    pub hostname: String,
    pub cpu_cores: Option<f64>,
    pub memory_mb: Option<f64>,
    pub disk_gb: Option<f64>,
    pub desktop_username: String,
    pub desktop_password: String,
}

/// `createFromIso({ ... })` inputs — the BYO-OS parallel to
/// `CloneDesktopParams`. No desktop username/password: there's no
/// cloud-init image to seed, the user sets up login themselves during
/// their own OS install.
pub struct CreateFromIsoParams {
    pub vmid: Option<f64>,
    pub hostname: String,
    pub cpu_cores: Option<f64>,
    pub memory_mb: Option<f64>,
    pub disk_gb: Option<f64>,
    pub iso_filename: String,
}

/// Module-level accessor for the process-wide Proxmox desktop service.
pub fn desktop() -> &'static ProxmoxDesktopService {
    ProxmoxDesktopService::desktop()
}

impl ProxmoxDesktopService {
    /// JS constructor (lib/proxmox_desktop.js:64-81).
    fn new(options: ProxmoxOptions) -> Self {
        let (legacy_host, legacy_port) = value_from_legacy_url(&options.host);
        let has_legacy_host = !legacy_host.is_empty();
        let host = if has_legacy_host {
            legacy_host
        } else {
            let raw = options.host.trim();
            let lower = raw.to_lowercase();
            let stripped = if lower.starts_with("https://") {
                raw.get(8..).unwrap_or("")
            } else if lower.starts_with("http://") {
                raw.get(7..).unwrap_or("")
            } else {
                raw
            };
            stripped
                .split('/')
                .next()
                .unwrap_or("")
                .split(':')
                .next()
                .unwrap_or("")
                .to_string()
        };
        // `this.port = legacy.host ? legacy.port : (Number(port) || 8006)`.
        let port = if has_legacy_host {
            legacy_port as i64
        } else if options.port.is_nan() || options.port == 0.0 {
            8006
        } else {
            options.port as i64
        };
        let node = options.node.trim().to_string();
        let base_url = normalized_api_base(&host, port as f64);
        let token_id = options.token_id.clone().unwrap_or_default();
        let token_secret = options.token_secret.clone().unwrap_or_default();
        let has_dedicated_configuration =
            !token_id.trim().is_empty() || !token_secret.trim().is_empty();
        let authorization = if has_dedicated_configuration {
            if !token_id.trim().is_empty() && !token_secret.trim().is_empty() {
                format!("PVEAPIToken={}={}", token_id.trim(), token_secret.trim())
            } else {
                String::new()
            }
        } else {
            options
                .legacy_token
                .clone()
                .unwrap_or_default()
                .trim()
                .to_string()
        };
        let verify_tls = parse_boolean(options.verify_tls.as_deref(), false);
        let tls_server_name = options
            .tls_server_name
            .clone()
            .unwrap_or_default()
            .trim()
            .to_string();
        // `[...new Set((templateVmids || []).map(Number).filter(Number.isInteger))]`.
        let mut template_vmids: Vec<i64> = Vec::new();
        for raw in &options.template_vmids {
            if raw.is_finite() && raw.fract() == 0.0 {
                let v = *raw as i64;
                if !template_vmids.contains(&v) {
                    template_vmids.push(v);
                }
            }
        }
        let mut builder = reqwest::Client::builder()
            .danger_accept_invalid_certs(!verify_tls)
            .redirect(reqwest::redirect::Policy::none());
        if !tls_server_name.is_empty() {
            if let Ok(ip) = host.parse::<std::net::IpAddr>() {
                let socket_addr = std::net::SocketAddr::new(ip, port as u16);
                builder = builder.resolve(&tls_server_name, socket_addr);
            }
        }
        let client = builder.build().unwrap_or_else(|_| reqwest::Client::new());
        Self {
            host,
            port,
            node,
            base_url,
            authorization,
            verify_tls,
            tls_server_name,
            template_vmids,
            client,
            caches: Arc::new(Mutex::new(Caches::default())),
            iso_storage: {
                let v = options.iso_storage.unwrap_or_default();
                if v.trim().is_empty() {
                    "local".to_string()
                } else {
                    v.trim().to_string()
                }
            },
            vm_disk_storage: {
                let v = options.vm_disk_storage.unwrap_or_default();
                if v.trim().is_empty() {
                    "local-lvm".to_string()
                } else {
                    v.trim().to_string()
                }
            },
            vm_bridge: {
                let v = options.vm_bridge.unwrap_or_default();
                if v.trim().is_empty() {
                    "vmbr0".to_string()
                } else {
                    v.trim().to_string()
                }
            },
        }
    }

    /// `ProxmoxDesktopService.fromEnv(env)` (lib/proxmox_desktop.js:83-99).
    fn from_env() -> Self {
        let env = |name: &str| std::env::var(name).ok();
        let legacy_url = env("PVE_URL").unwrap_or_default();
        let (legacy_host, legacy_port) = value_from_legacy_url(&legacy_url);
        // `String(PROXMOX_DESKTOP_TEMPLATES || PVE_TEMPLATE_LINUX || '9010')
        // .split(',').map(v => Number(v.trim()))` — empty segments Number to 0
        // and the constructor's isInteger filter drops them; non-numeric
        // segments are NaN and drop too.
        let templates_raw = env("PROXMOX_DESKTOP_TEMPLATES")
            .or_else(|| env("PVE_TEMPLATE_LINUX"))
            .unwrap_or_else(|| "9010".to_string());
        let template_vmids: Vec<f64> = templates_raw
            .split(',')
            .map(|value| {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    0.0
                } else {
                    trimmed.parse::<f64>().unwrap_or(f64::NAN)
                }
            })
            .collect();
        // `env.PROXMOX_HOST || legacy.host || '192.168.100.1'`.
        let proxmox_host = env("PROXMOX_HOST").unwrap_or_default();
        let host = if !proxmox_host.trim().is_empty() {
            proxmox_host
        } else if !legacy_host.is_empty() {
            legacy_host
        } else {
            "192.168.100.1".to_string()
        };
        // `env.PROXMOX_PORT || legacy.port || 8006` — legacy.port already
        // carries the 8006 fallback from `valueFromLegacyUrl`.
        let proxmox_port = env("PROXMOX_PORT")
            .map(|v| v.trim().parse::<f64>().unwrap_or(f64::NAN))
            .unwrap_or(f64::NAN);
        let port = if proxmox_port.is_nan() || proxmox_port == 0.0 {
            legacy_port
        } else {
            proxmox_port
        };
        let node = match env("PROXMOX_NODE") {
            Some(v) if !v.trim().is_empty() => v,
            _ => env("PVE_NODE").unwrap_or_else(|| "tartarus".to_string()),
        };
        Self::new(ProxmoxOptions {
            host,
            port,
            node,
            token_id: env("PROXMOX_TOKEN_ID"),
            token_secret: env("PROXMOX_TOKEN_SECRET"),
            legacy_token: env("PVE_TOKEN"),
            verify_tls: env("PROXMOX_VERIFY_TLS"),
            tls_server_name: env("PROXMOX_TLS_SERVERNAME"),
            template_vmids,
            iso_storage: env("PROXMOX_ISO_STORAGE"),
            vm_disk_storage: env("PROXMOX_VM_STORAGE"),
            vm_bridge: env("PROXMOX_VM_BRIDGE"),
        })
    }

    /// The process-wide service (`const proxmoxDesktop = ...fromEnv(...)`).
    pub fn desktop() -> &'static Self {
        static SERVICE: OnceLock<ProxmoxDesktopService> = OnceLock::new();
        SERVICE.get_or_init(Self::from_env)
    }

    /// `proxmoxDesktop.node` — read by the route layer for new records.
    pub fn node(&self) -> &str {
        &self.node
    }

    /// `proxmoxDesktop.templateVmids` — the clone allowlist.
    pub fn template_vmids(&self) -> &[i64] {
        &self.template_vmids
    }

    fn lock_caches(&self) -> MutexGuard<'_, Caches> {
        self.caches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `get configured` — `Boolean(baseUrl && node && authorization)`.
    pub fn configured(&self) -> bool {
        !self.base_url.is_empty() && !self.node.is_empty() && !self.authorization.is_empty()
    }

    fn assert_configured(&self) -> Result<(), ProxmoxServiceError> {
        if !self.configured() {
            return Err(ProxmoxServiceError::new(
                "NOT_CONFIGURED",
                "Proxmox is not configured.",
                503,
            ));
        }
        Ok(())
    }

    /// `assertVmid(vmid)` — `Number.isInteger` + 100..=9_999_999.
    pub fn assert_vmid(&self, vmid: Option<f64>) -> Result<i64, ProxmoxServiceError> {
        let value = vmid.unwrap_or(f64::NAN);
        if !value.is_finite() || value.fract() != 0.0 || value < 100.0 || value > 9_999_999.0 {
            return Err(ProxmoxServiceError::new(
                "INVALID_VM",
                "Invalid computer record.",
                400,
            ));
        }
        Ok(value as i64)
    }

    /// `record.guestType || 'qemu'`.
    fn guest_type_of(record: &Value) -> String {
        jsval::str_or(record.get("guestType"), "qemu")
    }

    /// `record.node || this.node`.
    fn node_of(&self, record: &Value) -> String {
        jsval::str_or(record.get("node"), &self.node)
    }

    /// `tlsOptions` as a JSON value (returned inside `createConsole`).
    fn tls_options_value(&self) -> Value {
        let mut tls = Map::new();
        tls.insert("rejectUnauthorized".to_string(), json!(self.verify_tls));
        if !self.tls_server_name.is_empty() {
            tls.insert("serverName".to_string(), json!(self.tls_server_name));
        }
        Value::Object(tls)
    }

    /// The URLSearchParams body for one request (array values repeat the key).
    fn form_body(params: &Value) -> String {
        let mut form = form_urlencoded::Serializer::new(String::new());
        if let Value::Object(entries) = params {
            for (key, value) in entries {
                append_form_value(&mut form, key, value);
            }
        }
        form.finish()
    }

    /// `request(method, apiPath, params, timeoutMs)` — resolves to
    /// `payload.data` (lib/proxmox_desktop.js:121-158).
    pub async fn request(
        &self,
        method: &str,
        api_path: &str,
        params: Option<&Value>,
        timeout_ms: u64,
    ) -> Result<Value, ProxmoxServiceError> {
        self.assert_configured()?;
        let verb = reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET);
        let url = if self.verify_tls && !self.tls_server_name.is_empty() {
            format!(
                "https://{}:{}/api2/json{}",
                self.tls_server_name, self.port, api_path
            )
        } else {
            format!("{}{}", self.base_url, api_path)
        };
        let mut req = self
            .client
            .request(verb.clone(), url)
            .header("Authorization", &self.authorization)
            .timeout(Duration::from_millis(timeout_ms));
        if let Some(body) = params.filter(|_| method != "GET") {
            req = req
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(ProxmoxDesktopService::form_body(body));
        }
        let response = req.send().await.map_err(|error| {
            if error.is_timeout() {
                ProxmoxServiceError::new("TIMEOUT", "The computer service timed out.", 504)
            } else {
                tracing::warn!(
                    "[proxmox] {method} {api_path} failed: {error} (source: {:?})",
                    error.source()
                );
                ProxmoxServiceError::new("UNREACHABLE", "The computer service is unreachable.", 502)
            }
        })?;
        // JS `redirect: 'error'` rejects on a redirect; reqwest with
        // Policy::none hands back the 3xx response instead — map it here.
        if response.status().is_redirection() {
            return Err(ProxmoxServiceError::new(
                "UNREACHABLE",
                "The computer service is unreachable.",
                502,
            ));
        }
        let status = response.status().as_u16();
        let payload: Value = response
            .text()
            .await
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null);
        if !(200..300).contains(&status) {
            let mapped = if status == 401 || status == 403 {
                503
            } else {
                status
            };
            // Proxmox puts the real diagnostic (e.g. "VM is locked (backup)",
            // "unable to open file - No space left on device") in `message`,
            // or per-field detail in `errors` — surface it instead of a
            // generic string so admins can see why a power action failed.
            let detail = payload
                .get("message")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    payload.get("errors").and_then(|errors| {
                        errors.as_object().and_then(|obj| {
                            obj.values()
                                .next()
                                .and_then(Value::as_str)
                                .map(str::to_string)
                        })
                    })
                });
            let message = match detail {
                Some(d) => format!("The computer service rejected the request: {d}"),
                None => "The computer service rejected the request.".to_string(),
            };
            return Err(ProxmoxServiceError::new(
                "UPSTREAM_REJECTED",
                message,
                mapped,
            ));
        }
        Ok(payload.get("data").cloned().unwrap_or(Value::Null))
    }

    /// `listGuests()` (lib/proxmox_desktop.js:160-169).
    pub async fn list_guests(&self) -> Result<Vec<Value>, ProxmoxServiceError> {
        let rows = self
            .request(
                "GET",
                "/cluster/resources?type=vm",
                None,
                DEFAULT_TIMEOUT_MS,
            )
            .await?;
        let empty: Vec<Value> = Vec::new();
        let list: &[Value] = match &rows {
            Value::Array(items) => items.as_slice(),
            _ => &empty,
        };
        Ok(list
            .iter()
            .map(|row| {
                let vmid_num = jsval::number(&row["vmid"]);
                let vmid_str = match vmid_num {
                    Some(n) => jsval::num_value(n).to_string(),
                    None => "NaN".to_string(),
                };
                json!({
                    "vmid": vmid_num.map(jsval::num_value).unwrap_or(Value::Null),
                    "node": jsval::str_or(row.get("node"), &self.node),
                    "type": jsval::str_or(row.get("type"), "qemu"),
                    "name": jsval::str_or(row.get("name"), &format!("computer-{}", vmid_str)),
                    "status": jsval::str_or(row.get("status"), "unknown"),
                    "template": jsval::truthy(&row["template"]),
                    "cpuCores": jsval::num_value(js_num_or(row.get("maxcpu"), 0.0)),
                    "memoryMb": jsval::num_value(
                        (js_num_or(row.get("maxmem"), 0.0) / 1024.0 / 1024.0).round(),
                    ),
                    "diskGb": jsval::num_value(
                        (js_num_or(row.get("maxdisk"), 0.0) / 1024.0 / 1024.0 / 1024.0).round(),
                    ),
                    "uptime": jsval::num_value(js_num_or(row.get("uptime"), 0.0)),
                })
            })
            .collect())
    }

    /// `nodeCapacity()` (lib/proxmox_desktop.js:171-181).
    pub async fn node_capacity(&self) -> Result<Value, ProxmoxServiceError> {
        let row = self
            .request(
                "GET",
                &format!("/nodes/{}/status", encode_uri_component(&self.node)),
                None,
                DEFAULT_TIMEOUT_MS,
            )
            .await?;
        let memory = &row["memory"];
        let rootfs = &row["rootfs"];
        let cpuinfo = &row["cpuinfo"];
        Ok(json!({
            "cpuUsage": jsval::num_value(js_num_or(row.get("cpu"), 0.0)),
            "cpuCores": jsval::num_value(js_num_chain(
                &[cpuinfo.get("cpus"), cpuinfo.get("cores")],
                0.0,
            )),
            "memoryUsed": jsval::num_value(js_num_chain(&[memory.get("used")], 0.0)),
            "memoryTotal": jsval::num_value(js_num_chain(&[memory.get("total")], 0.0)),
            "storageUsed": jsval::num_value(js_num_chain(&[rootfs.get("used")], 0.0)),
            "storageTotal": jsval::num_value(js_num_chain(&[rootfs.get("total")], 0.0)),
            "uptime": jsval::num_value(js_num_or(row.get("uptime"), 0.0)),
        }))
    }

    /// `getConfig(record)` (lib/proxmox_desktop.js:183-186).
    pub async fn get_config(&self, record: &Value) -> Result<Value, ProxmoxServiceError> {
        let vmid = self.assert_vmid(jsval::number(&record["vmid"]))?;
        let node = encode_uri_component(&self.node_of(record));
        let guest_type = Self::guest_type_of(record);
        self.request(
            "GET",
            &format!("/nodes/{}/{}/{}/config", node, guest_type, vmid),
            None,
            DEFAULT_TIMEOUT_MS,
        )
        .await
    }

    /// `getStatus(record)` (lib/proxmox_desktop.js:188-226) — the guest-agent
    /// lookups are throttled to one attempt per 300s per VM.
    pub async fn get_status(&self, record: &Value) -> Result<Value, ProxmoxServiceError> {
        let vmid = self.assert_vmid(jsval::number(&record["vmid"]))?;
        let node = encode_uri_component(&self.node_of(record));
        let guest_type = Self::guest_type_of(record);
        let status = self
            .request(
                "GET",
                &format!("/nodes/{}/{}/{}/status/current", node, guest_type, vmid),
                None,
                DEFAULT_TIMEOUT_MS,
            )
            .await?;
        if jsval::str_or(status.get("status"), "unknown") != "running" {
            let mut caches = self.lock_caches();
            caches.guest_ip.remove(&vmid);
            caches.guest_hostname.remove(&vmid);
            caches.guest_agent_last_attempt.remove(&vmid);
        }
        let mut ip_address = jsval::str_or(record.get("ipAddress"), "");
        let mut hostname = jsval::str_or(record.get("hostname"), "");
        let should_query_agent = {
            let caches = self.lock_caches();
            if ip_address.is_empty() {
                ip_address = caches.guest_ip.get(&vmid).cloned().unwrap_or_default();
            }
            if hostname.is_empty() {
                hostname = caches
                    .guest_hostname
                    .get(&vmid)
                    .cloned()
                    .unwrap_or_default();
            }
            let last_attempt = *caches.guest_agent_last_attempt.get(&vmid).unwrap_or(&0);
            guest_type == "qemu"
                && jsval::str_or(status.get("status"), "unknown") == "running"
                && (ip_address.is_empty() || hostname.is_empty())
                && now_ms().saturating_sub(last_attempt) > 300_000
        };
        if should_query_agent {
            self.lock_caches()
                .guest_agent_last_attempt
                .insert(vmid, now_ms());
            if ip_address.is_empty() {
                if let Ok(agent) = self
                    .request(
                        "GET",
                        &format!("/nodes/{}/qemu/{}/agent/network-get-interfaces", node, vmid),
                        None,
                        1500,
                    )
                    .await
                {
                    let found = first_private_ipv4(&agent);
                    if !found.is_empty() {
                        self.lock_caches().guest_ip.insert(vmid, found.clone());
                        ip_address = found;
                    }
                }
            }
            if hostname.is_empty() {
                if let Ok(agent_host) = self
                    .request(
                        "GET",
                        &format!("/nodes/{}/qemu/{}/agent/get-host-name", node, vmid),
                        None,
                        1000,
                    )
                    .await
                {
                    // `agentHost?.result?.['host-name'] || agentHost?.['host-name'] || hostname`.
                    let from_result = agent_host["result"]["host-name"].clone();
                    let from_top = agent_host["host-name"].clone();
                    let picked = if jsval::truthy(&from_result) {
                        from_result
                    } else if jsval::truthy(&from_top) {
                        from_top
                    } else {
                        Value::String(hostname.clone())
                    };
                    hostname = jsval::string(&picked);
                    if !hostname.is_empty() {
                        self.lock_caches()
                            .guest_hostname
                            .insert(vmid, hostname.clone());
                    }
                }
            }
        }
        Ok(json!({
            "state": jsval::str_or(status.get("status"), "unknown"),
            "cpuUsage": jsval::num_value(js_num_or(status.get("cpu"), 0.0)),
            "cpuCores": jsval::num_value(js_num_chain(
                &[status.get("cpus"), record.get("cpuCores")],
                0.0,
            )),
            "memoryUsed": jsval::num_value(js_num_or(status.get("mem"), 0.0)),
            "memoryTotal": jsval::num_value(js_num_chain(
                &[status.get("maxmem")],
                js_num_or(record.get("memoryMb"), 0.0) * 1024.0 * 1024.0,
            )),
            "diskUsed": jsval::num_value(js_num_or(status.get("disk"), 0.0)),
            "diskTotal": jsval::num_value(js_num_chain(
                &[status.get("maxdisk")],
                js_num_or(record.get("diskGb"), 0.0) * 1024.0 * 1024.0 * 1024.0,
            )),
            "uptime": jsval::num_value(js_num_or(status.get("uptime"), 0.0)),
            "ipAddress": ip_address,
            "hostname": hostname,
        }))
    }
}

fn append_form_value(form: &mut form_urlencoded::Serializer<String>, key: &str, value: &Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                append_form_value(form, key, item);
            }
        }
        Value::Number(_) => {
            let n = jsval::number(value).unwrap_or(f64::NAN);
            if n.is_nan() {
                form.append_pair(key, "NaN");
            } else {
                form.append_pair(key, &jsval::num_value(n).to_string());
            }
        }
        other => {
            form.append_pair(key, &jsval::string(other));
        }
    }
}

impl ProxmoxDesktopService {
    /// `ensureOptimizedVmConfig(vmid, node)` (lib/proxmox_desktop.js:228-249)
    /// — the whole body is try/catch in the JS, so every failure path returns
    /// `None` (null) and only a successful pass marks the VMID optimized.
    pub async fn ensure_optimized_vm_config(&self, vmid: i64, node: &str) -> Option<Value> {
        if self.lock_caches().optimized_vmids.contains(&vmid) {
            return None;
        }
        let path = format!("/nodes/{}/qemu/{}/config", encode_uri_component(node), vmid);
        let outcome: Result<Option<Value>, ProxmoxServiceError> = async {
            let config = self.request("GET", &path, None, DEFAULT_TIMEOUT_MS).await?;
            let mut updates = Map::new();
            if config["cpu"].as_str() != Some("host") {
                updates.insert("cpu".to_string(), json!("host"));
            }
            if !jsval::truthy(&config["rng0"]) {
                updates.insert("rng0".to_string(), json!("source=/dev/urandom"));
            }
            let vga = config["vga"].as_str().unwrap_or("");
            if !jsval::truthy(&config["vga"]) || vga == "virtio" || vga == "std" {
                updates.insert("vga".to_string(), json!("std,memory=64"));
            }
            if !updates.is_empty() {
                self.request(
                    "PUT",
                    &path,
                    Some(&Value::Object(updates.clone())),
                    DEFAULT_TIMEOUT_MS,
                )
                .await?;
                tracing::info!(
                    "[proxmox] Applied boot optimizations to VM {}: {}",
                    vmid,
                    updates.keys().cloned().collect::<Vec<_>>().join(", ")
                );
            }
            self.lock_caches().optimized_vmids.insert(vmid);
            Ok(Some(Value::Object(updates)))
        }
        .await;
        match outcome {
            Ok(updates) => updates,
            Err(error) => {
                tracing::warn!(
                    "[proxmox] Could not optimize VM {} config: {}",
                    vmid,
                    error.message
                );
                None
            }
        }
    }

    /// `ensureGuestOptimized(vmid, node)` (lib/proxmox_desktop.js:251-279) —
    /// infallible like the JS: the ping failure returns early and the guest
    /// exec failure only warns.
    pub async fn ensure_guest_optimized(&self, vmid: i64, node: &str) {
        if self
            .request(
                "POST",
                &format!(
                    "/nodes/{}/qemu/{}/agent/ping",
                    encode_uri_component(node),
                    vmid
                ),
                Some(&json!({})),
                2000,
            )
            .await
            .is_err()
        {
            return;
        }
        let script = "set -eu
systemctl disable --now systemd-networkd-wait-online.service NetworkManager-wait-online.service >/dev/null 2>&1 || true
systemctl mask systemd-networkd-wait-online.service NetworkManager-wait-online.service >/dev/null 2>&1 || true
modprobe virtio_rng >/dev/null 2>&1 || true
if [ -f /etc/gdm3/custom.conf ]; then
  if grep -q '^#*WaylandEnable=' /etc/gdm3/custom.conf; then
    sed -i 's/^#*WaylandEnable=.*/WaylandEnable=false/' /etc/gdm3/custom.conf
  elif grep -q '\\[daemon\\]' /etc/gdm3/custom.conf; then
    sed -i '/\\[daemon\\]/a WaylandEnable=false' /etc/gdm3/custom.conf
  fi
  if ! pgrep -x Xorg >/dev/null 2>&1; then
    systemctl restart gdm3 >/dev/null 2>&1 || true
  fi
fi
exit 0
";
        if let Err(error) = self
            .guest_exec(vmid, &json!(["/bin/sh", "-s"]), script, 15_000)
            .await
        {
            tracing::warn!(
                "[proxmox] Background guest optimization skipped for VM {}: {}",
                vmid,
                error.message
            );
        }
    }

    /// `power(record, requestedAction)` (lib/proxmox_desktop.js:281-293).
    pub async fn power(
        &self,
        record: &Value,
        requested_action: &str,
    ) -> Result<Value, ProxmoxServiceError> {
        let vmid = self.assert_vmid(jsval::number(&record["vmid"]))?;
        let action = match requested_action {
            "start" => "start",
            "shutdown" => "shutdown",
            "restart" => "reboot",
            "force-stop" => "stop",
            _ => {
                return Err(ProxmoxServiceError::new(
                    "INVALID_ACTION",
                    "Invalid power action.",
                    400,
                ))
            }
        };
        if Self::guest_type_of(record) == "qemu"
            && (requested_action == "start" || requested_action == "restart")
        {
            let node = self.node_of(record);
            let _ = self.ensure_optimized_vm_config(vmid, &node).await;
            let service = self.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(5000)).await;
                service.ensure_guest_optimized(vmid, &node).await;
            });
        }
        self.request(
            "POST",
            &format!(
                "/nodes/{}/{}/{}/status/{}",
                encode_uri_component(&self.node_of(record)),
                Self::guest_type_of(record),
                vmid,
                action
            ),
            Some(&json!({})),
            DEFAULT_TIMEOUT_MS,
        )
        .await
    }

    /// `deleteGuest(record, { force })` (lib/proxmox_desktop.js:295-313).
    pub async fn delete_guest(
        &self,
        record: &Value,
        force: bool,
    ) -> Result<Value, ProxmoxServiceError> {
        let vmid = self.assert_vmid(jsval::number(&record["vmid"]))?;
        let node = self.node_of(record);
        let guest_type = Self::guest_type_of(record);
        let outcome: Result<Value, ProxmoxServiceError> = async {
            // `try { await power force-stop; await Bun.sleep(1000); } catch {}`
            // — the sleep only happens when the stop succeeded.
            if self.power(record, "force-stop").await.is_ok() {
                tokio::time::sleep(Duration::from_millis(1000)).await;
            }
            let upid = self
                .request(
                    "DELETE",
                    &format!(
                        "/nodes/{}/{}/{}?purge=1&destroy-unreferenced-disks=1",
                        encode_uri_component(&node),
                        guest_type,
                        vmid
                    ),
                    None,
                    30_000,
                )
                .await?;
            if jsval::truthy(&upid) {
                self.wait_for_task(&node, Some(&upid), 30_000).await?;
            }
            Ok(json!({ "success": true }))
        }
        .await;
        match outcome {
            Ok(value) => Ok(value),
            Err(error) if force => Ok(json!({ "success": true, "ignoredError": error.message })),
            Err(error) => Err(error),
        }
    }

    /// `createConsole(record)` (lib/proxmox_desktop.js:315-334).
    pub async fn create_console(&self, record: &Value) -> Result<Value, ProxmoxServiceError> {
        let vmid = self.assert_vmid(jsval::number(&record["vmid"]))?;
        if Self::guest_type_of(record) != "qemu" {
            return Err(ProxmoxServiceError::new(
                "NO_GRAPHICAL_DESKTOP",
                "This computer does not have a graphical desktop.",
                409,
            ));
        }
        let status = self.get_status(record).await?;
        if jsval::str_or(status.get("state"), "unknown") != "running" {
            return Err(ProxmoxServiceError::new(
                "STOPPED",
                "The computer is not running.",
                409,
            ));
        }
        let node = self.node_of(record);
        // `void this.ensureOptimizedVmConfig(...).catch(...);
        //  void this.ensureGuestOptimized(...).catch(...)` — two detached tasks.
        let service = self.clone();
        let node_for_opt = node.clone();
        tokio::spawn(async move {
            let _ = service
                .ensure_optimized_vm_config(vmid, &node_for_opt)
                .await;
        });
        let service = self.clone();
        let node_for_guest = node.clone();
        tokio::spawn(async move {
            service.ensure_guest_optimized(vmid, &node_for_guest).await;
        });
        let console_data = self
            .request(
                "POST",
                &format!(
                    "/nodes/{}/qemu/{}/vncproxy",
                    encode_uri_component(&node),
                    vmid
                ),
                Some(&json!({ "websocket": 1 })),
                DEFAULT_TIMEOUT_MS,
            )
            .await?;
        let port = js_num_or(console_data.get("port"), 0.0);
        let ticket = jsval::str_or(console_data.get("ticket"), "");
        if !port.is_finite()
            || port.fract() != 0.0
            || !(5900.0..=5999.0).contains(&port)
            || ticket.is_empty()
        {
            return Err(ProxmoxServiceError::new(
                "CONSOLE_FAILED",
                "The desktop connection could not be created.",
                502,
            ));
        }
        let ws_url = format!(
            "{}/api2/json/nodes/{}/qemu/{}/vncwebsocket?port={}&vncticket={}",
            self.ws_base(),
            encode_uri_component(&node),
            vmid,
            jsval::num_value(port),
            encode_uri_component(&ticket)
        );
        Ok(json!({
            "wsUrl": ws_url,
            "port": jsval::num_value(port),
            "ticket": ticket,
            "authorization": self.authorization,
            "tlsOptions": self.tls_options_value(),
        }))
    }

    /// `this.baseUrl.replace(/^http/i, 'ws').replace(/\/api2\/json$/, '')`.
    fn ws_base(&self) -> String {
        let lower_head = self.base_url.get(..4).map(|s| s.to_ascii_lowercase());
        let swapped = if lower_head.as_deref() == Some("http") {
            format!("ws{}", self.base_url.get(4..).unwrap_or(""))
        } else {
            self.base_url.clone()
        };
        match swapped.strip_suffix("/api2/json") {
            Some(head) => head.to_string(),
            None => swapped,
        }
    }

    /// `waitForTask(node, upid, timeoutMs)` (lib/proxmox_desktop.js:336-348).
    pub async fn wait_for_task(
        &self,
        node: &str,
        upid: Option<&Value>,
        timeout_ms: u64,
    ) -> Result<(), ProxmoxServiceError> {
        let upid = match upid {
            Some(raw) if jsval::truthy(raw) => jsval::string(raw),
            _ => return Ok(()),
        };
        let path = format!(
            "/nodes/{}/tasks/{}/status",
            encode_uri_component(node),
            encode_uri_component(&upid)
        );
        let started = now_ms();
        while now_ms().saturating_sub(started) < timeout_ms {
            let status = self.request("GET", &path, None, 8000).await?;
            if jsval::str_or(status.get("status"), "unknown") == "stopped" {
                let exitstatus = jsval::str_or(status.get("exitstatus"), "");
                if !exitstatus.is_empty() && exitstatus != "OK" {
                    return Err(ProxmoxServiceError::new(
                        "TASK_FAILED",
                        format!("The computer could not be prepared. Proxmox said: {exitstatus}"),
                        502,
                    ));
                }
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }
        Err(ProxmoxServiceError::new(
            "TASK_TIMEOUT",
            "The computer is still being prepared.",
            504,
        ))
    }

    /// `waitForGuestAgent(vmid, timeoutMs)` (lib/proxmox_desktop.js:350-361).
    pub async fn wait_for_guest_agent(
        &self,
        vmid: i64,
        timeout_ms: u64,
    ) -> Result<(), ProxmoxServiceError> {
        let started = now_ms();
        while now_ms().saturating_sub(started) < timeout_ms {
            if self
                .request(
                    "POST",
                    &format!(
                        "/nodes/{}/qemu/{}/agent/ping",
                        encode_uri_component(&self.node),
                        vmid
                    ),
                    Some(&json!({})),
                    5000,
                )
                .await
                .is_ok()
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(2000)).await;
        }
        Err(ProxmoxServiceError::new(
            "GUEST_SETUP_FAILED",
            "The graphical desktop did not finish starting.",
            504,
        ))
    }

    /// `guestExec(vmid, command, inputData, timeoutMs)`
    /// (lib/proxmox_desktop.js:363-381).
    pub async fn guest_exec(
        &self,
        vmid: i64,
        command: &Value,
        input_data: &str,
        timeout_ms: u64,
    ) -> Result<(), ProxmoxServiceError> {
        let mut params = Map::new();
        params.insert("command".to_string(), command.clone());
        if !input_data.is_empty() {
            params.insert("input-data".to_string(), json!(input_data));
        }
        let started = self
            .request(
                "POST",
                &format!(
                    "/nodes/{}/qemu/{}/agent/exec",
                    encode_uri_component(&self.node),
                    vmid
                ),
                Some(&Value::Object(params)),
                DEFAULT_TIMEOUT_MS,
            )
            .await?;
        let pid = js_num_or(started.get("pid"), 0.0);
        if !pid.is_finite() || pid.fract() != 0.0 || pid < 1.0 {
            return Err(ProxmoxServiceError::new(
                "GUEST_SETUP_FAILED",
                "The graphical desktop could not be prepared.",
                502,
            ));
        }
        let path = format!(
            "/nodes/{}/qemu/{}/agent/exec-status?pid={}",
            encode_uri_component(&self.node),
            vmid,
            jsval::num_value(pid)
        );
        let began = now_ms();
        while now_ms().saturating_sub(began) < timeout_ms {
            let status = self.request("GET", &path, None, 8000).await?;
            if jsval::truthy(&status["exited"]) {
                if js_num_or(status.get("exitcode"), 0.0) != 0.0 {
                    return Err(ProxmoxServiceError::new(
                        "GUEST_SETUP_FAILED",
                        "The graphical desktop could not be prepared.",
                        502,
                    ));
                }
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }
        Err(ProxmoxServiceError::new(
            "GUEST_SETUP_FAILED",
            "The graphical desktop is still being prepared.",
            504,
        ))
    }

    /// `enableFriendlyDesktopLogin(vmid, username, password)`
    /// (lib/proxmox_desktop.js:383-451).
    pub async fn enable_friendly_desktop_login(
        &self,
        vmid: i64,
        username: &str,
        password: &str,
    ) -> Result<(), ProxmoxServiceError> {
        let login = self.validate_desktop_login(
            username,
            if password.is_empty() {
                "temporary-validation-only"
            } else {
                password
            },
        )?;
        self.wait_for_guest_agent(vmid, 180_000).await?;
        let pass_b64 = if password.is_empty() {
            String::new()
        } else {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(login.1.as_bytes())
        };
        let script = format!(
            r#"set -eu
timeout 15 cloud-init status --wait >/dev/null 2>&1 || true
systemctl disable --now systemd-networkd-wait-online.service NetworkManager-wait-online.service >/dev/null 2>&1 || true
systemctl mask systemd-networkd-wait-online.service NetworkManager-wait-online.service >/dev/null 2>&1 || true
modprobe virtio_rng >/dev/null 2>&1 || true
user={username}
pass_b64="{pass_b64}"
if ! id "$user" >/dev/null 2>&1; then
  if getent group "$user" >/dev/null 2>&1; then
    useradd -m -s /bin/bash -g "$user" -G sudo,adm,cdrom,dip "$user"
  else
    useradd -m -s /bin/bash -U -G sudo,adm,cdrom,dip "$user"
  fi
fi
if [ -n "$pass_b64" ]; then
  pass=$(printf '%s' "$pass_b64" | base64 -d)
  printf '%s:%s\n' "$user" "$pass" | chpasswd
fi
usermod -aG sudo,adm,cdrom,dip "$user" >/dev/null 2>&1 || true
install -d -m 0755 "/home/$user/.config" /var/lib/AccountsService/users
touch "/home/$user/.config/gnome-initial-setup-done"
chown -R "$user:$user" "/home/$user/.config"
sudo -u "$user" dbus-run-session gsettings set org.gnome.desktop.session idle-delay 0 >/dev/null 2>&1 || true
sudo -u "$user" dbus-run-session gsettings set org.gnome.desktop.screensaver lock-enabled false >/dev/null 2>&1 || true
if id ubuntu >/dev/null 2>&1 && [ "$user" != ubuntu ]; then
  passwd -l ubuntu >/dev/null 2>&1 || true
  printf '[User]\nSystemAccount=true\n' >/var/lib/AccountsService/users/ubuntu
fi
if [ -d /etc/gdm3 ] || [ -f /etc/gdm3/custom.conf ]; then
  mkdir -p /etc/gdm3
  cat >/etc/gdm3/custom.conf <<EOF
[daemon]
AutomaticLoginEnable=true
AutomaticLogin=$user
WaylandEnable=false

[security]
[xdmcp]
[chooser]
[debug]
EOF
  systemctl restart gdm3 >/dev/null 2>&1 || true
fi
if [ -d /etc/lightdm ]; then
  mkdir -p /etc/lightdm/lightdm.conf.d
  cat >/etc/lightdm/lightdm.conf.d/50-autologin.conf <<EOF
[Seat:*]
autologin-user=$user
autologin-user-timeout=0
EOF
  systemctl restart lightdm >/dev/null 2>&1 || true
fi
uid=$(id -u "$user")
for attempt in $(seq 1 45); do
  if pgrep -u "$uid" -x gnome-shell >/dev/null 2>&1 || pgrep -u "$uid" -x xfce4-session >/dev/null 2>&1 || pgrep -u "$uid" -x cinnamon >/dev/null 2>&1 || pgrep -u "$uid" -x mate-session >/dev/null 2>&1; then
    loginctl unlock-sessions >/dev/null 2>&1 || true
    exit 0
  fi
  sleep 1
done
loginctl unlock-sessions >/dev/null 2>&1 || true
exit 0
"#,
            username = login.0,
            pass_b64 = pass_b64
        );
        self.guest_exec(vmid, &json!(["/bin/sh", "-s"]), &script, 150_000)
            .await
    }

    /// `nextAvailableVmid(min, max, reservedVmids)`
    /// (lib/proxmox_desktop.js:453-457).
    pub async fn next_available_vmid(
        &self,
        min: f64,
        max: f64,
        reserved_vmids: &[f64],
    ) -> Result<i64, ProxmoxServiceError> {
        let guests = self.list_guests().await?;
        let mut used: HashSet<i64> = HashSet::new();
        for guest in &guests {
            if let Some(n) = jsval::number(&guest["vmid"]) {
                if n.is_finite() && n.fract() == 0.0 {
                    used.insert(n as i64);
                }
            }
        }
        for raw in reserved_vmids {
            if raw.is_finite() && raw.fract() == 0.0 {
                used.insert(*raw as i64);
            }
        }
        // JS `for (let vmid = Number(min); vmid <= Number(max); vmid++)` — a
        // NaN bound means the loop body never runs and the caller sees
        // NO_CAPACITY.
        if !min.is_finite() || !max.is_finite() {
            return Err(ProxmoxServiceError::new(
                "NO_CAPACITY",
                "No computer slots are currently available.",
                409,
            ));
        }
        for vmid in (min as i64)..=(max as i64) {
            if !used.contains(&vmid) {
                return Ok(vmid);
            }
        }
        Err(ProxmoxServiceError::new(
            "NO_CAPACITY",
            "No computer slots are currently available.",
            409,
        ))
    }

    /// `cloneDesktop({ ... })` (lib/proxmox_desktop.js:459-504).
    pub async fn clone_desktop(
        &self,
        params: &CloneDesktopParams,
    ) -> Result<Value, ProxmoxServiceError> {
        let template_vmid = self.assert_vmid(params.template_vmid)?;
        let vmid = self.assert_vmid(params.vmid)?;
        if !self.template_vmids.contains(&template_vmid) {
            return Err(ProxmoxServiceError::new(
                "INVALID_TEMPLATE",
                "That desktop template is not allowed.",
                400,
            ));
        }
        let login =
            self.validate_desktop_login(&params.desktop_username, &params.desktop_password)?;
        let template_config = self
            .get_config(&json!({ "vmid": template_vmid, "node": self.node, "guestType": "qemu" }))
            .await?;
        static DISK_KEY_RE: OnceLock<Regex> = OnceLock::new();
        static CLOUDINIT_RE: OnceLock<Regex> = OnceLock::new();
        let disk_key_re = DISK_KEY_RE.get_or_init(|| {
            Regex::new(r"^(ide|scsi|sata)\d+$").unwrap_or_else(|_| unreachable_regex())
        });
        let cloudinit_re = CLOUDINIT_RE.get_or_init(|| {
            Regex::new(r"cloudinit(?:,|$)").unwrap_or_else(|_| unreachable_regex())
        });
        let has_cloudinit = template_config.as_object().is_some_and(|entries| {
            entries.iter().any(|(key, value)| {
                disk_key_re.is_match(key) && cloudinit_re.is_match(&jsval::string(value))
            })
        });
        let template_flag = jsval::number(&template_config["template"]).unwrap_or(f64::NAN);
        if template_flag == 0.0 || template_flag.is_nan() || !has_cloudinit {
            return Err(ProxmoxServiceError::new(
                "INVALID_TEMPLATE",
                "This template is not ready for automatic desktop setup.",
                400,
            ));
        }
        let net0 = jsval::str_or(template_config.get("net0"), "");
        static BRIDGE_RE: OnceLock<Regex> = OnceLock::new();
        let bridge_re = BRIDGE_RE
            .get_or_init(|| Regex::new(r"(?:^|,)bridge=").unwrap_or_else(|_| unreachable_regex()));
        if net0.is_empty() || !bridge_re.is_match(&net0) {
            return Err(ProxmoxServiceError::new(
                "INVALID_TEMPLATE",
                "This template does not have a configured network.",
                400,
            ));
        }
        let cpu_input = match params.cpu_cores {
            Some(raw) if !raw.is_nan() && raw != 0.0 => raw,
            _ => 2.0,
        };
        let cpu_cores = clamp_js(cpu_input.round(), 2.0, 16.0);
        let mem_input = match params.memory_mb {
            Some(raw) if !raw.is_nan() && raw != 0.0 => raw,
            _ => 4096.0,
        };
        let memory_mb = clamp_js(mem_input.round(), 2048.0, 65536.0);
        let clean_host = clean_hostname(&params.hostname, vmid);
        let clone_upid = self
            .request(
                "POST",
                &format!(
                    "/nodes/{}/qemu/{}/clone",
                    encode_uri_component(&self.node),
                    template_vmid
                ),
                Some(&json!({
                    "newid": vmid,
                    "name": clean_host,
                    "full": 0,
                    "pool": "sandboxes",
                    "target": self.node,
                })),
                30_000,
            )
            .await?;
        self.wait_for_task(&self.node, Some(&clone_upid), 120_000)
            .await?;
        let balloon = clamp_js((memory_mb / 4.0).floor(), 1024.0, 4096.0);
        let mut config_body = Map::new();
        config_body.insert("name".to_string(), json!(clean_host));
        config_body.insert("cores".to_string(), jsval::num_value(cpu_cores));
        config_body.insert("sockets".to_string(), json!(1));
        config_body.insert("memory".to_string(), jsval::num_value(memory_mb));
        config_body.insert("cpu".to_string(), json!("host"));
        config_body.insert("rng0".to_string(), json!("source=/dev/urandom"));
        config_body.insert(
            "vga".to_string(),
            jsval::or(template_config.get("vga"), json!("std,memory=64")),
        );
        config_body.insert("balloon".to_string(), jsval::num_value(balloon));
        config_body.insert("agent".to_string(), json!(1));
        config_body.insert("ciuser".to_string(), json!(login.0));
        config_body.insert("cipassword".to_string(), json!(login.1));
        config_body.insert("ciupgrade".to_string(), json!(0));
        config_body.insert("ipconfig0".to_string(), json!("ip=dhcp"));
        if jsval::truthy(&template_config["sshkeys"]) {
            config_body.insert("delete".to_string(), json!("sshkeys"));
        }
        self.request(
            "PUT",
            &format!(
                "/nodes/{}/qemu/{}/config",
                encode_uri_component(&self.node),
                vmid
            ),
            Some(&Value::Object(config_body)),
            DEFAULT_TIMEOUT_MS,
        )
        .await?;
        self.lock_caches().optimized_vmids.insert(vmid);
        let config = self
            .request(
                "GET",
                &format!(
                    "/nodes/{}/qemu/{}/config",
                    encode_uri_component(&self.node),
                    vmid
                ),
                None,
                DEFAULT_TIMEOUT_MS,
            )
            .await?;
        let current_disk_gb = parse_disk_gb(&config);
        let disk_input = match params.disk_gb {
            Some(raw) if !raw.is_nan() && raw != 0.0 => raw,
            _ => 64.0,
        };
        let requested_disk_gb = clamp_js(disk_input.round(), 40.0, 256.0);
        if current_disk_gb != 0.0 && requested_disk_gb > current_disk_gb {
            // `['scsi0','virtio0','sata0'].find(key => config[key])` — when no
            // key matches, the JS sends the literal string "undefined".
            let disk = ["scsi0", "virtio0", "sata0"]
                .into_iter()
                .find(|key| jsval::truthy(&config[*key]))
                .map(str::to_string)
                .unwrap_or_else(|| "undefined".to_string());
            let resize_body = json!({ "disk": disk, "size": format!("{}G", jsval::num_value(requested_disk_gb)) });
            let resize_upid = self
                .request(
                    "PUT",
                    &format!(
                        "/nodes/{}/qemu/{}/resize",
                        encode_uri_component(&self.node),
                        vmid
                    ),
                    Some(&resize_body),
                    DEFAULT_TIMEOUT_MS,
                )
                .await?;
            if jsval::truthy(&resize_upid) {
                self.wait_for_task(&self.node, Some(&resize_upid), 120_000)
                    .await?;
            }
        }
        let start_upid = self
            .request(
                "POST",
                &format!(
                    "/nodes/{}/qemu/{}/status/start",
                    encode_uri_component(&self.node),
                    vmid
                ),
                Some(&json!({})),
                DEFAULT_TIMEOUT_MS,
            )
            .await?;
        if jsval::truthy(&start_upid) {
            self.wait_for_task(&self.node, Some(&start_upid), 120_000)
                .await?;
        }
        let service = self.clone();
        let username = login.0.clone();
        let password = login.1.clone();
        tokio::spawn(async move {
            match service.enable_friendly_desktop_login(vmid, &username, &password).await {
                Ok(()) => {
                    tracing::info!("[proxmox] Friendly desktop auto-login setup completed for VM {}", vmid)
                }
                Err(ref error) => tracing::warn!(
                    "[proxmox] Friendly desktop auto-login setup deferred or timed out for VM {}: {}",
                    vmid,
                    error.message
                ),
            }
        });
        Ok(json!({
            "vmid": jsval::num_value(vmid as f64),
            "node": self.node,
            "hostname": clean_host,
            "cpuCores": jsval::num_value(cpu_cores),
            "memoryMb": jsval::num_value(memory_mb),
            "diskGb": jsval::num_value(current_disk_gb.max(requested_disk_gb)),
        }))
    }

    /// Streams a local file straight into Proxmox's storage upload API
    /// (`POST /nodes/{node}/storage/{storage}/upload`) without reading it
    /// into memory — by the time this runs, the file is already fully
    /// assembled on our own disk (the chunked browser-upload endpoint wrote
    /// it there), so this is a server-to-Proxmox transfer on the local
    /// network, not a browser upload, and can safely stream the whole thing
    /// in one request.
    pub async fn upload_iso(
        &self,
        local_path: &std::path::Path,
    ) -> Result<String, ProxmoxServiceError> {
        self.assert_configured()?;
        let url = if self.verify_tls && !self.tls_server_name.is_empty() {
            format!(
                "https://{}:{}/api2/json/nodes/{}/storage/{}/upload",
                self.tls_server_name,
                self.port,
                encode_uri_component(&self.node),
                encode_uri_component(&self.iso_storage)
            )
        } else {
            format!(
                "{}/nodes/{}/storage/{}/upload",
                self.base_url,
                encode_uri_component(&self.node),
                encode_uri_component(&self.iso_storage)
            )
        };

        // A multi-GB streamed upload held open for minutes is more exposed
        // to a transient mid-transfer connection reset (a stale pooled
        // keep-alive connection, a brief blip on the local network) than the
        // short JSON calls this client otherwise makes — that has nothing to
        // do with Proxmox actually being down. Retry both connection-level
        // failures AND a 400 response a couple of times: a 400 here is
        // consistent with an earlier attempt's upload actually having landed
        // on Proxmox's side even though the client saw its connection drop
        // (the response just never made it back) — Proxmox then rejects the
        // retry as a duplicate filename. A fresh random destination name
        // each attempt sidesteps that collision either way; any previous
        // attempt's filename is cleaned up best-effort before moving on, so
        // a secretly-successful earlier attempt doesn't orphan storage.
        const MAX_ATTEMPTS: u32 = 3;
        let mut last_err: Option<ProxmoxServiceError> = None;
        let mut prev_filename: Option<String> = None;
        for attempt in 1..=MAX_ATTEMPTS {
            if let Some(prev) = prev_filename.take() {
                let _ = self.delete_iso(&prev).await;
            }
            let remote_filename = format!("{}.iso", mitch_lib::crypto::random_bytes_hex(12));

            let file = tokio::fs::File::open(local_path).await.map_err(|_| {
                ProxmoxServiceError::new("ISO_READ_FAILED", "Could not read the uploaded ISO.", 500)
            })?;
            let file_len = file.metadata().await.map(|m| m.len()).unwrap_or(0);
            let stream = tokio_util::io::ReaderStream::new(file);
            let body = reqwest::Body::wrap_stream(stream);
            let part = reqwest::multipart::Part::stream_with_length(body, file_len)
                .file_name(remote_filename.clone())
                .mime_str("application/octet-stream")
                .unwrap_or_else(|_| reqwest::multipart::Part::bytes(Vec::new()));
            let form = reqwest::multipart::Form::new()
                .text("content", "iso")
                .part("filename", part);

            let response = match self
                .client
                .post(&url)
                .header("Authorization", &self.authorization)
                .multipart(form)
                .timeout(Duration::from_secs(600))
                .send()
                .await
            {
                Ok(r) => r,
                Err(error) => {
                    let is_timeout = error.is_timeout();
                    tracing::warn!(
                        "[proxmox] ISO upload attempt {attempt}/{MAX_ATTEMPTS} failed: {error} (source: {:?})",
                        error.source()
                    );
                    last_err = Some(if is_timeout {
                        ProxmoxServiceError::new(
                            "TIMEOUT",
                            "Uploading the ISO to the hypervisor timed out.",
                            504,
                        )
                    } else {
                        ProxmoxServiceError::new(
                            "UNREACHABLE",
                            "The computer service is unreachable.",
                            502,
                        )
                    });
                    // A timeout means it was actively transferring and just
                    // slow — retrying won't help. A connection-level error is
                    // the transient-blip case a fresh connection fixes. Keep
                    // this attempt's filename in case it actually landed.
                    prev_filename = Some(remote_filename);
                    if is_timeout || attempt == MAX_ATTEMPTS {
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(3 * attempt as u64)).await;
                    continue;
                }
            };

            let status = response.status().as_u16();
            if response.status().is_redirection() || !(200..300).contains(&status) {
                let body_text = response.text().await.unwrap_or_default();
                tracing::warn!(
                    "[proxmox] ISO upload attempt {attempt}/{MAX_ATTEMPTS} rejected (HTTP {status}): {body_text}"
                );
                let mapped = if status == 401 || status == 403 { 503 } else { status };
                last_err = Some(ProxmoxServiceError::new(
                    "UPSTREAM_REJECTED",
                    "Uploading the ISO to the hypervisor failed.",
                    mapped,
                ));
                // Only a plain 400 gets retried with a fresh name — the
                // duplicate-filename-collision case. Auth/storage-full/etc
                // errors won't be fixed by trying again.
                if status != 400 || attempt == MAX_ATTEMPTS {
                    break;
                }
                prev_filename = None; // this attempt's name was rejected outright, not silently claimed
                tokio::time::sleep(Duration::from_secs(3 * attempt as u64)).await;
                continue;
            }
            // The upload response is itself a UPID for Proxmox's background
            // copy/validate task — wait for that to finish the same way
            // clone/resize/start do, instead of treating "upload accepted" as
            // "file is ready to attach to a VM".
            let upid: Value = response
                .text()
                .await
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or(Value::Null);
            self.wait_for_task(&self.node, Some(&upid), 300_000).await?;
            return Ok(remote_filename);
        }

        Err(last_err.unwrap_or_else(|| {
            ProxmoxServiceError::new("UNREACHABLE", "The computer service is unreachable.", 502)
        }))
    }

    /// Removes an ISO from `iso_storage` — the BYO-OS counterpart to
    /// `upload_iso`, used when a user replaces or deletes their stored ISO
    /// (enforcing the "1 ISO max" rule doesn't leave orphaned files behind
    /// on the hypervisor either, not just off this box's own disk).
    pub async fn delete_iso(&self, remote_filename: &str) -> Result<(), ProxmoxServiceError> {
        self.request(
            "DELETE",
            &format!(
                "/nodes/{}/storage/{}/content/{}:iso/{}",
                encode_uri_component(&self.node),
                encode_uri_component(&self.iso_storage),
                self.iso_storage,
                remote_filename
            ),
            None,
            DEFAULT_TIMEOUT_MS,
        )
        .await?;
        Ok(())
    }

    /// Has Proxmox itself fetch an ISO from a URL straight into
    /// `iso_storage`, via `/storage/{storage}/download-url` — the BYO-OS
    /// counterpart to `upload_iso` for a user who gives a link instead of a
    /// file. This goes through the plain form-encoded `request()` helper
    /// (small body, no multipart streaming), the same path every other
    /// Proxmox call already uses reliably, rather than `upload_iso`'s
    /// bespoke multipart client use — deliberately, since this sidesteps
    /// whatever large-streamed-body issue that path can hit.
    pub async fn download_iso_from_url(
        &self,
        url: &str,
        remote_filename: &str,
    ) -> Result<(), ProxmoxServiceError> {
        let upid = self
            .request(
                "POST",
                &format!(
                    "/nodes/{}/storage/{}/download-url",
                    encode_uri_component(&self.node),
                    encode_uri_component(&self.iso_storage)
                ),
                Some(&json!({
                    "content": "iso",
                    "filename": remote_filename,
                    "url": url,
                    "verify-certificates": 1,
                })),
                30_000,
            )
            .await?;
        self.wait_for_task(&self.node, Some(&upid), 600_000).await
    }

    /// Looks up a stored ISO's size on `iso_storage` by listing storage
    /// content — used after `download_iso_from_url` to enforce the same
    /// 6GB cap the chunked-upload path enforces up front (a URL download's
    /// size isn't known until Proxmox has already fetched it).
    pub async fn get_iso_size(&self, remote_filename: &str) -> Option<u64> {
        let result = self
            .request(
                "GET",
                &format!(
                    "/nodes/{}/storage/{}/content",
                    encode_uri_component(&self.node),
                    encode_uri_component(&self.iso_storage)
                ),
                None,
                15_000,
            )
            .await
            .ok()?;
        let target_volid = format!("{}:iso/{}", self.iso_storage, remote_filename);
        result.as_array()?.iter().find_map(|item| {
            if jsval::str_or(item.get("volid"), "") == target_volid {
                item.get("size").and_then(jsval::number).map(|n| n as u64)
            } else {
                None
            }
        })
    }

    /// Creates a from-scratch VM (no clone, no cloud-init — the user is
    /// installing their own OS by hand) booting from an ISO already present
    /// on `iso_storage`, and starts it. Mirrors `clone_desktop`'s
    /// cores/memory/disk handling but has no template to read a network
    /// bridge or VGA config from, so those come from `vm_bridge` and a
    /// fixed default instead.
    pub async fn create_from_iso(
        &self,
        params: &CreateFromIsoParams,
    ) -> Result<Value, ProxmoxServiceError> {
        let vmid = self.assert_vmid(params.vmid)?;
        let cpu_input = match params.cpu_cores {
            Some(raw) if !raw.is_nan() && raw != 0.0 => raw,
            _ => 2.0,
        };
        let cpu_cores = clamp_js(cpu_input.round(), 2.0, 16.0);
        let mem_input = match params.memory_mb {
            Some(raw) if !raw.is_nan() && raw != 0.0 => raw,
            _ => 4096.0,
        };
        let memory_mb = clamp_js(mem_input.round(), 2048.0, 65536.0);
        let disk_input = match params.disk_gb {
            Some(raw) if !raw.is_nan() && raw != 0.0 => raw,
            _ => 64.0,
        };
        let disk_gb = clamp_js(disk_input.round(), 40.0, 256.0);
        let balloon = clamp_js((memory_mb / 4.0).floor(), 1024.0, 4096.0);
        let clean_host = clean_hostname(&params.hostname, vmid);

        let mut body = Map::new();
        body.insert("vmid".to_string(), jsval::num_value(vmid as f64));
        body.insert("name".to_string(), json!(clean_host));
        body.insert("cores".to_string(), jsval::num_value(cpu_cores));
        body.insert("sockets".to_string(), json!(1));
        body.insert("memory".to_string(), jsval::num_value(memory_mb));
        body.insert("balloon".to_string(), jsval::num_value(balloon));
        body.insert("cpu".to_string(), json!("host"));
        body.insert("rng0".to_string(), json!("source=/dev/urandom"));
        body.insert("vga".to_string(), json!("std,memory=64"));
        body.insert("agent".to_string(), json!(1));
        body.insert("scsihw".to_string(), json!("virtio-scsi-pci"));
        body.insert(
            "net0".to_string(),
            json!(format!("virtio,bridge={}", self.vm_bridge)),
        );
        body.insert(
            "scsi0".to_string(),
            json!(format!(
                "{}:{}",
                self.vm_disk_storage,
                jsval::num_value(disk_gb)
            )),
        );
        body.insert(
            "ide2".to_string(),
            json!(format!(
                "{}:iso/{},media=cdrom",
                self.iso_storage, params.iso_filename
            )),
        );
        body.insert("boot".to_string(), json!("order=ide2;scsi0"));
        // "other" rather than a specific guest hint — we don't know what
        // the user is installing, and an inaccurate ostype can make Proxmox
        // apply guest-specific quirks (e.g. Windows-only defaults) that
        // don't fit.
        body.insert("ostype".to_string(), json!("other"));

        let create_upid = self
            .request(
                "POST",
                &format!("/nodes/{}/qemu", encode_uri_component(&self.node)),
                Some(&Value::Object(body)),
                30_000,
            )
            .await?;
        self.wait_for_task(&self.node, Some(&create_upid), 120_000)
            .await?;

        let start_upid = self
            .request(
                "POST",
                &format!(
                    "/nodes/{}/qemu/{}/status/start",
                    encode_uri_component(&self.node),
                    vmid
                ),
                Some(&json!({})),
                DEFAULT_TIMEOUT_MS,
            )
            .await?;
        if jsval::truthy(&start_upid) {
            self.wait_for_task(&self.node, Some(&start_upid), 120_000)
                .await?;
        }

        Ok(json!({
            "vmid": jsval::num_value(vmid as f64),
            "node": self.node,
            "hostname": clean_host,
            "cpuCores": jsval::num_value(cpu_cores),
            "memoryMb": jsval::num_value(memory_mb),
            "diskGb": jsval::num_value(disk_gb),
        }))
    }

    /// `updateHardware(record, { cpuCores, memoryMb, diskGb })`
    /// (lib/proxmox_desktop.js:506-534). The numbers are already
    /// `Number()`-coerced by the caller.
    pub async fn update_hardware(
        &self,
        record: &Value,
        cpu_cores: Option<f64>,
        memory_mb: Option<f64>,
        disk_gb: Option<f64>,
    ) -> Result<Value, ProxmoxServiceError> {
        let vmid = self.assert_vmid(jsval::number(&record["vmid"]))?;
        let node = encode_uri_component(&self.node_of(record));
        let mut updates = Map::new();
        if let Some(raw) = cpu_cores {
            let cores = clamp_js(raw.round(), 2.0, 16.0);
            updates.insert("cores".to_string(), js_num_value(cores));
        }
        if let Some(raw) = memory_mb {
            let mem = clamp_js(raw.round(), 2048.0, 65536.0);
            updates.insert("memory".to_string(), js_num_value(mem));
            let balloon = clamp_js((mem / 4.0).floor(), 1024.0, 4096.0);
            updates.insert("balloon".to_string(), js_num_value(balloon));
        }
        if !updates.is_empty() {
            self.request(
                "PUT",
                &format!("/nodes/{}/qemu/{}/config", node, vmid),
                Some(&Value::Object(updates)),
                DEFAULT_TIMEOUT_MS,
            )
            .await?;
        }
        if let Some(raw) = disk_gb {
            let config = self.get_config(record).await?;
            let current_disk_gb = parse_disk_gb(&config);
            let requested_disk_gb = clamp_js(raw.round(), 40.0, 256.0);
            if current_disk_gb != 0.0 && requested_disk_gb > current_disk_gb {
                if let Some(disk) = ["scsi0", "virtio0", "sata0"]
                    .into_iter()
                    .find(|key| jsval::truthy(&config[*key]))
                {
                    let resize_body = json!({ "disk": disk, "size": format!("{}G", jsval::num_value(requested_disk_gb)) });
                    let resize_upid = self
                        .request(
                            "PUT",
                            &format!("/nodes/{}/qemu/{}/resize", node, vmid),
                            Some(&resize_body),
                            DEFAULT_TIMEOUT_MS,
                        )
                        .await?;
                    if jsval::truthy(&resize_upid) {
                        self.wait_for_task(&self.node_of(record), Some(&resize_upid), 120_000)
                            .await?;
                    }
                }
            }
        }
        Ok(json!({ "success": true }))
    }

    /// `validateDesktopLogin(username, password)`
    /// (lib/proxmox_desktop.js:536-543).
    pub fn validate_desktop_login(
        &self,
        username: &str,
        password: &str,
    ) -> Result<(String, String), ProxmoxServiceError> {
        static LOGIN_RE: OnceLock<Regex> = OnceLock::new();
        let login_re = LOGIN_RE.get_or_init(|| {
            Regex::new(r"^[a-z][a-z0-9_\-]{1,31}$").unwrap_or_else(|_| unreachable_regex())
        });
        let clean_username = username.trim().to_lowercase();
        let clean_password = password.to_string();
        // JS `.length` is UTF-16 code units.
        let password_len = clean_password.encode_utf16().count();
        let bad_password = !(8..=128).contains(&password_len)
            || clean_password.contains('\r')
            || clean_password.contains('\n')
            || clean_password.contains('\0');
        if !login_re.is_match(&clean_username)
            || ["root", "daemon", "nobody", "ubuntu"].contains(&clean_username.as_str())
            || bad_password
        {
            return Err(ProxmoxServiceError::new(
                "INVALID_DESKTOP_LOGIN",
                "Choose a desktop username and a password of 8 to 128 characters.",
                400,
            ));
        }
        Ok((clean_username, clean_password))
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> ProxmoxDesktopService {
        static SERVICE: OnceLock<ProxmoxDesktopService> = OnceLock::new();
        SERVICE
            .get_or_init(|| ProxmoxDesktopService::new(ProxmoxOptions::default()))
            .clone()
    }

    #[test]
    fn normalized_api_base_covers_url_and_bare_host() {
        assert_eq!(
            normalized_api_base("192.168.100.1", 8006.0),
            "https://192.168.100.1:8006/api2/json"
        );
        assert_eq!(
            normalized_api_base("https://pve.local/api2/json/", 8006.0),
            "https://pve.local/api2/json"
        );
        assert_eq!(normalized_api_base("", 8006.0), "");
        assert_eq!(
            normalized_api_base("host", f64::NAN),
            "https://host:8006/api2/json"
        );
    }

    #[test]
    fn value_from_legacy_url_variants() {
        let (host, port) = value_from_legacy_url("https://192.168.100.1:8006/api2/json");
        assert_eq!(host, "192.168.100.1");
        assert_eq!(port, 8006.0);
        let (host, port) = value_from_legacy_url("https://pve.local");
        assert_eq!(host, "pve.local");
        assert_eq!(port, 8006.0);
        let (host, port) = value_from_legacy_url("not a url");
        assert_eq!(host, "");
        assert_eq!(port, 8006.0);
    }

    #[test]
    fn parse_boolean_ladder() {
        assert!(!parse_boolean(Some("0"), true));
        assert!(!parse_boolean(Some("false"), true));
        assert!(!parse_boolean(Some("no"), true));
        assert!(!parse_boolean(Some("off"), true));
        assert!(!parse_boolean(Some("OFF "), true));
        assert!(parse_boolean(Some(""), true));
        assert!(parse_boolean(None, true));
        assert!(!parse_boolean(None, false));
        assert!(parse_boolean(Some("1"), false));
        assert!(parse_boolean(Some("true"), false));
        assert!(parse_boolean(Some("yes"), false));
    }

    #[test]
    fn validate_desktop_login_ladder() {
        let service = service();
        let login = service
            .validate_desktop_login("Fogler ", "correct-horse")
            .unwrap();
        assert_eq!(login, ("fogler".to_string(), "correct-horse".to_string()));
        for bad_username in [
            "a",
            "1user",
            "has space",
            "root",
            "ubuntu",
            "waytoolongusernamehere0123456789abcdef",
        ] {
            assert!(service
                .validate_desktop_login(bad_username, "correct-horse")
                .is_err());
        }
        assert!(service.validate_desktop_login("fogler", "short").is_err());
        assert!(service
            .validate_desktop_login("fogler", &"x".repeat(129))
            .is_err());
        assert!(service
            .validate_desktop_login("fogler", "has\nnewline")
            .is_err());
        assert!(service.validate_desktop_login("fogler", "has\rcr").is_err());
        assert!(service
            .validate_desktop_login("fogler", "has\0nul")
            .is_err());
        // An 8-char password is the lower bound; 128 is the upper bound.
        assert!(service.validate_desktop_login("fogler", "12345678").is_ok());
        assert!(service
            .validate_desktop_login("fogler", &"x".repeat(128))
            .is_ok());
        // The empty-password default used by enableFriendlyDesktopLogin.
        assert!(service.validate_desktop_login("fogler", "").is_err());
        assert!(service
            .validate_desktop_login("fogler", "temporary-validation-only")
            .is_ok());
    }

    #[test]
    fn assert_vmid_range() {
        assert_eq!(service().assert_vmid(Some(200.0)).unwrap(), 200);
        assert_eq!(service().assert_vmid(Some(100.0)).unwrap(), 100);
        assert_eq!(service().assert_vmid(Some(9_999_999.0)).unwrap(), 9_999_999);
        for bad in [
            Some(99.9),
            Some(9_999_999.5),
            Some(10_000_000.0),
            None,
            Some(200.5),
        ] {
            assert_eq!(service().assert_vmid(bad).unwrap_err().code, "INVALID_VM");
        }
    }

    #[test]
    fn parse_disk_gb_units() {
        assert_eq!(
            parse_disk_gb(&json!({ "scsi0": "local-lvm:vm-300-disk-0,size=64G" })),
            64.0
        );
        assert_eq!(parse_disk_gb(&json!({ "virtio0": "x,size=0.5T" })), 512.0);
        assert_eq!(parse_disk_gb(&json!({ "sata0": "x,size=512M" })), 1.0);
        assert_eq!(parse_disk_gb(&json!({ "scsi0": "x,size=1024K" })), 1.0);
        assert_eq!(parse_disk_gb(&json!({})), 0.0);
        // The first key carrying a size wins.
        assert_eq!(
            parse_disk_gb(&json!({ "scsi0": "size=20G", "virtio0": "size=100G" })),
            20.0
        );
    }

    #[test]
    fn first_private_ipv4_shapes() {
        let nested = json!({
            "result": [
                { "ip-addresses": [
                    { "ip-address": "127.0.0.1", "ip-address-type": "ipv4" },
                    { "ip-address": "10.1.2.3", "ip-address-type": "ipv4" },
                ] },
            ],
        });
        assert_eq!(first_private_ipv4(&nested), "10.1.2.3");
        let direct = json!([
            { "ip-addresses": [{ "ip-address": "0.0.0.0", "ip-address-type": "ipv4" }] },
            { "ip-addresses": [
                { "ip-address": "fe80::1", "ip-address-type": "ipv6" },
                { "ip-address": "192.168.4.5", "ip-address-type": "ipv4" },
            ] },
        ]);
        assert_eq!(first_private_ipv4(&direct), "192.168.4.5");
        assert_eq!(first_private_ipv4(&Value::Null), "");
    }

    #[test]
    fn clean_hostname_ladder() {
        assert_eq!(clean_hostname("My_Computer!!", 300), "my-computer");
        assert_eq!(clean_hostname("", 300), "computer-300");
        assert_eq!(clean_hostname("---", 300), "computer-300");
        assert_eq!(clean_hostname("a".repeat(60).as_str(), 300), "a".repeat(48));
        assert_eq!(clean_hostname("Multi--Dash", 300), "multi-dash");
    }

    #[test]
    fn form_body_encoding() {
        assert_eq!(ProxmoxDesktopService::form_body(&json!({})), "");
        assert_eq!(
            ProxmoxDesktopService::form_body(
                &json!({ "a": 1, "b": true, "c": ["x", "y"], "d": "z" })
            ),
            "a=1&b=true&c=x&c=y&d=z"
        );
        assert_eq!(
            ProxmoxDesktopService::form_body(&json!({ "websocket": 1 })),
            "websocket=1"
        );
    }

    #[test]
    fn service_defaults_match_from_env_fallbacks() {
        let service = ProxmoxDesktopService::new(ProxmoxOptions::default());
        assert_eq!(service.host, "");
        assert_eq!(service.port, 8006);
        assert_eq!(service.node, "");
        assert_eq!(service.base_url, "");
        assert_eq!(service.authorization, "");
        assert!(!service.configured());
        assert_eq!(service.template_vmids, vec![9010]);
    }

    #[test]
    fn service_construction_ladder() {
        let service = ProxmoxDesktopService::new(ProxmoxOptions {
            host: "https://pve.local:8006/".to_string(),
            node: " tartarus ".to_string(),
            legacy_token: Some(" tok ".to_string()),
            verify_tls: Some("0".to_string()),
            tls_server_name: Some(" pve.local ".to_string()),
            template_vmids: vec![9010.0, 9011.0, 9010.0, f64::NAN, 1.5],
            ..ProxmoxOptions::default()
        });
        assert_eq!(service.host, "pve.local");
        assert_eq!(service.port, 8006);
        assert_eq!(service.node, "tartarus");
        assert_eq!(service.base_url, "https://pve.local:8006/api2/json");
        assert_eq!(service.authorization, "tok");
        assert!(!service.verify_tls);
        assert_eq!(service.tls_server_name, "pve.local");
        assert_eq!(service.template_vmids, vec![9010, 9011]);
        assert!(service.configured());
        // Dedicated token configuration wins over the legacy token, and a
        // half-configured dedicated pair produces an empty authorization.
        let dedicated = ProxmoxDesktopService::new(ProxmoxOptions {
            host: "pve.local".to_string(),
            node: "tartarus".to_string(),
            token_id: Some("api@pve!id".to_string()),
            token_secret: Some("secret".to_string()),
            ..ProxmoxOptions::default()
        });
        assert_eq!(dedicated.authorization, "PVEAPIToken=api@pve!id=secret");
        let half = ProxmoxDesktopService::new(ProxmoxOptions {
            host: "pve.local".to_string(),
            node: "tartarus".to_string(),
            token_id: Some("api@pve!id".to_string()),
            ..ProxmoxOptions::default()
        });
        assert_eq!(half.authorization, "");
    }

    #[test]
    fn js_num_or_semantics() {
        // `Number(v || fallback)` — a truthy non-numeric stays NaN.
        assert!(js_num_or(Some(&json!("abc")), 5.0).is_nan());
        assert_eq!(js_num_or(Some(&json!(0)), 5.0), 5.0);
        assert_eq!(js_num_or(None, 5.0), 5.0);
        assert_eq!(js_num_or(Some(&json!(7)), 5.0), 7.0);
        assert_eq!(js_num_or(Some(&json!("7")), 5.0), 7.0);
    }

    #[test]
    fn clamp_js_propagates_nan() {
        assert_eq!(clamp_js(17.0, 2.0, 16.0), 16.0);
        assert_eq!(clamp_js(1.0, 2.0, 16.0), 2.0);
        assert_eq!(clamp_js(5.0, 2.0, 16.0), 5.0);
        assert!(clamp_js(f64::NAN, 2.0, 16.0).is_nan());
    }

    #[test]
    fn ws_base_swaps_scheme_and_strips_api_suffix() {
        let service = ProxmoxDesktopService::new(ProxmoxOptions {
            host: "https://pve.local:8006/".to_string(),
            node: "tartarus".to_string(),
            ..ProxmoxOptions::default()
        });
        assert_eq!(service.ws_base(), "wss://pve.local:8006");
    }

    #[test]
    fn guest_type_and_node_fallbacks() {
        let service = ProxmoxDesktopService::new(ProxmoxOptions {
            node: "tartarus".to_string(),
            ..ProxmoxOptions::default()
        });
        let record = json!({});
        assert_eq!(ProxmoxDesktopService::guest_type_of(&record), "qemu");
        assert_eq!(service.node_of(&record), "tartarus");
        let record = json!({ "guestType": "lxc", "node": "other" });
        assert_eq!(ProxmoxDesktopService::guest_type_of(&record), "lxc");
        assert_eq!(service.node_of(&record), "other");
    }
}
