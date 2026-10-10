//! `/api/e2e/*` — the E2E-DM registration/relay surface (plan Step 11).
//!
//! Eight endpoints around the two in-memory maps `e2eUsers`/`e2eMessages`
//! (state.rs): get-key (read a registered public key + sealed private JWK),
//! join (mint a per-session server keypair), heartbeat, register-key
//! (create/rotate the account's key with a 5-deep history), verify-password,
//! send (relay ciphertext into `e2eMessages`), users (live roster), messages
//! (pull a conversation).
//!
//! Method-check parity with the JS: `join`, `heartbeat` and `send` have NO
//! method check (any verb runs the ladder — a GET parses the empty body to
//! `{}` and proceeds); get-key is GET-only, register-key/verify-password are
//! POST-only, each with an explicit `method ===` check and wrong verbs
//! falling through to the unclaimed-path 405; users/messages live INSIDE the
//! JS `if (method === 'GET')` branch (server.js:20455/20466), so only GETs
//! reach them and any other verb falls through to the 405 as well.
//!
//! Two auth ladders, both replicated: get-key/register-key/verify-password
//! use `names[sid]` (lowercase trim) with the `Auth required` / `Email not
//! found` messages, while join/heartbeat/send/messages use `emailFromSid`.
//! Response shapes differ too — the former three speak
//! `{success, message}`, users/messages speak `{error}`.

use crate::routes::dm::{is_revoked_id, qs_get};
use crate::routes::me::{cookies_of, data_file, json_response};
use crate::state::{AppState, E2eMessage, E2eUser};
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::auth;
use mitch_lib::dm;
use mitch_lib::jsval::{self, truthy};
use mitch_lib::profile::display_email;
use mitch_lib::school::now_millis;
use serde_json::{json, Map, Value};
use std::sync::Arc;

/// Dispatches `/api/e2e/*` requests; `None` falls through to the next group.
pub(crate) async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
    search: &str,
) -> Option<Response> {
    match path {
        "/api/e2e/get-key" => {
            if *method == Method::GET {
                return Some(get_key(state, headers));
            }
            None
        }
        "/api/e2e/join" => Some(join(state, headers, body_bytes).await),
        "/api/e2e/heartbeat" => Some(heartbeat(state, headers, body_bytes)),
        "/api/e2e/register-key" => {
            if *method == Method::POST {
                return Some(register_key(state, headers, body_bytes));
            }
            None
        }
        "/api/e2e/verify-password" => {
            if *method == Method::POST {
                return Some(verify_password(state, headers, body_bytes));
            }
            None
        }
        "/api/e2e/send" => Some(send(state, headers, body_bytes)),
        "/api/e2e/users" => {
            if *method == Method::GET {
                return Some(users(state, headers));
            }
            None
        }
        "/api/e2e/messages" => {
            if *method == Method::GET {
                return Some(messages(state, headers, search));
            }
            None
        }
        _ => None,
    }
}

/// `sid = cookies['studentId'] || cookies['id'] || ''`.
fn sid_of(state: &AppState, headers: &HeaderMap) -> String {
    let cookies = cookies_of(state, headers);
    let sid = cookies.get("studentId").unwrap_or("");
    let sid = if sid.is_empty() {
        cookies.get("id").unwrap_or("")
    } else {
        sid
    };
    sid.to_string()
}

/// The get-key/register-key/verify-password ladder: 401 `Auth required`, then
/// the `names[sid]` lookup (lowercase + trim) → 403 `Email not found`.
/// Returns the raw (lowercased/trimmed) email.
fn names_email(state: &AppState, headers: &HeaderMap) -> Result<String, Box<Response>> {
    let sid = sid_of(state, headers);
    if sid.is_empty() || !auth::valid_id(&sid, &state.id_secret) || is_revoked_id(state, &sid) {
        return Err(Box::new(json_response(
            401,
            json!({ "success": false, "message": "Auth required" }),
        )));
    }
    let email = auth::names_get_email(&state.store, &sid)
        .map(|e| e.to_lowercase().trim().to_string())
        .unwrap_or_default();
    if email.is_empty() {
        return Err(Box::new(json_response(
            403,
            json!({ "success": false, "message": "Email not found" }),
        )));
    }
    Ok(email)
}

/// The join/heartbeat/send/messages ladder: `normalizeEmail(emailFromSid(sid)
/// || '')` → 401 `Auth required` when empty.
fn email_from_sid_norm(state: &AppState, headers: &HeaderMap) -> Result<String, Box<Response>> {
    let sid = sid_of(state, headers);
    if sid.is_empty() || !auth::valid_id(&sid, &state.id_secret) || is_revoked_id(state, &sid) {
        return Err(Box::new(json_response(
            401,
            json!({ "success": false, "message": "Auth required" }),
        )));
    }
    let email = auth::email_from_sid(&state.store, &state.id_secret, &sid)
        .map(|e| auth::normalize_email(&e))
        .unwrap_or_default();
    if email.is_empty() {
        return Err(Box::new(json_response(
            401,
            json!({ "success": false, "message": "Auth required" }),
        )));
    }
    Ok(email)
}

/// `tryParseJson()` — an EMPTY body parses to `{}` success (the JS
/// `body = raw ? JSON.parse(raw) : {}`), malformed JSON → None.
fn parse_body(body_bytes: &[u8]) -> Option<Value> {
    let raw = std::str::from_utf8(body_bytes).ok()?;
    if raw.is_empty() {
        return Some(json!({}));
    }
    serde_json::from_str(raw).ok()
}

// ── GET /api/e2e/get-key ─────────────────────────────────────────────────────

fn get_key(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let email = match names_email(state, headers) {
        Ok(e) => e,
        Err(resp) => return *resp,
    };
    let e2e_keys: Value = state
        .store
        .read_document(&data_file(state, "e2e_keys.json"), json!({}));
    let norm = auth::normalize_email(&email);
    let entry = e2e_keys.get(&norm).filter(|e| truthy(e));
    let Some(entry) = entry else {
        return json_response(404, json!({ "success": false, "message": "Not found" }));
    };
    let history: Vec<Value> = entry
        .get("history")
        .and_then(|h| h.as_array())
        .map(|h| h.iter().take(5).map(history_entry).collect())
        .unwrap_or_default();
    // Top-level response (server.js:13761-13771) — the JS object literal
    // order: success, then pubKeyHex/encryptedPrivateJwk/ivHex as raw
    // passthrough with the JSON undefined-drop (a MISSING stored key drops
    // the property, since `entry.pubKeyHex` is undefined), then
    // kdfSaltHex/kdfIterations always via `|| ''` / `|| 0`, then keyHistory.
    let mut out = Map::new();
    out.insert("success".to_string(), json!(true));
    for key in ["pubKeyHex", "encryptedPrivateJwk", "ivHex"] {
        if let Some(v) = entry.get(key) {
            out.insert(key.to_string(), v.clone());
        }
    }
    out.insert(
        "kdfSaltHex".to_string(),
        jsval::or(entry.get("kdfSaltHex"), json!("")),
    );
    out.insert(
        "kdfIterations".to_string(),
        jsval::or(entry.get("kdfIterations"), json!(0)),
    );
    out.insert("keyHistory".to_string(), json!(history));
    json_response(200, Value::Object(out))
}

/// One `keyHistory` entry — `kdfSaltHex: h.kdfSaltHex || ''` and
/// `kdfIterations: h.kdfIterations || 0` coalesce (present-null → '' / 0),
/// the rest pass through raw with the undefined-drop.
fn history_entry(h: &Value) -> Value {
    let mut out = Map::new();
    for key in ["pubKeyHex", "encryptedPrivateJwk", "ivHex"] {
        if let Some(v) = h.get(key) {
            out.insert(key.to_string(), v.clone());
        }
    }
    out.insert(
        "kdfSaltHex".to_string(),
        jsval::or(h.get("kdfSaltHex"), json!("")),
    );
    out.insert(
        "kdfIterations".to_string(),
        jsval::or(h.get("kdfIterations"), json!(0)),
    );
    if let Some(v) = h.get("updatedAt") {
        out.insert("updatedAt".to_string(), v.clone());
    }
    Value::Object(out)
}

// ── /api/e2e/join ────────────────────────────────────────────────────────────

async fn join(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let email = match email_from_sid_norm(state, headers) {
        Ok(e) => e,
        Err(resp) => return *resp,
    };
    let Some(body) = parse_body(body_bytes) else {
        return json_response(400, json!({ "success": false, "message": "bad json" }));
    };
    // `(body.nickname || '').replace(/[^a-zA-Z0-9_@._-]/g, '').slice(0, 50)`.
    let raw_nick = jsval::string(&jsval::or(body.get("nickname"), json!("")));
    let nick = jsval::js_slice_utf16(
        &raw_nick
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '@' | '.' | '-'))
            .collect::<String>(),
        50,
    );
    let canonical_nick = auth::normalize_email(&nick);
    let pub_key = jsval::string(&jsval::or(body.get("pubKey"), json!("")));
    for s in bad_nics(state) {
        if nick.to_lowercase().contains(&s) {
            return json_response(400, json!({ "success": false, "message": "Bad Name" }));
        }
    }
    if nick.is_empty()
        || canonical_nick != email
        || !dm::is_hex(&json!(pub_key), 130)
        || !pub_key.starts_with("04")
    {
        return json_response(
            400,
            json!({ "success": false, "message": "Invalid identity or key" }),
        );
    }
    let (priv_key, pub_hex) = mitch_lib::crypto::generate_p256_keypair();
    {
        let mut users = state.e2e_users.lock().unwrap_or_else(|e| e.into_inner());
        let rec = E2eUser {
            pub_key: pub_key.clone(),
            priv_key,
            server_pub_hex: pub_hex.clone(),
            last_seen: now_millis(),
            email: email.clone(),
        };
        // JS assignment keeps an existing key's insertion position.
        match users.iter_mut().find(|(k, _)| *k == canonical_nick) {
            Some((_, existing)) => *existing = rec,
            None => users.push((canonical_nick, rec)),
        }
    }
    // `maskEmail` is a pass-through now (server.js:1380-1384).
    json_response(
        200,
        json!({ "success": true, "nickname": email, "serverPubKey": pub_hex }),
    )
}

/// `BAD_NICS` (server.js:1634) — the builtin `'awgnagae'` plus every entry of
/// data/bad_words.json. The JS compares `nick.toLowerCase().includes(s)`
/// WITHOUT lowercasing s, so an uppercase word can never match — replicated.
fn bad_nics(state: &AppState) -> Vec<String> {
    let mut out = vec!["awgnagae".to_string()];
    let words: Value = state
        .store
        .read_document(&data_file(state, "bad_words.json"), json!([]));
    if let Some(arr) = words.as_array() {
        out.extend(arr.iter().filter_map(|w| w.as_str().map(str::to_string)));
    }
    out
}

// ── /api/e2e/heartbeat ───────────────────────────────────────────────────────

fn heartbeat(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let email = match email_from_sid_norm(state, headers) {
        Ok(e) => e,
        Err(resp) => return *resp,
    };
    // `if (!await tryParseJson()) return jsonResp(200, { success: true })` —
    // bad JSON still answers success; an EMPTY body parses to {} and the
    // identity gate then 403s.
    let Some(body) = parse_body(body_bytes) else {
        return json_response(200, json!({ "success": true }));
    };
    let nick = auth::normalize_email(&jsval::string(&jsval::or(body.get("nickname"), json!(""))));
    if nick != email {
        return json_response(
            403,
            json!({ "success": false, "message": "Identity mismatch" }),
        );
    }
    // `if (nick in e2eUsers && normalizeEmail(e2eUsers[nick].email) === email)`
    // — only then is last_seen stamped.
    let mut users = state.e2e_users.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, u)) = users.iter_mut().find(|(k, _)| *k == nick) {
        if auth::normalize_email(&u.email) == email {
            u.last_seen = now_millis();
        }
    }
    json_response(200, json!({ "success": true }))
}

// ── POST /api/e2e/register-key ───────────────────────────────────────────────

fn register_key(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let email = match names_email(state, headers) {
        Ok(e) => e,
        Err(resp) => return *resp,
    };
    let Some(body) = parse_body(body_bytes) else {
        return json_response(400, json!({ "success": false, "message": "bad json" }));
    };
    // `const { pubKeyHex, encryptedPrivateJwk, ivHex } = body` — destructured
    // RAW values (missing → undefined); the `!x` gate is JS truthiness on the
    // raw value, and the raw values (not their String() forms) get stored.
    let raw_pub = body.get("pubKeyHex").cloned().unwrap_or(Value::Null);
    let raw_enc = body
        .get("encryptedPrivateJwk")
        .cloned()
        .unwrap_or(Value::Null);
    let raw_iv = body.get("ivHex").cloned().unwrap_or(Value::Null);
    if !truthy(&raw_pub) || !truthy(&raw_enc) || !truthy(&raw_iv) {
        return json_response(
            400,
            json!({ "success": false, "message": "Missing fields" }),
        );
    }
    let pub_key_hex = jsval::string(&raw_pub);
    let encrypted = jsval::string(&raw_enc);
    let iv_hex = jsval::string(&raw_iv);
    if !regex_is_match(r"^04[0-9a-f]{128}$", &pub_key_hex, true)
        || !regex_is_match(r"^[0-9a-f]{24}$", &iv_hex, true)
        || !regex_is_match(r"^[0-9a-f]+$", &encrypted, true)
        || encrypted.chars().map(|c| c.len_utf16()).sum::<usize>() % 2 != 0
        || encrypted.chars().map(|c| c.len_utf16()).sum::<usize>() > 16384
    {
        return json_response(
            400,
            json!({ "success": false, "message": "Invalid key payload" }),
        );
    }
    // `String(body.kdfSaltHex || '').toLowerCase()`.
    let kdf_salt = jsval::string(&jsval::or(body.get("kdfSaltHex"), json!(""))).to_lowercase();
    if !kdf_salt.is_empty() && !regex_is_match(r"^[0-9a-f]{64}$", &kdf_salt, false) {
        return json_response(
            400,
            json!({ "success": false, "message": "Invalid kdf salt" }),
        );
    }
    // `parseInt(body.kdfIterations, 10) || 0` — String coercion, decimal
    // prefix parse, NaN → 0.
    let kdf_iters_raw = jsval::string(&jsval::or(body.get("kdfIterations"), Value::Null));
    let kdf_iterations = parse_int_or_zero(&kdf_iters_raw);
    if kdf_iterations != 0.0 && !(100_000.0..=2_000_000.0).contains(&kdf_iterations) {
        return json_response(
            400,
            json!({ "success": false, "message": "Invalid kdf iterations" }),
        );
    }

    let mut e2e_keys: Value = state
        .store
        .read_document(&data_file(state, "e2e_keys.json"), json!({}));
    let norm = auth::normalize_email(&email);
    let previous = e2e_keys.get(&norm).cloned().unwrap_or(Value::Null);
    // `body.createOnly === true` — strictly the boolean.
    if body.get("createOnly").and_then(|v| v.as_bool()) == Some(true)
        && truthy_prop(previous.get("encryptedPrivateJwk"))
    {
        return json_response(
            409,
            json!({
                "success": false,
                "code": "KEY_ALREADY_EXISTS",
                "message": "Secure Chat is already set up for this account."
            }),
        );
    }
    let mut history: Vec<Value> = previous
        .get("history")
        .and_then(|h| h.as_array())
        .map(|h| h.iter().take(4).cloned().collect())
        .unwrap_or_default();
    // The unshift gate: previous.pubKeyHex truthy && !== new (RAW compare) &&
    // previous.encryptedPrivateJwk && previous.ivHex truthy.
    if truthy_prop(previous.get("pubKeyHex"))
        && previous.get("pubKeyHex") != Some(&raw_pub)
        && truthy_prop(previous.get("encryptedPrivateJwk"))
        && truthy_prop(previous.get("ivHex"))
    {
        let mut prev_entry = Map::new();
        for key in ["pubKeyHex", "encryptedPrivateJwk", "ivHex"] {
            if let Some(v) = previous.get(key) {
                prev_entry.insert(key.to_string(), v.clone());
            }
        }
        prev_entry.insert(
            "kdfSaltHex".to_string(),
            jsval::or(previous.get("kdfSaltHex"), json!("")),
        );
        prev_entry.insert(
            "kdfIterations".to_string(),
            jsval::or(previous.get("kdfIterations"), json!(0)),
        );
        if let Some(v) = previous.get("updatedAt") {
            prev_entry.insert("updatedAt".to_string(), v.clone());
        }
        history.insert(0, Value::Object(prev_entry));
    }
    history.truncate(5);
    // The JS key order: pubKeyHex, encryptedPrivateJwk, ivHex, kdfSaltHex,
    // kdfIterations, updatedAt, history.
    if !e2e_keys.is_object() {
        e2e_keys = json!({});
    }
    if let Some(map) = e2e_keys.as_object_mut() {
        map.insert(
            norm,
            json!({
                "pubKeyHex": raw_pub,
                "encryptedPrivateJwk": raw_enc,
                "ivHex": raw_iv,
                "kdfSaltHex": kdf_salt,
                "kdfIterations": js_num_value(kdf_iterations),
                "updatedAt": now_millis(),
                "history": history,
            }),
        );
    }
    let _ = state
        .store
        .write_document(&data_file(state, "e2e_keys.json"), &e2e_keys);
    json_response(200, json!({ "success": true }))
}

/// Integral doubles serialize without a trailing `.0` (JSON.stringify of a
/// JS number).
fn js_num_value(n: f64) -> Value {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 9.007_199_254_740_992e15 {
        json!(n as i64)
    } else {
        json!(n)
    }
}

/// `v?.prop && …` — an optional-chain truthiness (missing/null → false).
fn truthy_prop(v: Option<&Value>) -> bool {
    v.map(truthy).unwrap_or(false)
}

/// `regex.test(...)`; the JS uses /i on the key regexes only.
fn regex_is_match(pattern: &str, s: &str, case_insensitive: bool) -> bool {
    let full = if case_insensitive {
        format!("(?i){pattern}")
    } else {
        pattern.to_string()
    };
    regex::Regex::new(&full)
        .map(|re| re.is_match(s))
        .unwrap_or(false)
}

/// `parseInt(s, 10) || 0` — decimal prefix parse ("600k" → 600, NaN → 0).
fn parse_int_or_zero(s: &str) -> f64 {
    let mut t = s.trim_start();
    let sign = if let Some(rest) = t.strip_prefix('-') {
        t = rest;
        -1.0
    } else {
        1.0
    };
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return 0.0;
    }
    sign * digits.parse::<f64>().unwrap_or(0.0)
}

// ── POST /api/e2e/verify-password ────────────────────────────────────────────

fn verify_password(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    // NOTE: bun calls `checkRateLimit(req, path)` explicitly here
    // (server.js:15479), but it is a NO-OP — the global /api/ prelude already
    // stamped `req._rateLimitChecked` (server.js:5987-5988), so the Rust port
    // must NOT call rate_limit_check a second time or it would double-count a
    // slot against the [5, 60] table entry. The global 3b gate (which derives
    // the same getIdKey bucket) already covers this request.
    let email = match names_email(state, headers) {
        Ok(e) => e,
        Err(resp) => return *resp,
    };
    let Some(body) = parse_body(body_bytes) else {
        return json_response(400, json!({ "success": false, "message": "bad json" }));
    };
    let password = jsval::string(&jsval::or(body.get("password"), json!("")));
    if password.is_empty() {
        return json_response(
            400,
            json!({ "success": false, "message": "Password required" }),
        );
    }
    let passwords: Value = state
        .store
        .read_document(&data_file(state, "passwords.json"), json!({}));
    let stored = passwords
        .get(auth::normalize_email(&email).as_str())
        .and_then(|v| v.as_str())
        .unwrap_or("");
    // `!stored || !(await Bun.password.verify(...))` → 401 Incorrect password.
    if stored.is_empty() || !mitch_lib::crypto::argon2_verify(stored, &password) {
        return json_response(
            401,
            json!({ "success": false, "message": "Incorrect password" }),
        );
    }
    json_response(200, json!({ "success": true }))
}

// ── /api/e2e/send ────────────────────────────────────────────────────────────

fn send(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let email = match email_from_sid_norm(state, headers) {
        Ok(e) => e,
        Err(resp) => return *resp,
    };
    let Some(body) = parse_body(body_bytes) else {
        return json_response(400, json!({ "success": false, "message": "bad json" }));
    };
    // `const { from: frm = '', to = '', data: msgData = '', iv = '' } = body`
    // — the destructuring default applies ONLY to missing keys; a JSON null
    // stays null and the truthiness gate 400s it, exactly like JS.
    let prop = |k: &str| -> Value { body.get(k).cloned().unwrap_or(json!("")) };
    let frm = prop("from");
    let to = prop("to");
    let msg_data = prop("data");
    let iv = prop("iv");
    if !truthy(&frm) || !truthy(&to) || !truthy(&msg_data) || !truthy(&iv) {
        return json_response(
            400,
            json!({ "success": false, "message": "Missing fields" }),
        );
    }
    let from_norm = auth::normalize_email(&jsval::string(&frm));
    let to_norm = auth::normalize_email(&jsval::string(&to));
    if from_norm != email {
        return json_response(
            403,
            json!({ "success": false, "message": "Identity mismatch" }),
        );
    }
    let registered = state
        .e2e_users
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|(k, u)| *k == from_norm && auth::normalize_email(&u.email) == email);
    if !registered {
        return json_response(
            401,
            json!({ "success": false, "message": "Not registered" }),
        );
    }
    let msg_data_str = jsval::string(&msg_data);
    // `String(msgData).length > 256000` — JS .length is UTF-16 code units.
    let utf16_len = msg_data_str.chars().map(|c| c.len_utf16()).sum::<usize>();
    if to_norm.is_empty() || utf16_len > 256_000 || !dm::is_hex(&json!(jsval::string(&iv)), 24) {
        return json_response(
            400,
            json!({ "success": false, "message": "Invalid message" }),
        );
    }
    let entry = E2eMessage {
        from: from_norm.clone(),
        to: to_norm.clone(),
        data: msg_data_str,
        iv: jsval::string(&iv),
        timestamp: now_millis(),
    };
    let k = e2e_key(&from_norm, &to_norm);
    let mut msgs = state.e2e_messages.lock().unwrap_or_else(|e| e.into_inner());
    let list = msgs.entry(k).or_default();
    list.push(entry);
    if list.len() > 500 {
        let cut = list.len() - 500;
        *list = list.split_off(cut);
    }
    json_response(200, json!({ "success": true }))
}

/// `e2eKey(a, b)` — `[a, b].sort().join(':')`, lexicographic default sort.
fn e2e_key(a: &str, b: &str) -> String {
    if a <= b {
        format!("{a}:{b}")
    } else {
        format!("{b}:{a}")
    }
}

// ── GET /api/e2e/users ───────────────────────────────────────────────────────

fn users(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let sid = sid_of(state, headers);
    if sid.is_empty() || !auth::valid_id(&sid, &state.id_secret) || is_revoked_id(state, &sid) {
        return json_response(401, json!({ "error": "auth required" }));
    }
    let now = now_millis();
    let users: Vec<Value> = state
        .e2e_users
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|(_, u)| now - u.last_seen < 60_000)
        .map(|(n, u)| {
            json!({
                "nickname": display_email(&state.store, state.data_dir(), &state.id_secret, n),
                "pubKey": u.pub_key.clone(),
            })
        })
        .collect();
    json_response(200, json!({ "users": users }))
}

// ── GET /api/e2e/messages ────────────────────────────────────────────────────

fn messages(state: &Arc<AppState>, headers: &HeaderMap, search: &str) -> Response {
    let sid = sid_of(state, headers);
    if sid.is_empty() || !auth::valid_id(&sid, &state.id_secret) || is_revoked_id(state, &sid) {
        return json_response(401, json!({ "error": "auth required" }));
    }
    let email = auth::email_from_sid(&state.store, &state.id_secret, &sid)
        .map(|e| auth::normalize_email(&e))
        .unwrap_or_default();
    let me = auth::normalize_email(&qs_get(search, "me").unwrap_or_default());
    let with_user = auth::normalize_email(&qs_get(search, "with").unwrap_or_default());
    // `parseInt(qs.get('since') || '0') || 0`.
    let since = parse_int_or_zero(&qs_get(search, "since").unwrap_or_default());
    if me.is_empty() || with_user.is_empty() {
        return json_response(400, json!({ "error": "Missing params" }));
    }
    if email.is_empty() || me != email {
        return json_response(403, json!({ "error": "identity mismatch" }));
    }
    let k = e2e_key(&me, &with_user);
    let msgs = state
        .e2e_messages
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&k)
        .cloned()
        .unwrap_or_default();
    let out: Vec<Value> = msgs
        .iter()
        .filter(|m| (m.timestamp as f64) > since)
        .map(|m| {
            json!({
                "from": m.from.clone(),
                "to": m.to.clone(),
                "data": m.data.clone(),
                "iv": m.iv.clone(),
                "timestamp": m.timestamp,
            })
        })
        .collect();
    json_response(200, json!({ "messages": out }))
}

// ── Sweeper (workers.rs drives it every 60s) ─────────────────────────────────

/// server.js:4044-4049 — entries with `last_seen < now - 5min` deleted.
pub(crate) fn sweep_e2e_users(state: &Arc<AppState>) {
    let cutoff = now_millis() - 5 * 60 * 1000;
    state
        .e2e_users
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|(_, u)| u.last_seen >= cutoff);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn e2e_key_matches_js_sort_join() {
        assert_eq!(e2e_key("b@x", "a@x"), "a@x:b@x");
        assert_eq!(e2e_key("a@x", "b@x"), "a@x:b@x");
        assert_eq!(e2e_key("a@x", "a@x"), "a@x:a@x");
    }

    #[test]
    fn js_num_value_serializes_integral_doubles() {
        assert_eq!(js_num_value(600_000.0), serde_json::json!(600_000));
        assert_eq!(js_num_value(0.0), serde_json::json!(0));
        assert_eq!(js_num_value(1.5), serde_json::json!(1.5));
    }

    #[test]
    fn parse_int_matches_js_parseint_base10() {
        assert_eq!(parse_int_or_zero("600000"), 600_000.0);
        assert_eq!(parse_int_or_zero("600k"), 600.0);
        assert_eq!(parse_int_or_zero("12.9"), 12.0);
        assert_eq!(parse_int_or_zero("garbage"), 0.0);
        assert_eq!(parse_int_or_zero(""), 0.0);
        assert_eq!(parse_int_or_zero("-50"), -50.0);
        assert_eq!(parse_int_or_zero("null"), 0.0);
        assert_eq!(parse_int_or_zero("true"), 0.0);
    }

    #[test]
    fn kdf_iteration_window_matches_js() {
        // `if (kdfIterations && (< 100000 || > 2000000))` → 400; NaN → 0 skips.
        let trips = |v: f64| v != 0.0 && !(100_000.0..=2_000_000.0).contains(&v);
        assert!(trips(600.0));
        assert!(trips(2_000_001.0));
        assert!(trips(-1.0));
        assert!(!trips(0.0));
        assert!(!trips(600_000.0));
        assert!(!trips(2_000_000.0));
    }

    #[test]
    fn register_key_regex_ladder() {
        let good_pub = format!("04{}", "a".repeat(128));
        assert!(regex_is_match(r"^04[0-9a-f]{128}$", &good_pub, true));
        assert!(!regex_is_match(
            r"^04[0-9a-f]{128}$",
            &format!("05{}", "a".repeat(128)),
            true
        ));
        // /i — uppercase hex digits accepted.
        assert!(regex_is_match(
            r"^04[0-9a-f]{128}$",
            &format!("04{}", "AB".repeat(64)),
            true
        ));
        // Lowercase-only regexes.
        assert!(regex_is_match(r"^[0-9a-f]{24}$", &"b".repeat(24), true));
        assert!(!regex_is_match(r"^[0-9a-f]{24}$", &"b".repeat(23), true));
        assert!(regex_is_match(r"^[0-9a-f]+$", &"c".repeat(100), true));
        assert!(!regex_is_match(r"^[0-9a-f]+$", "zz", true));
        assert!(!regex_is_match(r"^[0-9a-f]{64}$", &"g".repeat(64), false));
    }

    #[test]
    fn history_entry_coalesce_matches_js() {
        let h = json!({ "pubKeyHex": "04x", "ivHex": "iv", "kdfSaltHex": null });
        let e = history_entry(&h);
        // kdfSaltHex present-null → '' (falsy); kdfIterations absent → 0;
        // pubKeyHex passthrough; encryptedPrivateJwk missing → dropped.
        assert_eq!(
            e,
            json!({
                "pubKeyHex": "04x",
                "ivHex": "iv",
                "kdfSaltHex": "",
                "kdfIterations": 0,
            })
        );
    }

    #[test]
    fn nickname_filter_matches_js() {
        // `(body.nickname || '').replace(/[^a-zA-Z0-9_@._-]/g, '').slice(0, 50)`
        // — every disallowed char stripped (not a stop), then sliced.
        let filter = |s: &str| -> String {
            jsval::js_slice_utf16(
                &s.chars()
                    .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '@' | '.' | '-'))
                    .collect::<String>(),
                50,
            )
        };
        assert_eq!(filter("al!ce_x@y.z-9"), "alce_x@y.z-9");
        assert_eq!(filter(""), "");
        assert_eq!(filter(&"a".repeat(55)).len(), 50);
        assert_eq!(filter("ok nick"), "oknick");
    }

    #[test]
    fn utf16_len_matches_js_string_length() {
        // `String(msgData).length` — UTF-16 code units (a surrogate pair = 2).
        let len = |s: &str| -> usize { s.chars().map(|c| c.len_utf16()).sum() };
        assert_eq!(len(&"a".repeat(256_000)), 256_000);
        assert_eq!(len("𝕏"), 2, "astral char counts as 2 code units");
        assert_eq!(len(""), 0);
    }

    #[test]
    fn sweep_drops_only_stale() {
        let now = now_millis();
        let users: Vec<(String, i64)> = vec![
            ("a".into(), now),
            ("b".into(), now - 4 * 60 * 1000),
            ("c".into(), now - 6 * 60 * 1000),
        ];
        let cutoff = now - 5 * 60 * 1000;
        let kept: Vec<_> = users.into_iter().filter(|(_, t)| *t >= cutoff).collect();
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].0, "a");
        assert_eq!(kept[1].0, "b");
    }
}
