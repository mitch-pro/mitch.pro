//! Sessions, cookies, bans, and rate limiting — port of server.js's auth
//! surface (getCookies ~5576, authSessionFromToken ~2789, checkPasswordCookie
//! ~5748, checkRateLimit ~5812, bans ~2864-2911).
//!
//! Contract highlights:
//! - Cookie `mitch_session` (AUTH_COOKIE): value = raw 32-byte base64url
//!   token; only sha256(token).hex is stored (auth_sessions.json key).
//! - `studentId`/`id`/`adminId` client cookies are DELETED (except
//!   NODE_ENV=test) and re-derived from the server-side session.
//! - `makeEmailId`: sha256(email|email:vN).hex[0..24] = 'e'+hash, sig =
//!   HMAC-SHA256(ID_SECRET, raw).hex[0..16], joined with '.'.
//! - `normalizeEmail`: lowercase+trim, strip +suffix, strip dots from local,
//!   fold mitch.pro/student.mitch.pro -> student.rjuhsd.us unless the local
//!   part is admin/support/noreply/mitch.
//! - Rate limits: sliding windows of float-epoch timestamps, two buckets per
//!   endpoint (`ip:<ip>` and `id:<sid>`/`anon`), anon = floor(max/5),
//!   timing-bot detection (>=4 intervals within 10s each, spread < 50ms).

use crate::data::DataStore;
use base64::Engine;
use serde_json::{json, Value};
use std::collections::HashMap;

/// Minimal header access without coupling mitch-lib to a web framework.
pub mod http {
    #[derive(Default)]
    pub struct HeaderMap(std::collections::HashMap<String, String>);

    impl HeaderMap {
        #[allow(clippy::new_without_default)]
        pub fn new() -> Self {
            Self(std::collections::HashMap::new())
        }
        pub fn get(&self, name: &str) -> Option<String> {
            self.0.get(&name.to_lowercase()).cloned()
        }
        pub fn insert(&mut self, name: &str, value: &str) {
            self.0.insert(name.to_lowercase(), value.to_string());
        }
    }

    impl From<&[(String, String)]> for HeaderMap {
        fn from(pairs: &[(String, String)]) -> Self {
            let mut m = Self::new();
            for (k, v) in pairs {
                m.insert(k, v);
            }
            m
        }
    }
}

pub const AUTH_COOKIE: &str = "mitch_session";
/// 30 days in ms (server.js:2681).
pub const AUTH_SESSION_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;
pub const DEV_TEST_EMAIL: &str = "admin@mitch.pro";
pub const WHITELISTED_IPS: &[&str] = &["66.60.183.124"];

/// `normalizeEmail(email)` — server.js:2054.
pub fn normalize_email(email: &str) -> String {
    if email.is_empty() {
        return String::new();
    }
    let e = email.to_lowercase().trim().to_string();
    if !e.contains('@') {
        return e;
    }
    let Some(at) = e.rfind('@') else {
        return e;
    };
    let local_raw = e[..at].split('+').next().unwrap_or("").to_string();
    let domain_raw = e[at + 1..].to_string();
    let local = local_raw.replace('.', "");
    let reserved = ["admin", "support", "noreply", "mitch"];
    let domain = if (domain_raw == "student.mitch.pro" || domain_raw == "mitch.pro")
        && !reserved.contains(&local.as_str())
    {
        "student.rjuhsd.us".to_string()
    } else {
        domain_raw
    };
    format!("{local}@{domain}")
}

/// `makeEmailId(email, gen)` — server.js:2645. Raw bytes of ID_SECRET as the
/// HMAC key (data/id_secret.key is read as a Buffer, not a string).
pub fn make_email_id(email: &str, gen: u64, id_secret: &[u8]) -> String {
    let key = if gen == 0 {
        email.to_string()
    } else {
        format!("{email}:v{gen}")
    };
    let email_hash = crate::crypto::sha256_hex(key.as_bytes());
    let raw = format!("e{}", &email_hash[..24]);
    let sig = &crate::crypto::hmac_sha256_hex(id_secret, raw.as_bytes())[..16];
    format!("{raw}.{sig}")
}

/// `validId(token)` — server.js:2651.
pub fn valid_id(token: &str, id_secret: &[u8]) -> bool {
    if token.is_empty() || !token.contains('.') {
        return false;
    }
    let Some(last_dot) = token.rfind('.') else {
        return false;
    };
    let raw = &token[..last_dot];
    let sig = &token[last_dot + 1..];
    let expected = &crate::crypto::hmac_sha256_hex(id_secret, raw.as_bytes())[..16];
    crate::crypto::timing_safe_equal(sig.as_bytes(), expected.as_bytes())
}

/// `hashSessionToken(token)` — sha256(token).hex.
pub fn hash_session_token(token: &str) -> String {
    crate::crypto::sha256_hex(token.as_bytes())
}

/// `devTestAccessEnabled()` — server.js:2683.
pub fn dev_test_access_enabled() -> bool {
    std::env::var("NODE_ENV").unwrap_or_default() != "production"
        && std::env::var("DEV_TEST_ACCESS").unwrap_or_default() == "1"
}

/// Parsed cookie jar — mirrors the JS getCookies() return value.
#[derive(Debug, Default, Clone)]
pub struct Cookies {
    pub map: HashMap<String, String>,
    /// Pre-deletion legacy values (JS cookies.rawStudentId / rawId).
    pub raw_student_id: String,
    pub raw_id: String,
}

impl Cookies {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.map.get(name).map(|s| s.as_str())
    }
    /// `authSidFromCookies`.
    pub fn auth_sid(&self) -> String {
        self.get("studentId").unwrap_or("").to_string()
    }
}

/// `getCookies(req)` — header-map variant. Framework-agnostic core below.
pub fn get_cookies(
    headers: &http::HeaderMap,
    store: &DataStore,
    id_secret: &[u8],
    node_env_test: bool,
) -> Cookies {
    let cookie_header = headers.get("cookie").unwrap_or_default();
    get_cookies_from_header_value(&cookie_header, store, id_secret, node_env_test)
}

/// String-based core (works with any framework's header extraction).
pub fn get_cookies_from_header_value(
    cookie_header: &str,
    store: &DataStore,
    id_secret: &[u8],
    node_env_test: bool,
) -> Cookies {
    let mut cookies = Cookies::default();
    for part in cookie_header.split(';') {
        let t = part.trim();
        if let Some(eq) = t.find('=') {
            let name = t[..eq].trim().to_string();
            let raw = t[eq + 1..].trim().to_string();
            let decoded = percent_decode(&raw).unwrap_or(raw);
            cookies.map.insert(name, decoded);
        }
    }

    let raw_student_id = cookies.get("studentId").unwrap_or("").to_string();
    let raw_id = cookies.get("id").unwrap_or("").to_string();
    cookies.raw_student_id = raw_student_id;
    cookies.raw_id = raw_id;

    // Display-only legacy cookies — never treat as auth proof.
    if !node_env_test {
        cookies.map.remove("studentId");
        cookies.map.remove("id");
        cookies.map.remove("adminId");
    }

    if let Some(token) = cookies.get(AUTH_COOKIE).map(str::to_string) {
        if let Some(session) = auth_session_from_token(&token, store, id_secret) {
            if !session.email.is_empty() || !session.norm_email.is_empty() {
                let mut sid = session.sid.clone();
                if sid.is_empty() || !valid_id(&sid, id_secret) {
                    sid = make_email_id(
                        session.norm_email.trim_matches('@'),
                        session.gen as u64,
                        id_secret,
                    );
                }
                cookies.map.insert("studentId".into(), sid.clone());
                cookies.map.insert("id".into(), sid);
                cookies
                    .map
                    .insert("_authSession".into(), session.serialize());
            }
        }
    }
    cookies
}

fn percent_decode(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 3 <= bytes.len() {
            let hex_pair = raw.get(i + 1..i + 3)?;
            let byte = u8::from_str_radix(hex_pair, 16).ok()?;
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// One auth_sessions.json record (server.js:2773-2784).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AuthSession {
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    #[serde(rename = "normEmail")]
    pub norm_email: String,
    #[serde(default)]
    pub sid: String,
    #[serde(default)]
    pub gen: i64,
    #[serde(rename = "createdAt", default)]
    pub created_at: i64,
    #[serde(rename = "lastSeen", default)]
    pub last_seen: i64,
    #[serde(rename = "expiresAt", default)]
    pub expires_at: i64,
    #[serde(rename = "userAgent", default)]
    pub user_agent: String,
    #[serde(default)]
    pub ip: String,
    #[serde(rename = "devSuperuser", default)]
    pub dev_superuser: bool,
}

impl AuthSession {
    fn serialize(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

fn sessions_file() -> std::path::PathBuf {
    std::path::PathBuf::from("data/auth_sessions.json")
}

pub(crate) fn names_file() -> std::path::PathBuf {
    std::path::PathBuf::from("data/names.json")
}

fn generations_file() -> std::path::PathBuf {
    std::path::PathBuf::from("data/generations.json")
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `currentSessionGeneration(normEmail)` — generations.json value or 0.
pub fn current_session_generation(store: &DataStore, norm: &str) -> i64 {
    let gens = store.read_document(&store.base_dir.join(generations_file()), Value::Null);
    match gens.get(norm) {
        Some(Value::Object(m)) => m.get("gen").and_then(|v| v.as_i64()).unwrap_or(0),
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        _ => 0,
    }
}

/// `issueLoginSession(normEmail, originalEmail)` — makeEmailId + names.json
/// write-back (sid -> display email).
pub fn issue_login_session(
    store: &DataStore,
    id_secret: &[u8],
    norm_email: &str,
    original_email: &str,
) -> String {
    let gen = current_session_generation(store, norm_email);
    let student_id = make_email_id(norm_email, gen as u64, id_secret);
    let names_path = store.base_dir.join(names_file());
    let mut names = store.read_document(&names_path, serde_json::json!({}));
    if names.get(&student_id).and_then(|v| v.as_str()) != Some(original_email) {
        if let Some(map) = names.as_object_mut() {
            map.insert(
                student_id.clone(),
                Value::String(original_email.to_string()),
            );
        }
        let _ = store.write_document(&names_path, &names);
    }
    student_id
}

/// `authSessionFromToken(token)` — validate + refresh (expiry, generation,
/// 5-min lastSeen). Returns None + lazily deletes expired/stale records.
pub fn auth_session_from_token(
    token: &str,
    store: &DataStore,
    id_secret: &[u8],
) -> Option<AuthSession> {
    if token.is_empty() {
        return None;
    }
    let key = hash_session_token(token);
    let sessions_path = store.base_dir.join(sessions_file());
    let mut sessions = store.read_document(&sessions_path, serde_json::json!({}));
    let now = now_millis();

    let mut rec = sessions.get(&key).cloned()?;
    let rec_obj = rec.as_object_mut()?;

    let expires_at = rec_obj
        .get("expiresAt")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if expires_at == 0 || now > expires_at {
        if let Some(map) = sessions.as_object_mut() {
            map.remove(&key);
        }
        let _ = store.write_document(&sessions_path, &sessions);
        return None;
    }

    let norm_email = rec_obj
        .get("normEmail")
        .or_else(|| rec_obj.get("email"))
        .and_then(|v| v.as_str())
        .map(normalize_email)
        .unwrap_or_default();
    if norm_email.is_empty() {
        return None;
    }

    let rec_gen = rec_obj.get("gen").and_then(|v| v.as_i64()).unwrap_or(0);
    if rec_gen != current_session_generation(store, &norm_email) {
        if let Some(map) = sessions.as_object_mut() {
            map.remove(&key);
        }
        let _ = store.write_document(&sessions_path, &sessions);
        return None;
    }

    let email = rec_obj
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let sid = issue_login_session(store, id_secret, &norm_email, &email);
    let stored_sid = rec_obj.get("sid").and_then(|v| v.as_str()).unwrap_or("");
    let last_seen = rec_obj
        .get("lastSeen")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if stored_sid != sid || now - last_seen > 5 * 60 * 1000 {
        rec_obj.insert("sid".into(), Value::String(sid.clone()));
        rec_obj.insert("lastSeen".into(), Value::Number(now.into()));
        let _ = store.write_document(&sessions_path, &sessions);
    }

    let mut session: AuthSession = serde_json::from_value(rec).unwrap_or_default();
    session.sid = sid;
    session.norm_email = norm_email;
    Some(session)
}

/// `bannedInfoForEmail(email)` — blacklist.json lookup.
pub fn banned_info_for_email(store: &DataStore, email: &str) -> Option<Value> {
    if email.is_empty() {
        return None;
    }
    let bl = store.read_document(
        &store.base_dir.join("data/blacklist.json"),
        serde_json::json!({}),
    );
    let norm = normalize_email(email);
    bl.get(&norm)
        .or_else(|| bl.get(email.to_lowercase().as_str()))
        .cloned()
}

/// `bannedInfoForSid(sid)` — sid -> email -> blacklist.
pub fn banned_info_for_sid(store: &DataStore, id_secret: &[u8], sid: &str) -> Option<Value> {
    if sid.is_empty() || !valid_id(sid, id_secret) {
        return None;
    }
    let email = email_from_sid(store, id_secret, sid);
    email.and_then(|e| banned_info_for_email(store, &e))
}

/// `bannedInfoForIp(ip)` — banned_ips.json lookup.
pub fn banned_info_for_ip(store: &DataStore, ip: &str) -> Option<Value> {
    if ip.is_empty() {
        return None;
    }
    let ips = store.read_document(
        &store.base_dir.join("data/banned_ips.json"),
        serde_json::json!({}),
    );
    ips.get(ip).cloned()
}

/// `emailFromSid(sid)` — names.json first, then tokens.json (incl. infinite
/// tokens with generation matching). Port of server.js:5844.
pub fn email_from_sid(store: &DataStore, id_secret: &[u8], sid: &str) -> Option<String> {
    if sid.is_empty() {
        return None;
    }
    let names = store.read_document(&store.base_dir.join(names_file()), serde_json::json!({}));
    if let Some(email) = names.get(sid).and_then(|v| v.as_str()) {
        let norm = normalize_email(email);
        let gen = current_session_generation(store, &norm);
        if sid == make_email_id(&norm, gen as u64, id_secret)
            || sid == make_email_id(email, gen as u64, id_secret)
        {
            return Some(email.to_string());
        }
        return None;
    }
    // tokens.json: direct sid key, then infinite tokens by generation.
    let tokens = store.read_document(
        &store.base_dir.join("data/tokens.json"),
        serde_json::json!({}),
    );
    if let Some(rec) = tokens.get(sid) {
        return rec
            .get("email")
            .and_then(|v| v.as_str())
            .map(str::to_string);
    }
    for (_tok, rec) in tokens.as_object()?.iter() {
        if !rec
            .get("infinite")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        let norm = rec
            .get("norm_email")
            .or_else(|| rec.get("email"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_default();
        let norm = normalize_email(&norm);
        let gen = rec.get("claim_count").and_then(|v| v.as_i64()).unwrap_or(0);
        if sid == make_email_id(&norm, gen as u64, id_secret) {
            return Some(
                rec.get("email")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or(norm),
            );
        }
    }
    None
}

/// The full `checkPasswordCookie(req, providedSid)` gate — server.js:5748.
/// `node_env_test` mirrors `process.env.NODE_ENV === 'test'` (always-true
/// bypass); `dev_test_access` mirrors `devTestAccessEnabled()`.
pub fn check_password_cookie(
    store: &DataStore,
    id_secret: &[u8],
    cookies: &Cookies,
    provided_sid: Option<&str>,
    node_env_test: bool,
    dev_test_access: bool,
) -> bool {
    let sid = cookies
        .get("studentId")
        .or_else(|| cookies.get("id"))
        .unwrap_or("");
    if sid.is_empty() {
        return false;
    }
    if let Some(provided) = provided_sid {
        if provided != sid {
            return false;
        }
    }
    if !valid_id(sid, id_secret) {
        return false;
    }
    if banned_info_for_sid(store, id_secret, sid).is_some() {
        return false;
    }

    let auth_session = cookies
        .get("_authSession")
        .and_then(|s| serde_json::from_str::<serde_json::Map<String, Value>>(s).ok());
    let email = auth_session
        .as_ref()
        .and_then(|s| s.get("email"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| email_from_sid(store, id_secret, sid))
        .or_else(|| {
            store
                .read_document(&store.base_dir.join(names_file()), serde_json::json!({}))
                .get(sid)
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
    let Some(email) = email else { return false };

    let norm = normalize_email(&email);
    let dev_superuser = auth_session
        .as_ref()
        .and_then(|s| s.get("devSuperuser"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if dev_superuser && dev_test_access && norm == DEV_TEST_EMAIL {
        return true;
    }
    if node_env_test {
        return true;
    }
    let passwords = store.read_document(
        &store.base_dir.join("data/passwords.json"),
        serde_json::json!({}),
    );
    passwords.get(&norm).is_some()
}

/// `cookiePathAttrs(req, maxAge, httpOnly)` — exact attribute string.
pub fn cookie_path_attrs(
    session_cookie_secure: &str,
    node_env_production: bool,
    max_age: i64,
    http_only: bool,
) -> String {
    let secure_flag = session_cookie_secure.trim();
    let secure =
        secure_flag == "1" || secure_flag.eq_ignore_ascii_case("true") || node_env_production;
    let mut parts = vec![
        "Path=/".to_string(),
        format!("Max-Age={max_age}"),
        "SameSite=Lax".to_string(),
    ];
    if secure {
        parts.push("Secure".to_string());
    }
    if http_only {
        parts.push("HttpOnly".to_string());
    }
    parts.join("; ")
}

/// `encodeURIComponent(value || '')` — JS encodeURIComponent (unreserved set:
/// `A-Za-z0-9-_.!~*'()`), used for cookie values in `setCookieHeader`.
pub fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
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
            | b')' => out.push(*b as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// `decodeURIComponent(value || '')` — JS decodeURIComponent.
pub fn decode_uri_component(s: &str) -> String {
    let mut bytes = Vec::with_capacity(s.len());
    let mut iter = s.as_bytes().iter().copied();
    while let Some(b) = iter.next() {
        if b == b'%' {
            let h1 = iter.next();
            let h2 = iter.next();
            if let (Some(h1), Some(h2)) = (h1, h2) {
                let hex_bytes = [h1, h2];
                if let Ok(hex_str) = std::str::from_utf8(&hex_bytes) {
                    if let Ok(val) = u8::from_str_radix(hex_str, 16) {
                        bytes.push(val);
                        continue;
                    }
                }
                bytes.push(b'%');
                bytes.push(h1);
                bytes.push(h2);
            } else {
                bytes.push(b'%');
                if let Some(h1) = h1 {
                    bytes.push(h1);
                }
            }
        } else {
            bytes.push(b);
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `setCookieHeader(name, value, req, maxAge, httpOnly)`.
pub fn set_cookie_header(
    name: &str,
    value: &str,
    secure_flag: &str,
    node_env_production: bool,
    max_age: i64,
    http_only: bool,
) -> String {
    format!(
        "{name}={}; {}",
        encode_uri_component(value),
        cookie_path_attrs(secure_flag, node_env_production, max_age, http_only)
    )
}

/// `clearCookieHeader(name, req, httpOnly)` (server.js:2841).
pub fn clear_cookie_header(
    name: &str,
    secure_flag: &str,
    node_env_production: bool,
    http_only: bool,
) -> String {
    format!(
        "{name}=; {}",
        cookie_path_attrs(secure_flag, node_env_production, 0, http_only)
    )
}

/// Result of `createAuthSession`.
pub struct IssuedAuthSession {
    pub token: String,
    pub sid: String,
    pub email: String,
    pub norm_email: String,
    pub gen: i64,
}

/// `createAuthSession(normEmail, originalEmail, req, options)`
/// (server.js:2870-2898) — random 32-byte base64url token, hashed key,
/// full session record in auth_sessions.json.
pub fn create_auth_session(
    store: &DataStore,
    id_secret: &[u8],
    norm_email: &str,
    original_email: &str,
    user_agent: &str,
    ip: &str,
    dev_superuser: bool,
) -> IssuedAuthSession {
    let norm = normalize_email(norm_email);
    let email = if original_email.is_empty() {
        norm.clone()
    } else {
        original_email.to_string()
    };
    let gen = current_session_generation(store, &norm);
    let sid = issue_login_session(store, id_secret, &norm, &email);
    let mut token_bytes = [0u8; 32];
    use rand::RngCore;
    rand::rng().fill_bytes(&mut token_bytes);
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token_bytes);
    let key = hash_session_token(&token);
    let now = now_millis();
    let sessions_path = store.base_dir.join(sessions_file());
    let mut sessions = store.read_document(&sessions_path, serde_json::json!({}));
    let ua: String = user_agent.chars().take(240).collect();
    if let Some(map) = sessions.as_object_mut() {
        map.insert(
            key,
            serde_json::json!({
                "email": email,
                "normEmail": norm,
                "sid": sid,
                "gen": gen,
                "createdAt": now,
                "lastSeen": now,
                "expiresAt": now + AUTH_SESSION_TTL_MS,
                "userAgent": ua,
                "ip": ip,
                "devSuperuser": dev_superuser && dev_test_access_enabled(),
            }),
        );
    }
    let _ = store.write_document(&sessions_path, &sessions);
    IssuedAuthSession {
        token,
        sid,
        email,
        norm_email: norm,
        gen,
    }
}

/// `invalidateAuthSessionsForEmail(normEmail, keepToken)` (server.js:2918-2931).
pub fn invalidate_auth_sessions_for_email(
    store: &DataStore,
    norm_email: &str,
    keep_token: Option<&str>,
) {
    let norm = normalize_email(norm_email);
    let keep_key = keep_token.map(hash_session_token).unwrap_or_default();
    let sessions_path = store.base_dir.join(sessions_file());
    let mut sessions = store.read_document(&sessions_path, serde_json::json!({}));
    let mut changed = false;
    if let Some(map) = sessions.as_object_mut() {
        let stale: Vec<String> = map
            .iter()
            .filter(|(key, rec)| {
                key.as_str() != keep_key
                    && normalize_email(
                        rec.get("normEmail")
                            .or_else(|| rec.get("email"))
                            .and_then(|v| v.as_str())
                            .unwrap_or(""),
                    ) == norm
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in stale {
            map.remove(&key);
            changed = true;
        }
    }
    if changed {
        let _ = store.write_document(&sessions_path, &sessions);
    }
}

/// `rotateSessionGeneration(normEmail)` (server.js:2932-2947) — bumps the
/// generation, drops the email's names.json sids, kills its sessions.
pub fn rotate_session_generation(store: &DataStore, norm_email: &str) -> i64 {
    let norm = normalize_email(norm_email);
    let current_gen = current_session_generation(store, &norm);
    let next_gen = current_gen + 1;
    let gens_path = store.base_dir.join(generations_file());
    let mut gens = store.read_document(&gens_path, serde_json::json!({}));
    if let Some(map) = gens.as_object_mut() {
        map.insert(
            norm.clone(),
            serde_json::json!({
                "gen": next_gen,
                "last_registered": now_millis() as f64 / 1000.0,
            }),
        );
    }
    let _ = store.write_document(&gens_path, &gens);

    let names_path = store.base_dir.join(names_file());
    let mut names = store.read_document(&names_path, serde_json::json!({}));
    if let Some(map) = names.as_object_mut() {
        let stale: Vec<String> = map
            .iter()
            .filter(|(_, v)| {
                v.as_str()
                    .map(|s| normalize_email(s) == norm)
                    .unwrap_or(false)
            })
            .map(|(k, _)| k.clone())
            .collect();
        for key in stale {
            map.remove(&key);
        }
        let _ = store.write_document(&names_path, &names);
    }
    invalidate_auth_sessions_for_email(store, &norm, None);
    next_gen
}

// ── Rate limiting ────────────────────────────────────────────────────────────

/// `RATE_LIMITS` — [maxRequests, windowSeconds] per exact endpoint path.
/// Verbatim from server.js:1392-1506.
pub fn rate_limit_for(endpoint: &str) -> (u32, u32) {
    match endpoint {
        "/api/webauthn/login/options" => (10, 60),
        "/api/webauthn/login/verify" => (10, 60),
        "/api/webauthn/register/options" => (20, 60),
        "/api/webauthn/register/verify" => (20, 60),
        "/api/webauthn/credentials" => (60, 60),
        "/api/webauthn/credentials/rename" => (20, 60),
        "/api/webauthn/credentials/delete" => (10, 60),
        "/api/request-access" => (10, 600),
        "/api/claim-token" => (10, 3600),
        "/api/pass" => (60, 60),
        "/api/e2e/verify-password" => (5, 60),
        "/api/content" => (120, 60),
        "/api/ping" => (120, 60),
        "/api/me/notif-prefs" => (30, 60),
        "/api/ai" => (20, 60),
        "/api/script" => (600, 60),
        "/api/admin/js" => (5, 60),
        "/api/admin/trigger-daily-summary" => (10, 60),
        "/api/admin/gift-coins" => (10, 60),
        "/api/admin/grant-premium" => (10, 60),
        "/api/admin/revoke-premium" => (10, 60),
        "/api/admin/vms" => (30, 60),
        "/api/admin/send-notification" => (20, 60),
        "/api/admin/unsend-notification" => (20, 60),
        "/api/admin/blog-contributors" => (20, 60),
        "/api/admin/blog-deletions" => (20, 60),
        "/api/blog/posts" => (30, 60),
        "/api/blog/write" => (6, 60),
        "/api/blog/comment" => (10, 60),
        "/api/blog/upload" => (10, 60),
        "/api/backgrounds/upload" => (6, 120),
        "/api/backgrounds/delete" => (20, 60),
        "/api/dm/attachment/upload" => (20, 60),
        "/api/dm/attachments/delete" => (30, 60),
        "/api/blog/subscription" => (20, 60),
        "/api/newsletter-signup" => (3, 60),
        "/api/newsletter/unsubscribe-direct" => (10, 600),
        "/api/invite/send" => (5, 3600),
        "/api/invite/set-code" => (3, 60),
        "/api/apply" => (2, 3600),
        "/api/suggest" => (3, 60),
        "/api/migrateid" => (20, 60),
        "/api/vpn-check" => (30, 60),
        "/api/me/coins" => (30, 60),
        "/api/daily-login/state" => (120, 60),
        "/api/daily-login/claim" => (30, 60),
        "/api/puzzles/claim" => (30, 60),
        "/api/puzzles/list" => (60, 60),
        "/api/me/logout-other" => (5, 60),
        "/api/leaderboard" => (10, 60),
        "/api/friends/list" => (30, 60),
        "/api/friends/request" => (10, 60),
        "/api/friends/request/cancel" => (15, 60),
        "/api/friends/requests/pending" => (30, 60),
        "/api/friends/request/respond" => (15, 60),
        "/api/friends/remove" => (10, 60),
        "/api/profile/report" => (5, 300),
        "/api/admin/profile-reports/resolve" => (20, 60),
        "/api/presence/heartbeat" => (60, 60),
        "/api/premium-chat/history" => (60, 60),
        "/api/premium-chat/send" => (3, 10),
        "/api/public-chat/history" => (60, 60),
        "/api/public-chat/send" => (3, 10),
        "/api/pickle-chat/history" => (60, 60),
        "/api/pickle-chat/send" => (3, 10),
        "/api/pickle-chat/react" => (30, 10),
        "/api/pickle-chat/presence" => (10, 60),
        "/api/pickle-club/vote" => (5, 60),
        "/api/pickle-club/crunch" => (5, 60),
        "/api/pickle-club/membership" => (60, 60),
        "/api/pickle-club/join" => (3, 300),
        "/api/pickle-club/applicants" => (30, 60),
        "/api/pickle-club/decide" => (30, 60),
        "/api/pickle-club/owners" => (30, 60),
        "/api/pickle-bulletin/state" => (120, 60),
        "/api/pickle-bulletin/post" => (5, 300),
        "/api/pickle-bulletin/react" => (30, 10),
        "/api/pickle-bulletin/delete" => (10, 60),
        "/api/dm/send" => (20, 10),
        "/api/marketplace/list" => (1, 30),
        "/api/marketplace/buy" => (1, 30),
        "/api/marketplace/cancel" => (2, 30),
        "/api/marketplace/mediate" => (1, 30),
        "/api/marketplace/appeal" => (1, 30),
        "/api/marketplace/items" => (60, 60),
        "/api/chess/puzzle-solved" => (15, 3600),
        "/api/claim-sebastians-reward" => (10, 60),
        "/api/games/sebastians-piccolo/payout" => (10, 60),
        "/api/games/lillians-logic/solve" => (10, 60),
        "/api/chess-vs/challenge" => (5, 600),
        "/api/chess-vs/move" => (60, 60),
        "/api/canvas/pixel" => (1000, 60),
        "/api/canvas/pixels/bulk" => (1000, 60),
        "/api/canvas/history" => (10, 60),
        "/api/battleship/challenge" => (5, 600),
        "/api/battleship/respond" => (10, 60),
        "/api/battleship/place" => (5, 60),
        "/api/battleship/fire" => (60, 60),
        "/api/battleship/state" => (60, 60),
        "/api/battleship/resign" => (5, 60),
        "/api/jeopardy/create" => (5, 600),
        "/api/jeopardy/join" => (10, 60),
        "/api/jeopardy/start" => (5, 60),
        "/api/jeopardy/select" => (30, 60),
        "/api/jeopardy/buzz" => (30, 60),
        "/api/jeopardy/answer" => (30, 60),
        "/api/jeopardy/wager" => (10, 60),
        "/api/jeopardy/visibility" => (120, 60),
        "/api/jeopardy/state" => (300, 60),
        "/api/jeopardy/final/wager" => (10, 60),
        "/api/jeopardy/final/answer" => (10, 60),
        // server.js:1618-1626 — the /api/vm RATE_LIMITS rows. The JS applies
        // them through the global gate (the per-route checkRateLimit calls in
        // the vm block are dead: the global gate sets _rateLimitChecked
        // first), which keys on the full pathname — so sub-paths like
        // /api/vm/computers/<id>/power fall to the default and only the flat
        // paths below see these numbers.
        "/api/vm/computers" => (60, 60),
        "/api/vm/power" => (6, 60),
        "/api/vm/desktop-session" => (12, 60),
        "/api/vm/extend" => (10, 60),
        "/api/vm/heartbeat" => (120, 60),
        "/api/vm/my-computer/create" => (5, 60),
        "/api/vm/my-computer/recreate" => (5, 60),
        "/api/vm/upgrades" => (30, 60),
        "/api/vm/upgrade" => (15, 60),
        _ => (100, 60), // __default__
    }
}

/// In-memory `rlLog` — sliding windows of float-epoch timestamps keyed by
/// `<rateKey>::<endpoint>`. Mirrors the JS module-level map.
#[derive(Default)]
pub struct RateLimiter {
    log: std::sync::Mutex<HashMap<String, Vec<f64>>>,
    /// timing::<key> -> (lastTime, intervals) — detectNonHumanTiming state.
    timing_log: std::sync::Mutex<HashMap<String, (f64, Vec<f64>)>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// `rateLimited(rateKey, endpoint)` — true when over the limit.
    pub fn rate_limited(&self, rate_key: &str, endpoint: &str) -> bool {
        let (max_req, window) = rate_limit_for(endpoint);
        let limit = if rate_key == "anon" {
            (max_req / 5).max(1)
        } else {
            max_req
        } as f64;
        let key = format!("{rate_key}::{endpoint}");
        let now = now_millis() as f64 / 1000.0;
        let mut log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        let ts = log.entry(key).or_default();
        ts.retain(|t| now - t < window as f64);
        if ts.len() as f64 >= limit {
            return true;
        }
        ts.push(now);
        false
    }

    /// `detectNonHumanTiming(key)` — >=4 intervals within the last 10s each
    /// and spread < 50ms. Returns true when flagged.
    pub fn detect_non_human_timing(&self, key: &str) -> bool {
        const MAX_GAP_MS: i64 = 10_000;
        const KEEP: usize = 5;
        const MIN_INTERVALS: usize = 4;
        const SPREAD_MS: i64 = 50;
        let now = now_millis();
        let mut timing_log = self.timing_log.lock().unwrap_or_else(|e| e.into_inner());
        let (last_time, intervals) = timing_log
            .entry(format!("timing::{key}"))
            .or_insert((-1.0, Vec::new()));
        let diff = now - *last_time as i64;
        *last_time = now as f64;
        if *last_time as i64 == now && diff < 0 {
            return false;
        }
        if diff > MAX_GAP_MS {
            intervals.clear();
            return false;
        }
        intervals.push(diff as f64);
        if intervals.len() > KEEP {
            intervals.remove(0);
        }
        if intervals.len() >= MIN_INTERVALS {
            let min = intervals.iter().cloned().fold(f64::INFINITY, f64::min);
            let max = intervals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            return max - min < SPREAD_MS as f64;
        }
        false
    }

    /// Per-endpoint rlLog stats for `/api/admin/data` dtype `rl_list` —
    /// `{endpoint: {keys, max_hits}}` keyed by `key::endpoint` suffix.
    pub fn rl_endpoints(&self) -> Vec<(String, usize, usize)> {
        let log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<(String, usize, usize)> = Vec::new();
        for (key, ts) in log.iter() {
            if ts.is_empty() {
                continue;
            }
            let ep = key.split("::").skip(1).collect::<Vec<_>>().join("::");
            let entry = out.iter_mut().find(|(e, _, _)| *e == ep);
            match entry {
                Some((_, keys, max_hits)) => {
                    *keys += 1;
                    *max_hits = (*max_hits).max(ts.len());
                }
                None => out.push((ep, 1, ts.len())),
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// dtype `rl_reset` — drop every key for one endpoint.
    pub fn rl_reset_endpoint(&self, endpoint: &str) {
        let suffix = format!("::{endpoint}");
        let mut log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        log.retain(|k, _| !k.ends_with(&suffix));
    }

    /// dtype `rl_reset_all` / `reset-ratelimit` support.
    pub fn rl_clear(&self) {
        self.log.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// server.js:3477-3483 — the 5-minute rlLog sweeper: keep only hits from
    /// the last hour (cutoff = now_secs − 3600) and drop keys left empty.
    /// The timing table is deliberately untouched (the JS never sweeps it).
    pub fn sweep_log(&self) {
        let cutoff = now_millis() as f64 / 1000.0 - 3600.0;
        let mut log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        log.retain(|_, ts| {
            ts.retain(|t| *t > cutoff);
            !ts.is_empty()
        });
    }

    /// `/api/admin/reset-ratelimit` — remove keys ending in `::<ep>` for any
    /// of `endpoints`; returns the number cleared.
    pub fn rl_reset_many(&self, endpoints: &[String]) -> usize {
        let mut log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        let before = log.len();
        log.retain(|k, _| !endpoints.iter().any(|ep| k.ends_with(&format!("::{ep}"))));
        before - log.len()
    }
}

/// `checkRateLimit` — the full per-request gate. `id_key` is the cookie-derived
/// bucket key (`id:<sid>` or `anon`), computed by the caller post-auth.
pub fn check_rate_limit(
    limiter: &RateLimiter,
    ip: &str,
    id_key: &str,
    endpoint: &str,
) -> Option<(u16, &'static str)> {
    if WHITELISTED_IPS.contains(&ip) {
        return None;
    }
    if limiter.rate_limited(&format!("ip:{ip}"), endpoint) || limiter.rate_limited(id_key, endpoint)
    {
        return Some((429, "Too many requests, slow down"));
    }
    None
}

// ── Admin role resolution ────────────────────────────────────────────────────
// Port of server.js:6160-6260. Shape (corrected by exploring the live data):
// admins.json is a LIST OF EMAILS PER ROLE — `{owners: string[], admins:
// string[], coOwners?: string[]}` — NOT `{email: {rank}}`. moderators.json is
// a plain JSON array of emails. The hierarchy is flat, not cumulative:
// `isAdminEmail` covers owners+co-owners+config-admins; moderators are a
// DISJOINT set. `isOwnerEmail` grants co-owners full owner privileges.

/// `SITE_CO_OWNER_EMAILS` (server.js:6215) — hardcoded, not data-driven.
pub const SITE_CO_OWNER_EMAIL: &str = "tyler.thompson1@student.rjuhsd.us";

/// `loadAdminConfig()` (server.js:6211) — `{owners, admins, coOwners?}` with
/// the JS defaults applied when a key is missing. Re-read on every call.
pub struct AdminConfig {
    pub owners: Vec<String>,
    pub admins: Vec<String>,
    pub co_owners: Vec<String>,
}

pub fn load_admin_config(store: &DataStore) -> AdminConfig {
    let val = store.read_document(&store.base_dir.join("data/admins.json"), json!({}));
    let str_list = |key: &str, default: Vec<String>| -> Vec<String> {
        val.get(key)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or(default)
    };
    AdminConfig {
        owners: str_list("owners", vec!["admin@mitch.pro".to_owned()]),
        admins: str_list("admins", Vec::new()),
        co_owners: str_list("coOwners", Vec::new()),
    }
}

/// `adminMemberEmails()` (6219).
pub fn admin_member_emails(store: &DataStore) -> Vec<String> {
    load_admin_config(store).admins
}

/// `ownerMemberEmails()` (6223) — falls back to `['admin@mitch.pro']`.
pub fn owner_member_emails(store: &DataStore) -> Vec<String> {
    load_admin_config(store).owners
}

/// `coOwnerMemberEmails()` (6227) — hardcoded set ∪ configured, deduped.
pub fn co_owner_member_emails(store: &DataStore) -> Vec<String> {
    let mut out = vec![SITE_CO_OWNER_EMAIL.to_owned()];
    out.extend(
        load_admin_config(store)
            .co_owners
            .into_iter()
            .filter(|e| !e.is_empty()),
    );
    dedup_normalized(out)
}

/// `moderatorEmails()` (6160) — `loadJson(MODERATORS_FILE, [])`, a bare array.
pub fn moderator_emails(store: &DataStore) -> Vec<String> {
    store
        .read_document(&store.base_dir.join("data/moderators.json"), json!([]))
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// `siteAdminEmails()` (6242) — owners ∪ co-owners ∪ admins, normalized, deduped.
pub fn site_admin_emails(store: &DataStore) -> Vec<String> {
    let mut all = owner_member_emails(store);
    all.extend(co_owner_member_emails(store));
    all.extend(admin_member_emails(store));
    dedup_normalized(all)
}

fn dedup_normalized(emails: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for e in emails {
        let norm = normalize_email(&e);
        if norm.is_empty() || !seen.insert(norm.clone()) {
            continue;
        }
        out.push(norm);
    }
    out
}

/// `isCoOwnerEmail(email)` (6232).
pub fn is_co_owner_email(store: &DataStore, email: &str) -> bool {
    if email.is_empty() {
        return false;
    }
    let norm = normalize_email(email);
    co_owner_member_emails(store).contains(&norm)
}

/// `isOwnerEmail(email)` (6236) — co-owners get full owner privileges.
pub fn is_owner_email(store: &DataStore, email: &str) -> bool {
    if email.is_empty() {
        return false;
    }
    let norm = normalize_email(email);
    is_co_owner_email(store, &norm)
        || owner_member_emails(store)
            .iter()
            .any(|o| normalize_email(o) == norm)
}

/// `isAdminEmail(email)` (6246) — dev-test backdoor, then site-admin membership.
pub fn is_admin_email(store: &DataStore, email: &str) -> bool {
    if email.is_empty() {
        return false;
    }
    let norm = normalize_email(email);
    if dev_test_access_enabled() && norm == DEV_TEST_EMAIL {
        return true;
    }
    site_admin_emails(store).contains(&norm)
}

/// `isModeratorEmail(email)` (6164) — dev-test backdoor, then bare-array scan.
pub fn is_moderator_email(store: &DataStore, email: &str) -> bool {
    if email.is_empty() {
        return false;
    }
    let norm = normalize_email(email);
    if dev_test_access_enabled() && norm == DEV_TEST_EMAIL {
        return true;
    }
    moderator_emails(store)
        .iter()
        .any(|m| normalize_email(m) == norm)
}

/// `isTesterEmail(email)` (server.js:6525) — testers list + admin membership.
pub fn is_tester_email(store: &DataStore, email: &str) -> bool {
    if email.is_empty() {
        return false;
    }
    let norm = normalize_email(email);
    if is_admin_email(store, &norm) {
        return true;
    }
    let raw = store.read_document(&store.base_dir.join("data/testers.json"), json!([]));
    if let Some(arr) = raw.as_array() {
        return arr.iter().any(|v| {
            v.as_str()
                .map(|s| normalize_email(s) == norm)
                .unwrap_or(false)
        });
    }
    if let Some(obj) = raw.as_object() {
        return obj.keys().any(|k| normalize_email(k) == norm);
    }
    false
}

/// `isModeratorId(sid)` (6171) — sid → email → isModeratorEmail.
pub fn is_moderator_id(store: &DataStore, id_secret: &[u8], sid: &str) -> bool {
    if sid.is_empty() {
        return false;
    }
    email_from_sid(store, id_secret, sid)
        .map(|email| is_moderator_email(store, &email))
        .unwrap_or(false)
}

/// `isOwnerId(sid)` — sid → email → isOwnerEmail (used by owner-only routes).
pub fn is_owner_id(store: &DataStore, id_secret: &[u8], sid: &str) -> bool {
    if sid.is_empty() {
        return false;
    }
    email_from_sid(store, id_secret, sid)
        .map(|email| is_owner_email(store, &email))
        .unwrap_or(false)
}

/// `isAdminId(sid)` (server.js:6181) — three stages in exact JS order:
/// 1. NODE_ENV=test backdoor: `emailFromSid(sid)` normalizes to admin@mitch.pro
///    (no admins.json read — a synthetic test session resolves via names.json).
/// 2. names.json path: if `names[sid]` is a site admin, verify the sid is the
///    CURRENT generation id for that email; return true/false WITHOUT falling
///    through to stage 3 (a stale/rotated sid fails outright).
/// 3. Infinite-token path: any `infinite` token whose normalized email is a
///    site admin, with `gen = claim_count || 0`.
///
/// Note: no validId() pre-gate — the makeEmailId comparisons enforce it.
pub fn is_admin_id(store: &DataStore, id_secret: &[u8], sid: &str, node_env_test: bool) -> bool {
    if sid.is_empty() {
        return false;
    }
    if node_env_test {
        if let Some(email) = email_from_sid(store, id_secret, sid) {
            if normalize_email(&email) == DEV_TEST_EMAIL {
                return true;
            }
        }
    }
    // names.json path (generation-bound; no fallthrough on mismatch).
    let names = store.read_document(&store.base_dir.join(names_file()), json!({}));
    if let Some(email) = names.get(sid).and_then(|v| v.as_str()) {
        let norm = normalize_email(email);
        if !site_admin_emails(store).contains(&norm) {
            return false;
        }
        let gen = current_session_generation(store, &norm);
        return sid == make_email_id(&norm, gen as u64, id_secret)
            || sid == make_email_id(email, gen as u64, id_secret);
    }
    // Infinite-token path.
    let tokens = store.read_document(&store.base_dir.join("data/tokens.json"), json!({}));
    let admin_norms: std::collections::HashSet<String> =
        site_admin_emails(store).into_iter().collect();
    for rec in tokens
        .as_object()
        .map(|m| m.values().collect::<Vec<_>>())
        .unwrap_or_default()
    {
        if !rec
            .get("infinite")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        let norm = rec
            .get("norm_email")
            .or_else(|| rec.get("email"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_default();
        let norm = normalize_email(&norm);
        if !admin_norms.contains(&norm) {
            continue;
        }
        let gen = rec.get("claim_count").and_then(|v| v.as_i64()).unwrap_or(0);
        if sid == make_email_id(&norm, gen as u64, id_secret) {
            return true;
        }
    }
    false
}

/// `isAnyAdminId(sid)` (6177) — admin (incl. owner/co-owner) OR moderator.
pub fn is_any_admin_id(
    store: &DataStore,
    id_secret: &[u8],
    sid: &str,
    node_env_test: bool,
) -> bool {
    is_admin_id(store, id_secret, sid, node_env_test) || is_moderator_id(store, id_secret, sid)
}

/// `isPremiumEmail(email)` (6147) — admin/moderator OR an approved premium
/// application in applications.json.
pub fn is_premium_email(store: &DataStore, email: &str) -> bool {
    if email.is_empty() {
        return false;
    }
    if is_admin_email(store, email) || is_moderator_email(store, email) {
        return true;
    }
    let norm = normalize_email(email);
    let applications =
        store.read_document(&store.base_dir.join("data/applications.json"), json!([]));
    applications
        .as_array()
        .map(|a| {
            a.iter().any(|app| {
                normalize_email(app.get("email").and_then(|v| v.as_str()).unwrap_or("")) == norm
                    && app.get("status").and_then(|v| v.as_str()) == Some("approved")
                    && (app.get("type").and_then(|v| v.as_str()) == Some("premium")
                        || app.get("grantPremium").and_then(|v| v.as_bool()) == Some(true))
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_store(tag: &str) -> (PathBuf, Arc<DataStore>) {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-auth-test-{tag}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        let store = DataStore::open(&base, &base.join("data")).unwrap();
        (base.clone(), Arc::new(store))
    }

    fn secret() -> Vec<u8> {
        b"test-id-secret-bytes-0123456789".to_vec()
    }

    #[test]
    fn role_resolution_matches_js_shape() {
        // admins.json is a LIST OF EMAILS PER ROLE (verified against live data),
        // NOT {email: {rank}}: {owners: [], admins: [], coOwners?: []}.
        let (base, store) = temp_store("roles");
        store
            .write_document(
                &base.join("data/admins.json"),
                &json!({
                    "owners": ["owner@mitch.pro", "co-owner@mitch.pro"],
                    "coOwners": ["configured-coowner@mitch.pro"],
                    "admins": ["admin@mitch.pro"],
                }),
            )
            .unwrap();
        store
            .write_document(
                &base.join("data/moderators.json"),
                &json!(["mod@mitch.pro"]),
            )
            .unwrap();

        // Co-owners get full owner privileges (isOwnerEmail includes isCoOwnerEmail).
        assert!(is_owner_email(&store, "co-owner@mitch.pro"));
        assert!(is_co_owner_email(&store, "configured-coowner@mitch.pro"));
        assert!(
            is_co_owner_email(&store, SITE_CO_OWNER_EMAIL),
            "hardcoded co-owner"
        );
        assert!(is_owner_email(&store, "configured-coowner@mitch.pro"));
        assert!(is_owner_email(&store, "owner@mitch.pro"));
        // Plain config admins: admin yes, owner no.
        assert!(is_admin_email(&store, "admin@mitch.pro"));
        assert!(!is_owner_email(&store, "admin@mitch.pro"));
        // Moderators are a DISJOINT set, not cumulative with admin.
        assert!(is_moderator_email(&store, "mod@mitch.pro"));
        assert!(!is_admin_email(&store, "mod@mitch.pro"));
        assert!(!is_moderator_email(&store, "admin@mitch.pro"));
        // siteAdminEmails = owners ∪ co-owners ∪ admins.
        let site = site_admin_emails(&store);
        for email in [
            "owner@mitch.pro",
            "co-owner@mitch.pro",
            "configured-coowner@mitch.pro",
            SITE_CO_OWNER_EMAIL,
            "admin@mitch.pro",
        ] {
            assert!(site.contains(&normalize_email(email)), "{email} missing");
        }
        // Defaults when admins.json is absent: owners default to admin@mitch.pro.
        let (empty_base, empty_store) = temp_store("roles-empty");
        assert!(is_owner_email(&empty_store, "admin@mitch.pro"));
        assert!(!is_admin_email(&empty_store, "random@mitch.pro"));
        std::fs::remove_dir_all(base).ok();
        std::fs::remove_dir_all(empty_base).ok();
    }

    #[test]
    fn is_admin_id_requires_current_generation() {
        // names.json path: the sid must be the CURRENT generation id — a stale
        // generation fails outright without falling through to the token path.
        let id_secret = secret();
        let (base, store) = temp_store("roles-gen");
        let email = "admin@mitch.pro";
        let norm = normalize_email(email);
        store
            .write_document(&base.join("data/names.json"), &json!({}))
            .unwrap();
        store
            .write_document(
                &base.join("data/admins.json"),
                &json!({ "owners": [email] }),
            )
            .unwrap();
        // generations.json shape: {normEmail: {gen} | number}.
        store
            .write_document(
                &base.join("data/generations.json"),
                &json!({ "admin@mitch.pro": { "gen": 2 } }),
            )
            .unwrap();

        let stale = make_email_id(email, 0, &id_secret);
        let current = make_email_id(email, 2, &id_secret);
        let norm_current = make_email_id(&norm, 2, &id_secret);
        store
            .write_document(
                &base.join("data/names.json"),
                &json!({ stale.clone(): email, current.clone(): email }),
            )
            .unwrap();
        // Stale generation: admin email but wrong gen → false, no fallthrough.
        assert!(!is_admin_id(&store, &id_secret, &stale, false));
        // Current generation via raw email and via normalized email both pass.
        assert!(is_admin_id(&store, &id_secret, &current, false));
        assert!(is_admin_id(&store, &id_secret, &norm_current, false));
        assert!(is_any_admin_id(&store, &id_secret, &current, false));
        // A non-admin name with a valid generation is still not an admin.
        let rando = make_email_id("rando@mitch.pro", 0, &id_secret);
        store
            .write_document(
                &base.join("data/names.json"),
                &json!({ rando.clone(): "rando@mitch.pro" }),
            )
            .unwrap();
        assert!(!is_admin_id(&store, &id_secret, &rando, false));
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn is_admin_id_infinite_token_path() {
        let id_secret = secret();
        let (base, store) = temp_store("roles-token");
        let email = "owner@mitch.pro";
        let norm = normalize_email(email);
        store
            .write_document(&base.join("data/names.json"), &json!({}))
            .unwrap();
        store
            .write_document(
                &base.join("data/admins.json"),
                &json!({ "owners": [email] }),
            )
            .unwrap();
        // Infinite token claimed once: gen = claim_count = 1.
        let claimed = make_email_id(&norm, 1, &id_secret);
        store
            .write_document(
                &base.join("data/tokens.json"),
                &json!({ "tok123": {
                    "email": email,
                    "norm_email": norm,
                    "infinite": true,
                    "claim_count": 1,
                }}),
            )
            .unwrap();
        assert!(is_admin_id(&store, &id_secret, &claimed, false));
        // Wrong generation fails.
        assert!(!is_admin_id(
            &store,
            &id_secret,
            &make_email_id(&norm, 0, &id_secret),
            false
        ));
        // Non-infinite tokens are skipped.
        store
            .write_document(
                &base.join("data/tokens.json"),
                &json!({ "tok123": {
                    "email": email,
                    "norm_email": norm,
                    "infinite": false,
                    "claim_count": 1,
                }}),
            )
            .unwrap();
        assert!(!is_admin_id(&store, &id_secret, &claimed, false));
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn is_premium_email_ladder() {
        let (base, store) = temp_store("roles-premium");
        store
            .write_document(
                &base.join("data/admins.json"),
                &json!({ "owners": ["owner@mitch.pro"] }),
            )
            .unwrap();
        // Admin → premium.
        assert!(is_premium_email(&store, "owner@mitch.pro"));
        // Approved premium application → premium.
        store
            .write_document(
                &base.join("data/applications.json"),
                &json!([
                    {"email": "paid@mitch.pro", "status": "approved", "type": "premium"},
                    {"email": "pending@mitch.pro", "status": "pending", "type": "premium"},
                    {"email": "rejected@mitch.pro", "status": "approved", "type": "free"},
                    {"email": "granted@mitch.pro", "status": "approved", "grantPremium": true},
                ]),
            )
            .unwrap();
        assert!(is_premium_email(&store, "paid@mitch.pro"));
        assert!(!is_premium_email(&store, "pending@mitch.pro"));
        assert!(!is_premium_email(&store, "rejected@mitch.pro"));
        assert!(is_premium_email(&store, "granted@mitch.pro"));
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn normalize_email_matches_js() {
        assert_eq!(normalize_email("A.B+x@mitch.pro"), "ab@student.rjuhsd.us");
        assert_eq!(
            normalize_email("a.b+x@student.mitch.pro"),
            "ab@student.rjuhsd.us"
        );
        // Reserved locals keep their domain.
        assert_eq!(normalize_email("admin@mitch.pro"), "admin@mitch.pro");
        assert_eq!(normalize_email("support@mitch.pro"), "support@mitch.pro");
        assert_eq!(normalize_email("noreply@mitch.pro"), "noreply@mitch.pro");
        assert_eq!(normalize_email("mitch@mitch.pro"), "mitch@mitch.pro");
        // Non-mitch domains pass through.
        assert_eq!(normalize_email("A.B+x@gmail.com"), "ab@gmail.com");
        // No @: lowercased/trimmed only.
        assert_eq!(normalize_email("  MiXeD  "), "mixed");
        assert_eq!(normalize_email(""), "");
    }

    #[test]
    fn make_email_id_and_valid_id_round_trip() {
        let secret = secret();
        let id = make_email_id("ab@student.rjuhsd.us", 0, &secret);
        // Shape: e + 24 hex chars, dot, 16 hex chars.
        assert_eq!(id.len(), 42);
        assert!(id.starts_with('e'));
        assert!(valid_id(&id, &secret));
        // gen > 0 keys with :vN.
        let id_gen3 = make_email_id("ab@student.rjuhsd.us", 3, &secret);
        assert_ne!(id, id_gen3);
        assert!(valid_id(&id_gen3, &secret));
        // Wrong secret fails.
        assert!(!valid_id(&id, b"other-secret"));
        // Garbage fails.
        assert!(!valid_id("no-dot", &secret));
        assert!(!valid_id("", &secret));
        assert!(!valid_id("e123.sig", &secret));
    }

    #[test]
    fn rate_limiter_sliding_window_and_anon_fifth() {
        // A path NOT in RATE_LIMITS gets the __default__ [100, 60].
        const UNLISTED: &str = "/api/definitely-not-listed-xyz";
        let rl = RateLimiter::new();
        for _ in 0..100 {
            assert!(!rl.rate_limited("ip:1.2.3.4", UNLISTED));
        }
        assert!(rl.rate_limited("ip:1.2.3.4", UNLISTED));
        // Anon bucket is a fifth: 20 for the default.
        let rl2 = RateLimiter::new();
        for _ in 0..20 {
            assert!(!rl2.rate_limited("anon", UNLISTED));
        }
        assert!(rl2.rate_limited("anon", UNLISTED));
        // Listed endpoints use their own window: /api/dm/send is [20, 10].
        let rl3 = RateLimiter::new();
        for _ in 0..20 {
            assert!(!rl3.rate_limited("ip:1.2.3.4", "/api/dm/send"));
        }
        assert!(rl3.rate_limited("ip:1.2.3.4", "/api/dm/send"));
        // Different endpoints have independent buckets.
        assert!(!rl3.rate_limited("ip:1.2.3.4", "/api/ping"));
    }

    #[test]
    fn timing_detection_flags_regular_intervals() {
        let rl = RateLimiter::new();
        let key = "1.2.3.4:/api/login";
        assert!(!rl.detect_non_human_timing(key));
        assert!(!rl.detect_non_human_timing(key));
        assert!(!rl.detect_non_human_timing(key));
        assert!(!rl.detect_non_human_timing(key));
        // 5th request with tight intervals triggers.
        assert!(rl.detect_non_human_timing(key));
    }

    #[tokio::test]
    async fn check_password_cookie_full_gate() {
        let (base, store) = temp_store("cpc");
        let secret = secret();
        let norm = "ab@student.rjuhsd.us";
        let sid = make_email_id(norm, 0, &secret);

        // Seed names + passwords via the store (DB-backed paths).
        let mut names = serde_json::Map::new();
        names.insert(sid.clone(), serde_json::json!("a.b+x@mitch.pro"));
        store
            .write_document(
                &base.join("data/names.json"),
                &serde_json::Value::Object(names),
            )
            .unwrap();
        let mut passwords = serde_json::Map::new();
        passwords.insert(norm.to_string(), serde_json::json!("argon2id$hash"));
        store
            .write_document(
                &base.join("data/passwords.json"),
                &serde_json::Value::Object(passwords),
            )
            .unwrap();

        // No cookies -> false.
        let empty = Cookies::default();
        assert!(!check_password_cookie(
            &store, &secret, &empty, None, false, false
        ));

        // With a valid session cookie (simulating getCookies' derivation):
        let mut jar = Cookies::default();
        jar.map.insert("studentId".into(), sid.clone());
        assert!(check_password_cookie(
            &store, &secret, &jar, None, false, false
        ));

        // Provided-sid mismatch fails.
        assert!(!check_password_cookie(
            &store,
            &secret,
            &jar,
            Some("other"),
            false,
            false
        ));

        // Invalid sid fails.
        let mut bad = Cookies::default();
        bad.map.insert("studentId".into(), "garbage".into());
        assert!(!check_password_cookie(
            &store, &secret, &bad, None, false, false
        ));

        // NODE_ENV=test bypass: fires after sid + email resolution, so an
        // empty cookie jar still returns false (parity with the JS order).
        assert!(!check_password_cookie(
            &store, &secret, &empty, None, true, false
        ));
        // But a valid sid with NO password entry passes under test mode.
        let mut no_password = Cookies::default();
        no_password.map.insert("studentId".into(), sid.clone());
        assert!(check_password_cookie(
            &store,
            &secret,
            &no_password,
            None,
            true,
            false
        ));
        std::fs::remove_dir_all(base).ok();
    }
}
