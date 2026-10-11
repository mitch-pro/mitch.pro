//! `/api/me/security-code`, `/api/me/2fa/*` (server.js:15949-16033) plus the
//! shared security-code / two-factor helpers (server.js:2460-2611).
//!
//! These endpoints skip `validId` and resolve the email from the raw sid
//! only (JS: `emailFromSid(cookies['studentId'] || cookies['id'] || '')`).

use super::{cookies_of, data_file, json_response, me_uid, parse_body_strict};
use crate::routes::admin::legacy::html_base_template;
use crate::routes::push::send_email_bg;
use crate::state::{AppState, PendingSecurityCode};
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::auth;
use mitch_lib::jsval;
use mitch_lib::totp;
use serde_json::{json, Value};
use std::sync::Arc;

pub(crate) fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    _body: &Value,
    body_bytes: &[u8],
) -> Option<Response> {
    if path == "/api/me/security-code" && method == Method::POST {
        return Some(security_code(state, headers, body_bytes));
    }
    if path == "/api/me/2fa/setup-totp" && method == Method::POST {
        return Some(setup_totp(state, headers));
    }
    if path == "/api/me/2fa/enable" && method == Method::POST {
        return enable_2fa(state, headers, body_bytes);
    }
    if path == "/api/me/2fa/disable" && method == Method::POST {
        return disable_2fa(state, headers, body_bytes);
    }
    None
}

fn security_code(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let cookies = cookies_of(state, headers);
    let uid = me_uid(&cookies);
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &uid) else {
        return json_response(401, json!({ "error": "not logged in" }));
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let action = jsval::string(&jsval::or(body.get("action"), json!("")));
    let action = action.trim().to_lowercase();
    match send_security_action_code(state, &auth::normalize_email(&email), &action) {
        Ok(()) => json_response(200, json!({ "ok": true })),
        Err(err) => json_response(400, json!({ "error": err })),
    }
}

fn setup_totp(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let cookies = cookies_of(state, headers);
    let uid = me_uid(&cookies);
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &uid) else {
        return json_response(401, json!({ "error": "not logged in" }));
    };
    let norm = auth::normalize_email(&email);
    let secret = totp::random_base32(32);
    save_two_factor_config(
        state,
        &norm,
        &json!({ "pendingTotpSecret": totp::seal_totp_secret(&secret, &state.id_secret) }),
    );
    let label = auth::encode_uri_component(&format!("mitch.pro:{norm}"));
    let issuer = auth::encode_uri_component("mitch.pro");
    let otpauth = format!(
        "otpauth://totp/{label}?secret={secret}&issuer={issuer}&algorithm=SHA1&digits=6&period=30"
    );
    let qr_url = format!(
        "https://api.qrserver.com/v1/create-qr-code/?size=220x220&data={}",
        auth::encode_uri_component(&otpauth)
    );
    json_response(
        200,
        json!({ "ok": true, "secret": secret, "otpauth": otpauth, "qrUrl": qr_url }),
    )
}

fn enable_2fa(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Option<Response> {
    let cookies = cookies_of(state, headers);
    let uid = me_uid(&cookies);
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &uid) else {
        return Some(json_response(401, json!({ "error": "not logged in" })));
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return Some(json_response(400, json!({ "error": "bad json" })));
    };
    let norm = auth::normalize_email(&email);
    let type_ = jsval::string(&jsval::or(body.get("type"), json!("")));
    let type_ = type_.trim().to_lowercase();

    if type_ == "email" {
        let verified =
            verify_security_action_code(state, &norm, "enable_email_2fa", body.get("emailCode"));
        if !verified.ok {
            return Some(json_response(
                verified.status,
                json!({ "error": verified.error }),
            ));
        }
        save_two_factor_config(
            state,
            &norm,
            &json!({
                "twofa_enabled": true,
                "twofa_type": "email",
                "twoFactorEnabled": true,
                "twofaEnabled": true,
            }),
        );
        return Some(json_response(200, json!({ "ok": true, "type": "email" })));
    }

    if type_ == "totp" {
        let profiles = state
            .store
            .read_document(&data_file(state, "profiles.json"), json!({}));
        let profile = profiles.get(&norm).cloned().unwrap_or(json!({}));
        // JS: profiles[norm]?.pendingTotpSecret || profiles[norm]?.totp_secret
        //     || profiles[norm]?.totpSecret — first truthy string wins.
        let stored_secret = jsval::string(&jsval::or(
            profile
                .get("pendingTotpSecret")
                .filter(|v| jsval::truthy(v)),
            jsval::or(
                profile.get("totp_secret").filter(|v| jsval::truthy(v)),
                jsval::or(profile.get("totpSecret"), json!("")),
            ),
        ));
        let secret = totp::open_totp_secret(&stored_secret, &state.id_secret);
        let code = jsval::string(&jsval::or(body.get("code"), json!("")));
        if secret.is_empty() || !totp::verify_totp(&secret, &code) {
            return Some(json_response(400, json!({ "error": "invalid totp code" })));
        }
        let verified =
            verify_security_action_code(state, &norm, "enable_totp_2fa", body.get("emailCode"));
        if !verified.ok {
            return Some(json_response(
                verified.status,
                json!({ "error": verified.error }),
            ));
        }
        let sealed = totp::seal_totp_secret(&secret, &state.id_secret);
        save_two_factor_config(
            state,
            &norm,
            &json!({
                "twofa_enabled": true,
                "twofa_type": "totp",
                "twoFactorEnabled": true,
                "twofaEnabled": true,
                "totp_secret": sealed,
                "totpSecret": sealed,
                "pendingTotpSecret": "",
            }),
        );
        return Some(json_response(200, json!({ "ok": true, "type": "totp" })));
    }

    Some(json_response(400, json!({ "error": "invalid 2fa type" })))
}

fn disable_2fa(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Option<Response> {
    let cookies = cookies_of(state, headers);
    let uid = me_uid(&cookies);
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &uid) else {
        return Some(json_response(401, json!({ "error": "not logged in" })));
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return Some(json_response(400, json!({ "error": "bad json" })));
    };
    let norm = auth::normalize_email(&email);
    let stored = mitch_lib::passwords::password_hash_get(&state.store, &norm).unwrap_or_default();
    let password = jsval::string(&jsval::or(body.get("password"), json!("")));
    // JS: try { ok = await Bun.password.verify(String(body.password || ''), stored); } catch {}
    // A missing/empty stored hash throws → ok stays false.
    let ok = !stored.is_empty() && mitch_lib::crypto::argon2_verify(&stored, &password);
    if !ok {
        return Some(json_response(401, json!({ "error": "password incorrect" })));
    }
    let verified = verify_security_action_code(state, &norm, "disable_2fa", body.get("emailCode"));
    if !verified.ok {
        return Some(json_response(
            verified.status,
            json!({ "error": verified.error }),
        ));
    }
    save_two_factor_config(
        state,
        &norm,
        &json!({
            "twofa_enabled": false,
            "twofa_type": "",
            "twoFactorEnabled": false,
            "twofaEnabled": false,
            "totp_secret": "",
            "totpSecret": "",
            "pendingTotpSecret": "",
        }),
    );
    Some(json_response(200, json!({ "ok": true })))
}

// ── helpers (server.js:2460-2611) ────────────────────────────────────────────

/// `securityActionKey(normEmail, action)` (server.js:2466-2468).
fn security_action_key(norm_email: &str, action: &str) -> String {
    format!(
        "{}:{}",
        auth::normalize_email(norm_email),
        action.trim().to_lowercase()
    )
}

/// `SECURITY_ACTION_LABELS` (server.js:2470-2475).
fn security_action_label(action: &str) -> Option<&'static str> {
    match action {
        "change_password" => Some("password change"),
        "enable_email_2fa" => Some("email 2FA setup"),
        "enable_totp_2fa" => Some("authenticator app 2FA setup"),
        "disable_2fa" => Some("2FA disable"),
        _ => None,
    }
}

/// `sendSecurityActionCode(normEmail, action)` (server.js:2477-2490).
pub(crate) fn send_security_action_code(
    state: &Arc<AppState>,
    norm_email: &str,
    action: &str,
) -> Result<(), &'static str> {
    let normalized_action = action.trim().to_lowercase();
    let Some(label) = security_action_label(&normalized_action) else {
        return Err("invalid security action");
    };
    let code = format!("{}", (100000.0 + js_rand() * 900000.0) as i64);
    let key = security_action_key(norm_email, &normalized_action);
    state
        .pending_security_codes
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            key,
            PendingSecurityCode {
                code: code.clone(),
                attempts: 0,
                expires: mitch_lib::school::now_millis() + 10 * 60 * 1000,
            },
        );
    let target_email = mitch_lib::profile::canonical_delivery_email(
        &state.store,
        state.data_dir(),
        &state.id_secret,
        norm_email,
    );
    let html = make_verification_code_html(state, label, &code, 10.0, &target_email);
    send_email_bg(
        state,
        &target_email,
        &format!("Confirm {label} - mitch.pro"),
        &html,
    );
    Ok(())
}

/// Result of `verifySecurityActionCode`.
pub(crate) struct VerifyResult {
    pub ok: bool,
    pub status: u16,
    pub error: &'static str,
}

impl VerifyResult {
    fn ok() -> Self {
        Self {
            ok: true,
            status: 200,
            error: "",
        }
    }
    fn fail(status: u16, error: &'static str) -> Self {
        Self {
            ok: false,
            status,
            error,
        }
    }
}

/// `verifySecurityActionCode(normEmail, action, code)` (server.js:2492-2507).
pub(crate) fn verify_security_action_code(
    state: &Arc<AppState>,
    norm_email: &str,
    action: &str,
    code: Option<&Value>,
) -> VerifyResult {
    let key = security_action_key(norm_email, action);
    let mut codes = state
        .pending_security_codes
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let expired = match codes.get(&key) {
        Some(rec) => mitch_lib::school::now_millis() > rec.expires,
        None => true,
    };
    if expired {
        codes.remove(&key);
        return VerifyResult::fail(401, "email verification code expired");
    }
    let rec = codes
        .get_mut(&key)
        .unwrap_or_else(|| unreachable!("entry checked above"));
    rec.attempts += 1;
    if rec.attempts > 5 {
        codes.remove(&key);
        return VerifyResult::fail(429, "too many email verification attempts");
    }
    let supplied = jsval::string(&jsval::or(code, json!("")));
    if supplied.trim() != rec.code {
        return VerifyResult::fail(400, "invalid email verification code");
    }
    codes.remove(&key);
    VerifyResult::ok()
}

/// `verifyPasswordChangeSecondFactor(normEmail, code)` (server.js:2509-2516).
pub(crate) fn verify_password_change_second_factor(
    state: &Arc<AppState>,
    norm_email: &str,
    code: Option<&Value>,
) -> VerifyResult {
    let twofa = two_factor_config(state, norm_email);
    if twofa.enabled && twofa.type_ == "totp" {
        let code_str = jsval::string(&jsval::or(code, json!("")));
        if !totp::verify_totp(&twofa.secret, &code_str) {
            return VerifyResult::fail(400, "invalid authenticator code");
        }
        return VerifyResult::ok();
    }
    verify_security_action_code(state, norm_email, "change_password", code)
}

/// `twoFactorConfig(normEmail)` (server.js:2547-2555).
pub(crate) struct TwoFactorConfig {
    pub enabled: bool,
    pub type_: String,
    pub secret: String,
}

pub(crate) fn two_factor_config(state: &Arc<AppState>, norm_email: &str) -> TwoFactorConfig {
    let profiles = state
        .store
        .read_document(&data_file(state, "profiles.json"), json!({}));
    let p = profiles.get(norm_email).cloned().unwrap_or(json!({}));
    TwoFactorConfig {
        enabled: jsval::truthy(&jsval::or(p.get("twofa_enabled"), json!(false)))
            || jsval::truthy(&jsval::or(p.get("twofaEnabled"), json!(false)))
            || jsval::truthy(&jsval::or(p.get("twoFactorEnabled"), json!(false))),
        type_: jsval::string(&jsval::or(
            p.get("twofa_type").filter(|v| jsval::truthy(v)),
            jsval::or(p.get("twofaType"), json!("email")),
        )),
        secret: totp::open_totp_secret(
            &jsval::string(&jsval::or(
                p.get("totp_secret").filter(|v| jsval::truthy(v)),
                jsval::or(p.get("totpSecret"), json!("")),
            )),
            &state.id_secret,
        ),
    }
}

/// `saveTwoFactorConfig(normEmail, patch)` (server.js:2600-2611).
pub(crate) fn save_two_factor_config(state: &Arc<AppState>, norm_email: &str, patch: &Value) {
    let file = data_file(state, "profiles.json");
    // JS loads the outer map FIRST, then ensureProfileDefaults does its own
    // load/normalize/save; the outer (pre-ensure) record still wins the
    // spread below. Reproduce that exactly.
    let outer = state.store.read_document(&file, json!({}));
    let p = mitch_lib::profile::ensure_profile_defaults(
        &state.store,
        state.data_dir(),
        &state.id_secret,
        norm_email,
        norm_email,
        &json!({}),
    );
    let stale = outer.get(norm_email).cloned().unwrap_or(json!({}));
    let mut merged = merge_objects(&merge_objects(&p, &stale), patch);
    if let Some(map) = merged.as_object_mut() {
        map.insert(
            "updatedAt".to_string(),
            json!(mitch_lib::school::now_millis()),
        );
    }
    let mut profiles = state.store.read_document(&file, json!({}));
    if let Some(map) = profiles.as_object_mut() {
        map.insert(norm_email.to_string(), merged);
    }
    let _ = state.store.write_document(&file, &profiles);
}

/// `makeVerificationCodeHtml(label, code, expiryMinutes[, email])`
/// (server.js:1764-1778), reusing the Step 8 dark shell.
pub(crate) fn make_verification_code_html(
    state: &Arc<AppState>,
    label: &str,
    code: &str,
    expiry_minutes: f64,
    email: &str,
) -> String {
    let safe_label = if label.is_empty() {
        "this action"
    } else {
        label
    };
    let safe_code = code.trim();
    let safe_mins = if expiry_minutes.is_finite() && expiry_minutes > 0.0 {
        expiry_minutes
    } else {
        10.0
    };
    let content = format!(
        concat!(
            "\n    <h2 style=\"margin: 0 0 16px; font-size: 20px; font-weight: 700; color: #f4f4f5;\">Verification Code</h2>\n",
            "    <p style=\"margin: 0 0 24px;\">Please use the following verification code to confirm <strong>{}</strong> on your account:</p>\n",
            "    <div style=\"background-color: rgba(168, 85, 247, 0.1); border: 1px solid rgba(168, 85, 247, 0.3); border-radius: 12px; padding: 20px; text-align: center; margin-bottom: 24px;\">\n",
            "      <span style=\"font-family: monospace; font-size: 36px; font-weight: 800; letter-spacing: 0.25em; color: #c084fc; padding-left: 0.25em;\">{}</span>\n",
            "    </div>\n",
            "    <p style=\"margin: 0; font-size: 13px; color: #f87171;\">⚠️ This verification code is active and valid for <strong>{} minutes</strong>. If you did not request this action, please secure your account.</p>\n",
            "  "
        ),
        safe_label,
        safe_code,
        safe_mins
    );
    html_base_template(
        state,
        email,
        &format!("Confirm {safe_label} - mitch.pro"),
        &content,
    )
}

/// `Math.random()` — f64 in [0, 1) like JS.
pub(crate) fn js_rand() -> f64 {
    use rand::Rng;
    rand::rng().random::<f64>()
}

/// Shallow JSON object merge (`{...a, ...b}`) — b wins; key order follows a,
/// then b's new keys (serde_json preserve_order matches JS spread).
fn merge_objects(a: &Value, b: &Value) -> Value {
    let mut out = match a {
        Value::Object(m) => m.clone(),
        _ => serde_json::Map::new(),
    };
    if let Some(bm) = b.as_object() {
        for (k, v) in bm {
            out.insert(k.clone(), v.clone());
        }
    }
    Value::Object(out)
}
