//! Authentication, registration, password reset, invite, newsletter, and SSO
//! route handlers (server.js:15030-15250, 15860-16635).

use std::path::PathBuf;
use std::sync::Arc;

use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::Response;
use mitch_lib::auth::{self, encode_uri_component};
use mitch_lib::coins;
use mitch_lib::crypto::{self, random_bytes_hex};
use mitch_lib::profile::ensure_profile_defaults;
use mitch_lib::totp;
use serde_json::{json, Value};

use crate::errors::json_resp;
use crate::handler::get_real_ip;
use crate::hosts::{
    is_mitch_sso_host, is_pickle_host, is_rjuhsd_host, request_host, sso_back_allowed,
};
use crate::routes::admin::legacy::{html_base_template, unsubscribe_url};
use crate::routes::me::account::{auth_success_response, is_secure_password};
use crate::routes::me::security::{save_two_factor_config, two_factor_config};
use crate::routes::push::{ntfy_notify, send_email_bg, verify_recaptcha};
use crate::state::{AppState, PendingTwoFactor, SsoBridgeToken};

fn is_invalidated(state: &AppState, id: &str) -> bool {
    if id.is_empty() {
        return false;
    }
    let doc = state
        .store
        .read_document(&data_file(state, "invalidated.json"), json!({}));
    doc.as_object().is_some_and(|m| m.contains_key(id))
}

fn is_revoked(state: &AppState, id: &str) -> bool {
    if id.is_empty() {
        return false;
    }
    let doc = state
        .store
        .read_document(&data_file(state, "revoked.json"), json!({}));
    doc.as_object().is_some_and(|m| m.contains_key(id))
}

/// Main route dispatch for auth, registration, reset, invite, newsletter, and SSO endpoints.
pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    search: &str,
    body_bytes: &[u8],
) -> Option<Response> {
    if path == "/api/signup" && *method == Method::POST {
        return Some(signup(state, headers, body_bytes).await);
    }
    if path == "/api/verify-signup" && *method == Method::POST {
        return Some(verify_signup(state, headers, body_bytes).await);
    }
    if path == "/api/login" && *method == Method::POST {
        return Some(login(state, headers, body_bytes).await);
    }
    if path == "/api/logout" && *method == Method::POST {
        return Some(logout(state, headers));
    }
    if path == "/api/verify-2fa" && *method == Method::POST {
        return Some(verify_2fa(state, headers, body_bytes));
    }
    if path == "/api/request-access" && *method == Method::POST {
        return Some(request_access(state, headers, body_bytes).await);
    }
    if path == "/api/claim-token" && *method == Method::POST {
        return Some(claim_token(state, headers, body_bytes).await);
    }
    if path == "/api/newsletter-signup" && *method == Method::POST {
        return Some(newsletter_signup(state, headers, body_bytes).await);
    }
    if path == "/api/newsletter/unsubscribe-direct" && *method == Method::POST {
        return Some(newsletter_unsubscribe_direct(state, headers, body_bytes));
    }
    if path == "/api/invite/set-code" && *method == Method::POST {
        return Some(invite_set_code(state, headers, body_bytes));
    }
    if path == "/api/invite/send" && *method == Method::POST {
        return Some(invite_send(state, headers, body_bytes));
    }
    if path == "/api/suggest" && *method == Method::POST {
        return Some(suggest(state, headers, body_bytes).await);
    }
    if path == "/api/sso/bridge" && *method == Method::GET {
        return Some(sso_bridge(state, headers, search));
    }
    if path == "/api/sso/bridge/handoff" && *method == Method::POST {
        return Some(sso_bridge_handoff(state, headers, body_bytes));
    }
    if path == "/api/sso/exchange" && (*method == Method::GET || *method == Method::POST) {
        return Some(sso_exchange(state, method, headers, search, body_bytes));
    }
    None
}

fn data_file(state: &AppState, name: &str) -> PathBuf {
    state.cfg.data_dir.join(name)
}

fn parse_body(body_bytes: &[u8]) -> Option<Value> {
    if body_bytes.is_empty() {
        return None;
    }
    serde_json::from_slice(body_bytes).ok()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn is_valid_username(name: &str) -> bool {
    if name.is_empty() || name.len() > 30 {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-')
}

pub(crate) fn resolve_login_identifier(state: &AppState, input: &str) -> Option<String> {
    let ident = input.trim().to_lowercase();
    if ident.is_empty() {
        return None;
    }
    let passwords = state
        .store
        .read_document(&data_file(state, "passwords.json"), json!({}));
    if ident.contains('@') {
        let norm = auth::normalize_email(&ident);
        if passwords.get(&norm).is_some() {
            return Some(norm);
        }
        return None;
    }
    let profiles = state
        .store
        .read_document(&data_file(state, "profiles.json"), json!({}));
    if let Some(obj) = profiles.as_object() {
        for (norm_email, profile) in obj {
            let u = profile
                .get("username")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_lowercase();
            if u == ident && passwords.get(norm_email).is_some() {
                return Some(norm_email.clone());
            }
        }
    }
    None
}

fn is_premium_email(state: &AppState, email: &str) -> bool {
    auth::is_premium_email(&state.store, email)
}

// ── HTML builders ──────────────────────────────────────────────────────────

pub(crate) fn make_verification_code_html(
    state: &AppState,
    action_label: &str,
    code: &str,
    minutes: i64,
    target_email: &str,
) -> String {
    let content = format!(
        r#"
    <h2 style="margin: 0 0 16px; font-size: 20px; font-weight: 700; color: #f4f4f5; text-align: center;">{action_label}</h2>
    <p style="margin: 0 0 20px; text-align: center; color: #cbd5e1;">Your verification code is below. It will expire in {minutes} minutes.</p>
    <div style="background: rgba(168, 85, 247, 0.1); border: 2px dashed #a855f7; border-radius: 12px; padding: 20px; text-align: center; margin-bottom: 24px;">
      <span style="font-family: monospace; font-size: 32px; font-weight: 700; letter-spacing: 6px; color: #c084fc;">{code}</span>
    </div>
    <p style="margin: 0; font-size: 13px; color: #94a3b8; text-align: center;">If you didn't request this code, you can safely ignore this email.</p>
    "#
    );
    html_base_template(state, target_email, action_label, &content)
}

fn make_invite_award_html(state: &AppState, email: &str) -> String {
    let content = r#"
    <h2 style="margin: 0 0 16px; font-size: 20px; font-weight: 700; color: #10b981; text-align: center;">🎉 Referral Bonus Claimed!</h2>
    <div style="background-color: rgba(16, 185, 129, 0.08); border: 1px solid rgba(16, 185, 129, 0.25); border-radius: 12px; padding: 20px; text-align: center; margin-bottom: 24px;">
      <p style="margin: 0 0 8px; font-size: 16px; font-weight: 700; color: #f4f4f5;">You earned 2,000 MitchCoins!</p>
      <p style="margin: 0; color: #cbd5e1;">Someone you invited just completed their sign-up on mitch.pro. Keep sharing your invite link to earn more referral bonuses!</p>
    </div>
    <div style="text-align: center; margin-bottom: 8px;">
      <a href="https://mitchdog.com" style="display: inline-block; background-color: #10b981; color: #ffffff; text-decoration: none; padding: 12px 24px; border-radius: 10px; font-weight: 700;">Claim Bonus</a>
    </div>
    "#;
    html_base_template(state, email, "mitch.pro - Referral Bonus Claimed!", content)
}

fn make_newsletter_welcome_html(state: &AppState, email: &str, unsub_url: &str) -> String {
    let content = format!(
        r#"
    <h2 style="margin: 0 0 16px; font-size: 20px; font-weight: 700; color: #38bdf8; text-align: center;">Welcome to the Newsletter!</h2>
    <p style="margin: 0 0 24px; text-align: center; color: #cbd5e1;">You are now subscribed to the mitch.pro newsletter. You'll receive updates when new games, features, or developer updates are posted.</p>
    <div style="text-align: center; margin-bottom: 8px;">
      <a href="https://mitchdog.com" style="display: inline-block; background: linear-gradient(135deg, #a855f7, #6366f1); color: #ffffff; text-decoration: none; padding: 12px 24px; border-radius: 10px; font-weight: 700;">Explore mitch.pro</a>
    </div>
    <p style="margin: 32px 0 0; font-size: 11px; color: #64748b; text-align: center;">
      If you wish to opt-out, you can <a href="{unsub_url}" style="color: #64748b; text-decoration: underline;">unsubscribe here</a> at any time.
    </p>
    "#
    );
    html_base_template(state, email, "mitch.pro Newsletter Subscription", &content)
}

fn make_invite_friend_html(
    state: &AppState,
    to_email: &str,
    sender_display: &str,
    invite_link: &str,
) -> String {
    let content = format!(
        r#"
    <h2 style="margin: 0 0 16px; font-size: 22px; font-weight: 800; color: #f4f4f5; text-align: center;">🎮 Join mitch.pro</h2>
    <div style="background-color: rgba(255, 255, 255, 0.03); border: 1px solid rgba(255,255,255,0.08); border-radius: 12px; padding: 20px; margin-bottom: 24px; text-align: center;">
      <p style="margin: 0 0 12px; font-size: 16px; font-weight: 700; color: #f4f4f5;">You've been invited by {sender_display}!</p>
      <p style="margin: 0 0 16px; color: #94a3b8; line-height: 1.6;">mitch.pro is a student-only platform featuring custom games, tools, secure messaging, and MitchCoins.</p>
      <p style="margin: 0; color: #fbbf24; font-weight: 700;">🎁 Sign up today and you'll both earn 2,000 MitchCoins!</p>
    </div>
    <div style="text-align: center; margin-bottom: 8px;">
      <a href="{invite_link}" style="display: inline-block; background: linear-gradient(135deg, #a855f7, #6366f1); color: #ffffff; text-decoration: none; padding: 14px 28px; border-radius: 10px; font-weight: 700; box-shadow: 0 10px 20px rgba(168, 85, 247, 0.3);">Accept Invite</a>
    </div>
    "#
    );
    html_base_template(state, to_email, "You're invited to mitch.pro!", &content)
}

// ── Handlers ───────────────────────────────────────────────────────────────

/// `POST /api/signup` (server.js:16180-16251).
async fn signup(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(body) = parse_body(body_bytes) else {
        return json_resp(400, json!({ "success": false, "message": "bad json" }));
    };
    let email = body
        .get("email")
        .or_else(|| body.get("username"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let password = body
        .get("password")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if email.is_empty() || password.is_empty() {
        return json_resp(
            400,
            json!({ "success": false, "message": "Email/username and password required." }),
        );
    }
    if !email.contains('@') || email.len() > 254 {
        return json_resp(
            400,
            json!({ "success": false, "message": "Valid email address required." }),
        );
    }
    let norm_email = auth::normalize_email(&email);

    let (pwd_valid, pwd_err) = is_secure_password(state, &password);
    if !pwd_valid {
        return json_resp(400, json!({ "success": false, "message": pwd_err }));
    }

    let ip = get_real_ip(headers, None);
    let recaptcha_token = body
        .get("recaptcha_token")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !verify_recaptcha(state, recaptcha_token, &ip, "").await {
        return json_resp(
            400,
            json!({ "success": false, "message": "reCAPTCHA failed. Please try again." }),
        );
    }

    // Blacklist check
    if mitch_lib::bans::blacklist_get(&state.store, &norm_email, &norm_email).is_some() {
        return json_resp(
            400,
            json!({ "success": false, "message": "Access denied." }),
        );
    }

    // Existing account check
    let passwords = state
        .store
        .read_document(&data_file(state, "passwords.json"), json!({}));
    if passwords.get(&norm_email).is_some() {
        return json_resp(
            400,
            json!({ "success": false, "message": "Account already exists. Please log in." }),
        );
    }

    // Username validation if specified
    let requested_username = body
        .get("username")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if !requested_username.is_empty() {
        if !is_valid_username(&requested_username) {
            return json_resp(
                400,
                json!({ "success": false, "message": "Username must use lowercase letters, numbers, \".\", \"_\", or \"-\"." }),
            );
        }
        let profiles = state
            .store
            .read_document(&data_file(state, "profiles.json"), json!({}));
        if let Some(obj) = profiles.as_object() {
            if obj.values().any(|p| {
                p.get("username")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_lowercase()
                    == requested_username
            }) {
                return json_resp(
                    400,
                    json!({ "success": false, "message": "Username is already taken." }),
                );
            }
        }
    }

    let code = format!("{:06}", (rand::random::<u32>() % 900_000) + 100_000);
    let hash = crypto::argon2_hash(&password);
    let ref_code = body
        .get("refCode")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_uppercase();
    let ref_code = ref_code.chars().take(32).collect::<String>();

    let mut codes = state
        .store
        .read_document(&data_file(state, "signup_codes.json"), json!({}));
    if let Some(map) = codes.as_object_mut() {
        map.insert(
            norm_email.clone(),
            json!({
                "code": code,
                "hash": hash,
                "email": email,
                "refCode": ref_code,
                "username": requested_username,
                "nickname": body.get("nickname").and_then(|v| v.as_str()).unwrap_or("").trim().chars().take(40).collect::<String>(),
                "gradYear": body.get("gradYear").and_then(|v| v.as_str()).unwrap_or("").trim().chars().take(20).collect::<String>(),
                "gender": body.get("gender").and_then(|v| v.as_str()).unwrap_or("").trim().chars().take(40).collect::<String>(),
                "referralSource": body.get("referralSource").and_then(|v| v.as_str()).unwrap_or("").trim().chars().take(80).collect::<String>(),
                "referralDetails": body.get("referralDetails").and_then(|v| v.as_str()).unwrap_or("").trim().chars().take(200).collect::<String>(),
                "expires": now_ms() + (30 * 60 * 1000)
            }),
        );
        let _ = state
            .store
            .write_document(&data_file(state, "signup_codes.json"), &codes);
    }

    let html = make_verification_code_html(state, "Account Signup", &code, 30, &email);
    send_email_bg(state, &email, "Your mitch.pro Verification Code", &html);

    json_resp(200, json!({ "success": true }))
}

/// `POST /api/verify-signup` (server.js:16254-16343).
async fn verify_signup(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(body) = parse_body(body_bytes) else {
        return json_resp(400, json!({ "success": false, "message": "bad json" }));
    };
    let ip = get_real_ip(headers, None);
    let recaptcha_token = body
        .get("recaptcha_token")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !verify_recaptcha(state, recaptcha_token, &ip, "").await {
        return json_resp(
            400,
            json!({ "success": false, "message": "reCAPTCHA failed. Please try again." }),
        );
    }
    let email = body
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let code = body
        .get("code")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let norm_email = auth::normalize_email(&email);

    let mut codes = state
        .store
        .read_document(&data_file(state, "signup_codes.json"), json!({}));
    let entry = codes.get(&norm_email).cloned();
    let Some(entry_val) = entry else {
        return json_resp(
            400,
            json!({ "success": false, "message": "Invalid verification code." }),
        );
    };

    let entry_code = entry_val.get("code").and_then(|v| v.as_str()).unwrap_or("");
    if entry_code != code {
        return json_resp(
            400,
            json!({ "success": false, "message": "Invalid verification code." }),
        );
    }

    let expires = entry_val
        .get("expires")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if now_ms() > expires {
        if let Some(map) = codes.as_object_mut() {
            map.remove(&norm_email);
            let _ = state
                .store
                .write_document(&data_file(state, "signup_codes.json"), &codes);
        }
        return json_resp(
            400,
            json!({ "success": false, "message": "Verification code expired. Please sign up again." }),
        );
    }

    let hash = entry_val.get("hash").and_then(|v| v.as_str()).unwrap_or("");
    let mut passwords = state
        .store
        .read_document(&data_file(state, "passwords.json"), json!({}));
    if let Some(map) = passwords.as_object_mut() {
        map.insert(norm_email.clone(), json!(hash));
        let _ = state
            .store
            .write_document(&data_file(state, "passwords.json"), &passwords);
    }

    let raw_email = entry_val
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or(&email)
        .to_string();
    let tok = random_bytes_hex(24);
    let mut tokens = state
        .store
        .read_document(&data_file(state, "tokens.json"), json!({}));
    if let Some(map) = tokens.as_object_mut() {
        let host = request_host(headers).to_string();
        map.insert(
            tok.clone(),
            json!({
                "email": raw_email,
                "norm_email": norm_email,
                "created_at": (now_ms() as f64) / 1000.0,
                "used": true,
                "claimed_domains": [host]
            }),
        );
        let _ = state
            .store
            .write_document(&data_file(state, "tokens.json"), &tokens);
    }

    let _sid = auth::issue_login_session(&state.store, &state.id_secret, &norm_email, &raw_email);

    let profile_patch = json!({
        "username": entry_val.get("username").and_then(|v| v.as_str()).unwrap_or(""),
        "nickname": entry_val.get("nickname").and_then(|v| v.as_str()).unwrap_or(""),
        "displayName": entry_val.get("nickname").and_then(|v| v.as_str()).unwrap_or(""),
        "gradYear": entry_val.get("gradYear").and_then(|v| v.as_str()).unwrap_or(""),
        "gender": entry_val.get("gender").and_then(|v| v.as_str()).unwrap_or(""),
        "referralSource": entry_val.get("referralSource").and_then(|v| v.as_str()).unwrap_or(""),
        "hasCompletedTutorial": false
    });
    ensure_profile_defaults(
        &state.store,
        &state.cfg.data_dir,
        &state.id_secret,
        &norm_email,
        &raw_email,
        &profile_patch,
    );

    let ref_source = entry_val
        .get("referralSource")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !ref_source.is_empty() {
        let mut refs = state
            .store
            .read_document(&data_file(state, "referral_sources.json"), json!({}));
        if let Some(map) = refs.as_object_mut() {
            map.insert(
                norm_email.clone(),
                json!({
                    "source": ref_source,
                    "details": entry_val.get("referralDetails").and_then(|v| v.as_str()).unwrap_or(""),
                    "timestamp": now_ms()
                }),
            );
            let _ = state
                .store
                .write_document(&data_file(state, "referral_sources.json"), &refs);
        }
    }

    if let Some(map) = codes.as_object_mut() {
        map.remove(&norm_email);
        let _ = state
            .store
            .write_document(&data_file(state, "signup_codes.json"), &codes);
    }

    // Referral reward
    let ref_code = entry_val
        .get("refCode")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_uppercase();
    if !ref_code.is_empty() {
        let inv_codes = state
            .store
            .read_document(&data_file(state, "invite_codes.json"), json!({}));
        let mut found_ref_norm: Option<String> = None;
        if let Some(map) = inv_codes.as_object() {
            for (k, v) in map {
                let code_str = v.as_str().unwrap_or("").trim().to_uppercase();
                if code_str == ref_code {
                    found_ref_norm = Some(k.clone());
                    break;
                }
            }
        }
        if let Some(ref_norm) = found_ref_norm {
            if ref_norm != norm_email {
                let mut inv_claims = state
                    .store
                    .read_document(&data_file(state, "invite_claims.json"), json!({}));
                let already_claimed = inv_claims.get(&norm_email).is_some();
                if !already_claimed {
                    if let Some(map) = inv_claims.as_object_mut() {
                        map.insert(
                            norm_email.clone(),
                            json!({
                                "refNorm": ref_norm,
                                "ts": now_ms(),
                                "paid": true
                            }),
                        );
                        let _ = state
                            .store
                            .write_document(&data_file(state, "invite_claims.json"), &inv_claims);
                    }
                    coins::add_coins(
                        &state.store,
                        &state.cfg.data_dir,
                        &ref_norm,
                        2000.0,
                        1.0,
                        "invite",
                    );
                    coins::add_coins(
                        &state.store,
                        &state.cfg.data_dir,
                        &norm_email,
                        2000.0,
                        1.0,
                        "invite",
                    );
                    let email_html = make_invite_award_html(state, &ref_norm);
                    send_email_bg(
                        state,
                        &ref_norm,
                        "mitch.pro - Referral Bonus Claimed!",
                        &email_html,
                    );
                    let msg = format!(
                        "Referral paid: {} and {} each earned 2000 coins for invite",
                        ref_norm, norm_email
                    );
                    ntfy_notify(&msg, "Invite Reward", "default");
                }
            }
        }
    }

    auth_success_response(
        state,
        headers,
        json!({ "success": true }),
        &norm_email,
        &raw_email,
    )
}

/// `POST /api/login` (server.js:15860-15917).
async fn login(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(body) = parse_body(body_bytes) else {
        return json_resp(400, json!({ "success": false, "message": "bad json" }));
    };
    let email = body
        .get("email")
        .or_else(|| body.get("username"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let password = body
        .get("password")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if email.is_empty() || password.is_empty() {
        return json_resp(
            400,
            json!({ "success": false, "message": "Email/username and password required." }),
        );
    }

    let ip = get_real_ip(headers, None);
    let recaptcha_token = body
        .get("recaptcha_token")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !verify_recaptcha(state, recaptcha_token, &ip, "").await {
        return json_resp(
            400,
            json!({ "success": false, "message": "reCAPTCHA failed. Please try again." }),
        );
    }

    let Some(norm_email) = resolve_login_identifier(state, &email) else {
        return json_resp(
            401,
            json!({ "success": false, "message": "Invalid email/username or password." }),
        );
    };

    let passwords = state
        .store
        .read_document(&data_file(state, "passwords.json"), json!({}));
    let Some(stored_val) = passwords.get(&norm_email) else {
        return json_resp(
            401,
            json!({ "success": false, "message": "Invalid email/username or password." }),
        );
    };
    let stored_hash = stored_val.as_str().unwrap_or("");

    if !crypto::argon2_verify(stored_hash, &password) {
        return json_resp(
            401,
            json!({ "success": false, "message": "Invalid email/username or password." }),
        );
    }

    ensure_profile_defaults(
        &state.store,
        &state.cfg.data_dir,
        &state.id_secret,
        &norm_email,
        &norm_email,
        &json!({}),
    );
    let twofa = two_factor_config(state, &norm_email);
    if twofa.enabled {
        let temp_token = random_bytes_hex(24);
        let mut rec = PendingTwoFactor {
            norm_email: norm_email.clone(),
            twofa_type: twofa.type_.clone(),
            code: None,
            attempts: 0,
            expires: now_ms() + 5 * 60 * 1000,
        };
        if twofa.type_ == "email" {
            let code = format!("{:06}", (rand::random::<u32>() % 900_000) + 100_000);
            rec.code = Some(code.clone());
            let html = make_verification_code_html(
                state,
                "Login Two-Factor Authentication",
                &code,
                5,
                &norm_email,
            );
            send_email_bg(state, &norm_email, "Your mitch.pro login code", &html);
        }
        if let Ok(mut map) = state.pending_two_factor.lock() {
            map.insert(temp_token.clone(), rec);
        }
        return json_resp(
            200,
            json!({
                "success": false,
                "twofa_required": true,
                "twofa_type": twofa.type_,
                "temp_token": temp_token
            }),
        );
    }

    auth_success_response(
        state,
        headers,
        json!({ "success": true }),
        &norm_email,
        &norm_email,
    )
}

/// `POST /api/logout` (server.js:15919-15941).
fn logout(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let cookies = auth::get_cookies_from_header_value(
        headers
            .get(axum::http::header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or(""),
        &state.store,
        &state.id_secret,
        false,
    );
    let token = cookies.get("mitch_session").unwrap_or("");
    if !token.is_empty() {
        let mut sessions = state
            .store
            .read_document(&data_file(state, "auth_sessions.json"), json!({}));
        let key = auth::hash_session_token(token);
        if let Some(map) = sessions.as_object_mut() {
            if map.remove(&key).is_some() {
                let _ = state
                    .store
                    .write_document(&data_file(state, "auth_sessions.json"), &sessions);
            }
        }
    }

    let secure_flag = std::env::var("SESSION_COOKIE_SECURE").unwrap_or_default();
    let node_env_prod = std::env::var("NODE_ENV").unwrap_or_default() == "production";

    let mut resp = json_resp(200, json!({ "success": true }));
    let append = |resp: &mut Response, val: String| {
        if let Ok(v) = HeaderValue::from_str(&val) {
            resp.headers_mut().append(axum::http::header::SET_COOKIE, v);
        }
    };
    append(
        &mut resp,
        auth::clear_cookie_header("mitch_session", &secure_flag, node_env_prod, true),
    );
    append(
        &mut resp,
        auth::clear_cookie_header("studentId", &secure_flag, node_env_prod, false),
    );
    append(
        &mut resp,
        auth::clear_cookie_header("id", &secure_flag, node_env_prod, false),
    );
    append(
        &mut resp,
        auth::clear_cookie_header("password", &secure_flag, node_env_prod, false),
    );
    append(
        &mut resp,
        auth::clear_cookie_header("adminId", &secure_flag, node_env_prod, false),
    );
    resp
}

/// `POST /api/verify-2fa` (server.js:15943-15966).
fn verify_2fa(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(body) = parse_body(body_bytes) else {
        return json_resp(400, json!({ "success": false, "message": "bad json" }));
    };
    let temp_token = body
        .get("temp_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let code = body
        .get("code")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    let mut entry_opt: Option<PendingTwoFactor> = None;
    if let Ok(mut map) = state.pending_two_factor.lock() {
        if let Some(entry) = map.get_mut(&temp_token) {
            if now_ms() > entry.expires {
                map.remove(&temp_token);
            } else {
                entry.attempts += 1;
                if entry.attempts > 5 {
                    map.remove(&temp_token);
                    return json_resp(
                        429,
                        json!({ "success": false, "message": "Too many verification attempts." }),
                    );
                }
                entry_opt = Some(entry.clone());
            }
        }
    }

    let Some(entry) = entry_opt else {
        return json_resp(
            401,
            json!({ "success": false, "message": "Verification expired. Please log in again." }),
        );
    };

    let cfg = two_factor_config(state, &entry.norm_email);
    let ok = if entry.twofa_type == "totp" {
        totp::verify_totp(&cfg.secret, &code)
    } else {
        entry.code.as_deref() == Some(&code)
    };

    if !ok {
        return json_resp(
            401,
            json!({ "success": false, "message": "Invalid verification code." }),
        );
    }

    if let Ok(mut map) = state.pending_two_factor.lock() {
        map.remove(&temp_token);
    }

    auth_success_response(
        state,
        headers,
        json!({ "success": true }),
        &entry.norm_email,
        &entry.norm_email,
    )
}

/// `POST /api/request-access` (server.js:16346-16379).
async fn request_access(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(body) = parse_body(body_bytes) else {
        return json_resp(400, json!({ "success": false, "message": "bad json" }));
    };
    let email = body
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let ip = get_real_ip(headers, None);
    let recaptcha_token = body
        .get("recaptcha_token")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !verify_recaptcha(state, recaptcha_token, &ip, "").await {
        return json_resp(
            400,
            json!({ "success": false, "message": "reCAPTCHA failed. Please try again." }),
        );
    }

    let norm_email = auth::normalize_email(&email);
    let passwords = state
        .store
        .read_document(&data_file(state, "passwords.json"), json!({}));
    if passwords.get(&norm_email).is_none() {
        return json_resp(
            400,
            json!({ "success": false, "message": "Account not found." }),
        );
    }

    let otp = format!("{:06}", (rand::random::<u32>() % 900_000) + 100_000);
    let token = random_bytes_hex(24);
    let mut tokens = state
        .store
        .read_document(&data_file(state, "tokens.json"), json!({}));
    if let Some(map) = tokens.as_object_mut() {
        map.insert(
            token.clone(),
            json!({
                "email": email,
                "norm_email": norm_email,
                "otp": otp,
                "created_at": (now_ms() as f64) / 1000.0,
                "expires": now_ms() + (30 * 60 * 1000),
                "used": false,
                "type": "reset"
            }),
        );
        let _ = state
            .store
            .write_document(&data_file(state, "tokens.json"), &tokens);
    }

    let html = make_verification_code_html(state, "Password Reset", &otp, 30, &email);
    send_email_bg(state, &email, "Your mitch.pro Reset Code", &html);

    json_resp(200, json!({ "success": true }))
}

/// `POST /api/claim-token` (server.js:16558-16635).
async fn claim_token(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(body) = parse_body(body_bytes) else {
        return json_resp(400, json!({ "success": false, "message": "bad json" }));
    };
    let ip = get_real_ip(headers, None);
    let recaptcha_token = body
        .get("recaptcha_token")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !verify_recaptcha(state, recaptcha_token, &ip, "").await {
        return json_resp(
            400,
            json!({ "success": false, "message": "reCAPTCHA failed. Please try again." }),
        );
    }

    let raw_domain = body
        .get("domain")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let domain = if !raw_domain.is_empty() {
        raw_domain.split(':').next().unwrap_or("").to_string()
    } else {
        request_host(headers)
            .split(':')
            .next()
            .unwrap_or("unknown")
            .to_string()
    };

    let token = body
        .get("token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let mut tokens = state
        .store
        .read_document(&data_file(state, "tokens.json"), json!({}));

    let mut token_key = token.clone();
    let mut entry = tokens.get(&token).cloned();

    // OTP fallback for 6-digit codes
    if entry.is_none() && token.len() == 6 && token.chars().all(|c| c.is_ascii_digit()) {
        if let Some(obj) = tokens.as_object() {
            for (k, t) in obj {
                let t_type = t.get("type").and_then(|v| v.as_str()).unwrap_or("");
                let t_otp = t.get("otp").and_then(|v| v.as_str()).unwrap_or("");
                let t_used = t.get("used").and_then(|v| v.as_bool()).unwrap_or(false);
                let t_exp = t.get("expires").and_then(|v| v.as_i64()).unwrap_or(0);
                if t_type == "reset"
                    && t_otp == token
                    && !t_used
                    && (t_exp == 0 || now_ms() < t_exp)
                {
                    token_key = k.clone();
                    entry = Some(t.clone());
                    break;
                }
            }
        }
    }

    let Some(mut entry_val) = entry else {
        return json_resp(400, json!({ "success": false, "message": "Invalid token" }));
    };

    let infinite = entry_val
        .get("infinite")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let email = entry_val
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let norm_email = entry_val
        .get("norm_email")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| auth::normalize_email(&email));
    let entry_type = entry_val
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if entry_type == "reset" {
        let new_password = body
            .get("password")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if new_password.is_empty() {
            return json_resp(
                400,
                json!({ "success": false, "message": "Password is required." }),
            );
        }
        let (pwd_valid, pwd_err) = is_secure_password(state, &new_password);
        if !pwd_valid {
            return json_resp(400, json!({ "success": false, "message": pwd_err }));
        }
        let hash = crypto::argon2_hash(&new_password);
        let mut passwords = state
            .store
            .read_document(&data_file(state, "passwords.json"), json!({}));
        if let Some(map) = passwords.as_object_mut() {
            map.insert(norm_email.clone(), json!(hash));
            let _ = state
                .store
                .write_document(&data_file(state, "passwords.json"), &passwords);
        }
        auth::rotate_session_generation(&state.store, &norm_email);
        if two_factor_config(state, &norm_email).type_ == "totp" {
            save_two_factor_config(
                state,
                &norm_email,
                &json!({
                    "twofa_enabled": false,
                    "twofa_type": "",
                    "twoFactorEnabled": false,
                    "twofaEnabled": false,
                    "totp_secret": "",
                    "totpSecret": "",
                    "pendingTotpSecret": ""
                }),
            );
        }
        if let Some(obj) = entry_val.as_object_mut() {
            obj.insert("used".to_string(), json!(true));
            obj.insert("used_at".to_string(), json!((now_ms() as f64) / 1000.0));
        }
    } else if !infinite {
        let used = entry_val
            .get("used")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let claimed_domains = entry_val.get("claimed_domains");
        if used && claimed_domains.is_none() {
            return json_resp(
                400,
                json!({ "success": false, "message": "Token already used" }),
            );
        }
        let created_at = entry_val
            .get("created_at")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        if (now_ms() as f64) / 1000.0 - created_at > 1_209_600.0 {
            return json_resp(
                400,
                json!({ "success": false, "message": "Token expired (14d)" }),
            );
        }
        let mut cd_map = match claimed_domains {
            Some(Value::Object(m)) => m.clone(),
            _ => serde_json::Map::new(),
        };
        if cd_map.contains_key(&domain) {
            return json_resp(
                400,
                json!({ "success": false, "message": "Already claimed on this domain" }),
            );
        }
        cd_map.insert(domain, json!((now_ms() as f64) / 1000.0));
        if let Some(obj) = entry_val.as_object_mut() {
            obj.insert("claimed_domains".to_string(), Value::Object(cd_map));
        }
    } else {
        let claim_count = entry_val
            .get("claim_count")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        if let Some(obj) = entry_val.as_object_mut() {
            obj.insert("claim_count".to_string(), json!(claim_count + 1));
        }
    }

    if let Some(map) = tokens.as_object_mut() {
        map.insert(token_key, entry_val);
        let _ = state
            .store
            .write_document(&data_file(state, "tokens.json"), &tokens);
    }

    let _sid = auth::issue_login_session(&state.store, &state.id_secret, &norm_email, &email);

    auth_success_response(
        state,
        headers,
        json!({ "success": true }),
        &norm_email,
        &email,
    )
}

/// `POST /api/newsletter-signup` (server.js:16381-16407).
async fn newsletter_signup(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Response {
    let cookies = auth::get_cookies_from_header_value(
        headers
            .get(axum::http::header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or(""),
        &state.store,
        &state.id_secret,
        false,
    );
    let student_id = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("")
        .trim();
    if !auth::valid_id(student_id, &state.id_secret)
        || !state.check_password_cookie(headers, None)
        || is_invalidated(state, student_id)
        || is_revoked(state, student_id)
    {
        return json_resp(
            403,
            json!({ "success": false, "message": "Not signed in." }),
        );
    }

    let Some(body) = parse_body(body_bytes) else {
        return json_resp(400, json!({ "success": false, "message": "bad json" }));
    };
    let ip = get_real_ip(headers, None);
    let recaptcha_token = body
        .get("recaptcha_token")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !verify_recaptcha(state, recaptcha_token, &ip, "").await {
        return json_resp(
            400,
            json!({ "success": false, "message": "reCAPTCHA failed. Please try again." }),
        );
    }

    let email = body
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if !email.ends_with("@student.rjuhsd.us") || email.len() > 254 {
        return json_resp(
            400,
            json!({ "success": false, "message": "Invalid email ending." }),
        );
    }

    let extra_file = data_file(state, "newsletter_extra.json");
    let mut extra: Vec<String> = state
        .store
        .read_document(&extra_file, json!([]))
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let norm = auth::normalize_email(&email);
    let exists = extra.iter().any(|e| auth::normalize_email(e) == norm);
    if exists {
        return json_resp(200, json!({ "success": true, "already": true }));
    }

    extra.push(email.clone());
    extra.sort();
    extra.dedup();
    let _ = state.store.write_document(&extra_file, &json!(extra));

    let unsub = unsubscribe_url(state, &email);
    let welcome_html = make_newsletter_welcome_html(state, &email, &unsub);
    send_email_bg(
        state,
        &email,
        "You're Signed Up for the mitch.pro Newsletter",
        &welcome_html,
    );
    ntfy_notify(&email, "Newsletter Signup", "default");

    json_resp(200, json!({ "success": true }))
}

/// `POST /api/newsletter/unsubscribe-direct` (server.js:16515-16555).
fn newsletter_unsubscribe_direct(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Response {
    let Some(body) = parse_body(body_bytes) else {
        return json_resp(400, json!({ "success": false, "message": "bad json" }));
    };

    let email_val = body.get("email").and_then(|v| v.as_str()).map(|s| s.trim());
    let token_val = body.get("token").and_then(|v| v.as_str()).map(|s| s.trim());

    let norm_email = if email_val.is_some() || token_val.is_some() {
        let Some(email) = email_val.filter(|s| !s.is_empty()) else {
            return json_resp(
                400,
                json!({ "success": false, "message": "Email required." }),
            );
        };
        let Some(token) = token_val.filter(|s| !s.is_empty()) else {
            return json_resp(
                400,
                json!({ "success": false, "message": "Unsubscribe token required." }),
            );
        };
        let norm = auth::normalize_email(email);
        let tokens = state
            .store
            .read_document(&data_file(state, "unsubscribe_tokens.json"), json!({}));
        let expected = tokens.get(&norm).and_then(|v| v.as_str()).unwrap_or("");
        if expected.is_empty() || expected != token {
            return json_resp(
                403,
                json!({ "success": false, "message": "Invalid or expired unsubscribe token." }),
            );
        }
        norm
    } else {
        if !state.check_password_cookie(headers, None) {
            return json_resp(
                401,
                json!({ "success": false, "message": "Log in, or use the unsubscribe link from your email." }),
            );
        }
        let cookies = auth::get_cookies_from_header_value(
            headers
                .get(axum::http::header::COOKIE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or(""),
            &state.store,
            &state.id_secret,
            false,
        );
        let sid = cookies
            .get("studentId")
            .filter(|s| !s.is_empty())
            .or_else(|| cookies.get("id"))
            .unwrap_or("");
        let Some(acct_email) = auth::email_from_sid(&state.store, &state.id_secret, sid) else {
            return json_resp(
                400,
                json!({ "success": false, "message": "Could not detect your account email." }),
            );
        };
        auth::normalize_email(&acct_email)
    };

    let unsub_file = data_file(state, "newsletter_unsub.json");
    let mut unsub: Vec<String> = state
        .store
        .read_document(&unsub_file, json!([]))
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    if !unsub.contains(&norm_email) {
        unsub.push(norm_email.clone());
        unsub.sort();
        unsub.dedup();
        let _ = state.store.write_document(&unsub_file, &json!(unsub));
        let _ = std::fs::write(
            &unsub_file,
            mitch_lib::data::js_stringify_pretty(&json!(unsub)),
        );
    }

    json_resp(200, json!({ "success": true, "email": norm_email }))
}

/// `POST /api/invite/set-code` (server.js:16412-16436).
fn invite_set_code(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let cookies = auth::get_cookies_from_header_value(
        headers
            .get(axum::http::header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or(""),
        &state.store,
        &state.id_secret,
        false,
    );
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("");
    if !auth::valid_id(sid, &state.id_secret) {
        return json_resp(401, json!({ "success": false, "error": "auth required" }));
    }
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, sid) else {
        return json_resp(
            401,
            json!({ "success": false, "error": "identity missing" }),
        );
    };
    if !is_premium_email(state, &email) {
        return json_resp(403, json!({ "success": false, "error": "Premium only" }));
    }

    let Some(body) = parse_body(body_bytes) else {
        return json_resp(400, json!({ "success": false }));
    };
    let raw_code = body
        .get("code")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_uppercase();
    let desired: String = raw_code
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(20)
        .collect();
    if desired.len() < 3 {
        return json_resp(
            400,
            json!({ "success": false, "error": "Code must be 3–20 alphanumeric characters." }),
        );
    }

    let norm = auth::normalize_email(&email);
    let mut inv_codes = state
        .store
        .read_document(&data_file(state, "invite_codes.json"), json!({}));
    if let Some(map) = inv_codes.as_object() {
        for (k, v) in map {
            if v.as_str().unwrap_or("").trim().to_uppercase() == desired && k != &norm {
                return json_resp(
                    409,
                    json!({ "success": false, "error": "That code is already taken. Try another." }),
                );
            }
        }
    }

    if let Some(map) = inv_codes.as_object_mut() {
        map.insert(norm, json!(desired));
        let _ = state
            .store
            .write_document(&data_file(state, "invite_codes.json"), &inv_codes);
    }

    json_resp(200, json!({ "success": true, "code": desired }))
}

/// `POST /api/invite/send` (server.js:16439-16485).
fn invite_send(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let cookies = auth::get_cookies_from_header_value(
        headers
            .get(axum::http::header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or(""),
        &state.store,
        &state.id_secret,
        false,
    );
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("");
    if !auth::valid_id(sid, &state.id_secret) {
        return json_resp(401, json!({ "success": false, "error": "auth required" }));
    }
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, sid) else {
        return json_resp(
            401,
            json!({ "success": false, "error": "identity missing" }),
        );
    };
    let norm = auth::normalize_email(&email);

    let Some(body) = parse_body(body_bytes) else {
        return json_resp(400, json!({ "success": false }));
    };
    let to_email = body
        .get("to")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if to_email.is_empty() || !to_email.contains('@') {
        return json_resp(
            400,
            json!({ "success": false, "error": "Invalid email address." }),
        );
    }
    if auth::normalize_email(&to_email) == norm {
        return json_resp(
            400,
            json!({ "success": false, "error": "You can't invite yourself." }),
        );
    }

    let mut inv_codes = state
        .store
        .read_document(&data_file(state, "invite_codes.json"), json!({}));
    let code = match inv_codes.get(&norm).and_then(|v| v.as_str()) {
        Some(c) => c.to_string(),
        None => {
            let prefix: String = email
                .split('@')
                .next()
                .unwrap_or("USER")
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(12)
                .collect::<String>()
                .to_uppercase();
            let suffix = format!("{:03}", (rand::random::<u32>() % 900) + 100);
            let gen = format!("{prefix}{suffix}");
            if let Some(map) = inv_codes.as_object_mut() {
                map.insert(norm.clone(), json!(gen));
                let _ = state
                    .store
                    .write_document(&data_file(state, "invite_codes.json"), &inv_codes);
            }
            gen
        }
    };

    let mut inv_sent = state
        .store
        .read_document(&data_file(state, "invite_sent.json"), json!({}));
    let norm_to = auth::normalize_email(&to_email);
    let mut already_sent = false;
    if let Some(arr) = inv_sent.get(&norm).and_then(|v| v.as_array()) {
        already_sent = arr
            .iter()
            .any(|v| v.as_str().map(auth::normalize_email) == Some(norm_to.clone()));
    }
    if !already_sent {
        if let Some(map) = inv_sent.as_object_mut() {
            let list = map.entry(norm.clone()).or_insert_with(|| json!([]));
            if let Some(arr) = list.as_array_mut() {
                arr.push(json!(norm_to));
            }
            let _ = state
                .store
                .write_document(&data_file(state, "invite_sent.json"), &inv_sent);
        }
    }

    let invite_link = format!(
        "https://mitchdog.com/enroll?ref={}&email={}",
        encode_uri_component(&code),
        encode_uri_component(&to_email)
    );
    let sender_display = email.split('@').next().unwrap_or("A friend");
    let html = make_invite_friend_html(state, &to_email, sender_display, &invite_link);
    send_email_bg(
        state,
        &to_email,
        &format!("{sender_display} invited you to join mitch.pro"),
        &html,
    );

    json_resp(200, json!({ "success": true, "alreadySent": already_sent }))
}

/// `POST /api/suggest` (server.js:16488-16512).
async fn suggest(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(body) = parse_body(body_bytes) else {
        return json_resp(400, json!({ "success": false, "message": "bad json" }));
    };
    let ip = get_real_ip(headers, None);
    let recaptcha_token = body
        .get("recaptcha_token")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !verify_recaptcha(state, recaptcha_token, &ip, "").await {
        return json_resp(
            400,
            json!({ "success": false, "message": "reCAPTCHA failed. Please try again." }),
        );
    }

    let cookies = auth::get_cookies_from_header_value(
        headers
            .get(axum::http::header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or(""),
        &state.store,
        &state.id_secret,
        false,
    );
    let cookie_sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("");
    let user_id = body
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or(cookie_sid)
        .trim()
        .to_string();

    let sug_type = body
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("general")
        .trim()
        .chars()
        .take(50)
        .collect::<String>();
    let text = body
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .chars()
        .take(2000)
        .collect::<String>();
    if text.is_empty() {
        return json_resp(400, json!({ "success": false, "message": "Empty." }));
    }

    let names = state
        .store
        .read_document(&data_file(state, "names.json"), json!({}));
    let name = names
        .get(&user_id)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| user_id.chars().take(12).collect::<String>());

    let entry = json!({
        "id": user_id,
        "name": name,
        "type": sug_type,
        "text": text,
        "ts": (now_ms() as f64) / 1000.0
    });

    let mut sugs = state
        .store
        .read_document(&data_file(state, "suggestions.json"), json!([]));
    if let Some(arr) = sugs.as_array_mut() {
        arr.push(entry);
        let _ = state
            .store
            .write_document(&data_file(state, "suggestions.json"), &sugs);
    }

    let label = match sug_type.as_str() {
        "feedback" => "Feedback",
        "add_page" => "Page Suggestion",
        "broken_game" => "Broken Game Report",
        _ => "Suggestion",
    };
    ntfy_notify(&text, label, "default");

    json_resp(200, json!({ "success": true }))
}

// ── SSO Bridge ─────────────────────────────────────────────────────────────

fn parse_sso_e2e_private_jwk(raw: Option<&str>) -> Option<Value> {
    let raw_str = raw?.trim();
    if raw_str.is_empty() || raw_str.len() > 8192 {
        return None;
    }
    let cand: Value = serde_json::from_str(raw_str).ok()?;
    if cand.get("kty").and_then(|v| v.as_str()) == Some("EC")
        && cand.get("crv").and_then(|v| v.as_str()) == Some("P-256")
        && cand.get("x").is_some()
        && cand.get("y").is_some()
        && cand.get("d").is_some()
    {
        Some(json!({
            "kty": "EC",
            "crv": "P-256",
            "x": cand["x"],
            "y": cand["y"],
            "d": cand["d"],
            "ext": true
        }))
    } else {
        None
    }
}

/// `GET /api/sso/bridge` (server.js:15031-15145).
fn sso_bridge(state: &Arc<AppState>, headers: &HeaderMap, search: &str) -> Response {
    let req_h = request_host(headers);
    let query_map = crate::handler::query(search);
    let back_param = query_map
        .get("back")
        .cloned()
        .unwrap_or_else(|| "https://rjuhsd.school/".to_string());
    let Some(back_url) = sso_back_allowed(&state.cfg, &back_param, &req_h) else {
        return json_resp(400, json!({ "error": "Invalid back URL." }));
    };

    let self_host = req_h.split(':').next().unwrap_or("").to_ascii_lowercase();

    if state.check_password_cookie(headers, None) {
        let cookies = crate::routes::me::cookies_of(state, headers);
        let sid = cookies
            .get("studentId")
            .filter(|s| !s.is_empty())
            .or_else(|| cookies.get("id"))
            .unwrap_or("");
        let email = auth::email_from_sid(&state.store, &state.id_secret, sid).unwrap_or_default();
        if !email.is_empty() && auth::banned_info_for_email(&state.store, &email).is_none() {
            let token = random_bytes_hex(24);
            if let Ok(mut map) = state.sso_bridge_tokens.lock() {
                map.insert(
                    token.clone(),
                    SsoBridgeToken {
                        email: email.clone(),
                        expires: now_ms() + 90_000,
                        e2e_private_jwk: None,
                    },
                );
            }

            let back_host = back_url.host_str().unwrap_or("").to_ascii_lowercase();
            if !back_host.is_empty() && back_host != self_host {
                let bp = back_url.path();
                let is_matrix_or_game = bp == "/matrix"
                    || bp.starts_with("/matrix/")
                    || bp == "/games"
                    || bp.starts_with("/games/")
                    || bp == "/game-portal"
                    || bp.starts_with("/game-portal/")
                    || bp == "/msn-games"
                    || bp.starts_with("/msn-games/");
                let dest = format!(
                    "https://{}/api/sso/exchange?token={}&back={}",
                    back_host,
                    token,
                    encode_uri_component(back_url.as_str())
                );
                if is_matrix_or_game {
                    let mut resp = crate::static_files::redirect(&dest, 302);
                    resp.headers_mut().insert(
                        axum::http::header::CACHE_CONTROL,
                        HeaderValue::from_static("no-store"),
                    );
                    resp.headers_mut().insert(
                        axum::http::header::HeaderName::from_static("referrer-policy"),
                        HeaderValue::from_static("no-referrer"),
                    );
                    return resp;
                }
                let hop_html = format!(
                    r#"<!doctype html><meta charset="utf-8"><title>Signing in…</title>
<body></body>
<script>
(function(){{
  var jwk = "";
  try {{
    jwk = localStorage.getItem("_e2e_private_jwk_v3:{email}") || localStorage.getItem("_e2e_private_jwk") || "";
  }} catch (e) {{}}
  var f = document.createElement("form");
  f.action = "/api/sso/bridge/handoff";
  f.method = "POST";
  function add(n, v) {{ var i = document.createElement("input"); i.type = "hidden"; i.name = n; i.value = v; f.appendChild(i); }}
  add("token", {token_json});
  add("back", {back_json});
  if (jwk) add("e2ePrivateJwk", jwk);
  document.body.appendChild(f);
  f.submit();
}})();
</script>
"#,
                    token_json = json!(token),
                    back_json = json!(back_url.as_str())
                );
                let mut resp = Response::new(axum::body::Body::from(hop_html));
                resp.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/html; charset=utf-8"),
                );
                resp.headers_mut().insert(
                    axum::http::header::CACHE_CONTROL,
                    HeaderValue::from_static("no-store"),
                );
                return resp;
            }
            return crate::static_files::redirect(back_url.as_str(), 302);
        }
    }

    if is_rjuhsd_host(headers) {
        return crate::static_files::redirect(
            &format!("/enroll/?next={}", encode_uri_component(back_url.as_str())),
            302,
        );
    }

    let login_origin = "https://mitchdog.com";
    crate::static_files::redirect(
        &format!(
            "{login_origin}/enroll/?next={}",
            encode_uri_component(&format!(
                "{login_origin}/api/sso/bridge?back={}",
                encode_uri_component(back_url.as_str())
            ))
        ),
        302,
    )
}

/// `POST /api/sso/bridge/handoff` (server.js:15149-15190).
fn sso_bridge_handoff(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let req_h = request_host(headers);
    let form_str = String::from_utf8_lossy(body_bytes);
    let form = crate::handler::query(&form_str);
    let token = form.get("token").cloned().unwrap_or_default();

    let mut valid_rec = false;
    if let Ok(mut map) = state.sso_bridge_tokens.lock() {
        if let Some(rec) = map.get_mut(&token) {
            if now_ms() <= rec.expires && !rec.email.is_empty() {
                valid_rec = true;
                if let Some(jwk) =
                    parse_sso_e2e_private_jwk(form.get("e2ePrivateJwk").map(|s| s.as_str()))
                {
                    rec.e2e_private_jwk = Some(jwk);
                }
            }
        }
    }
    if !valid_rec {
        return crate::static_files::redirect("/?sso=expired", 302);
    }

    let back = form.get("back").cloned().unwrap_or_default();
    let Some(back_url) = sso_back_allowed(&state.cfg, &back, &req_h) else {
        return json_resp(400, json!({ "error": "Invalid back URL." }));
    };

    let back_host = back_url.host_str().unwrap_or("").to_ascii_lowercase();
    let dest = format!(
        "https://{}/api/sso/exchange?token={}&back={}",
        back_host,
        token,
        encode_uri_component(back_url.as_str())
    );

    let continue_html = format!(
        r#"<!doctype html><meta charset="utf-8"><title>Signing in…</title>
<script>location.replace({dest_json});</script>
"#,
        dest_json = json!(dest)
    );
    let mut resp = Response::new(axum::body::Body::from(continue_html));
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    resp
}

/// `GET / POST /api/sso/exchange` (server.js:15192-15250).
fn sso_exchange(
    state: &Arc<AppState>,
    method: &Method,
    headers: &HeaderMap,
    search: &str,
    body_bytes: &[u8],
) -> Response {
    let req_h = request_host(headers);
    if !is_rjuhsd_host(headers)
        && !is_pickle_host(headers)
        && !is_mitch_sso_host(&state.cfg, &req_h)
    {
        return json_resp(
            400,
            json!({ "error": "Exchange is not available on this host." }),
        );
    }

    let params = if *method == Method::POST {
        let form_str = String::from_utf8_lossy(body_bytes);
        crate::handler::query(&form_str)
    } else {
        crate::handler::query(search)
    };

    let token = params.get("token").cloned().unwrap_or_default();
    let mut token_rec: Option<SsoBridgeToken> = None;
    if let Ok(mut map) = state.sso_bridge_tokens.lock() {
        if let Some(rec) = map.remove(&token) {
            if now_ms() <= rec.expires {
                token_rec = Some(rec);
            }
        }
    }

    let Some(rec) = token_rec else {
        return crate::static_files::redirect("/?sso=expired", 302);
    };

    if auth::banned_info_for_email(&state.store, &rec.email).is_some() {
        return crate::static_files::redirect("/?sso=banned", 302);
    }

    let back_param = params
        .get("back")
        .cloned()
        .unwrap_or_else(|| "/".to_string());
    let dest_url = sso_back_allowed(&state.cfg, &back_param, &req_h)
        .map(|u| u.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());

    let mut resp = auth_success_response(
        state,
        headers,
        json!({ "success": true }),
        &rec.email,
        &rec.email,
    );

    let mut jwk = rec.e2e_private_jwk;
    if jwk.is_none() {
        jwk = parse_sso_e2e_private_jwk(params.get("e2ePrivateJwk").map(|s| s.as_str()));
    }

    if let Some(jwk_val) = jwk {
        let store_key = format!("_e2e_private_jwk_v3:{}", rec.email);
        let settle_html = format!(
            r#"<!doctype html><meta charset="utf-8"><title>Signed in</title>
<script>
(function(){{
  try {{ localStorage.setItem({key_json}, {jwk_json}); }} catch (e) {{}}
  location.replace({dest_json});
}})();
</script>
"#,
            key_json = json!(store_key),
            jwk_json = json!(jwk_val.to_string()),
            dest_json = json!(dest_url)
        );
        let mut html_resp = Response::new(axum::body::Body::from(settle_html));
        *html_resp.headers_mut() = resp.headers().clone();
        html_resp.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        html_resp.headers_mut().insert(
            axum::http::header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        );
        return html_resp;
    }

    resp.headers_mut().insert(
        axum::http::header::LOCATION,
        HeaderValue::from_str(&dest_url).unwrap_or_else(|_| HeaderValue::from_static("/")),
    );
    *resp.status_mut() = StatusCode::FOUND;
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hosts::SiteConfig;
    use crate::state::AppState;

    fn test_state() -> (Arc<AppState>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "mitch-server-auth-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap_or_default();
        let cfg = SiteConfig::load();
        let cfg = SiteConfig {
            data_dir: dir.join("data"),
            ..cfg
        };
        let store = Arc::new(
            mitch_lib::data::DataStore::open(&dir, &dir.join("data"))
                .unwrap_or_else(|e| panic!("store: {e}")),
        );
        (Arc::new(AppState::new(cfg, Arc::clone(&store))), dir)
    }

    #[tokio::test]
    async fn test_signup_and_verify_signup_flow() {
        std::env::set_var("NODE_ENV", "test");
        let (state, _dir) = test_state();

        let req_body = json!({
            "email": "alice@student.rjuhsd.us",
            "password": "CorrectHorseBatteryStaple123!",
            "username": "alice_bob",
            "nickname": "Alice"
        });
        let headers = HeaderMap::new();
        let resp = signup(&state, &headers, &serde_json::to_vec(&req_body).unwrap()).await;
        assert_eq!(resp.status(), StatusCode::OK);

        // Verify signup code was written
        let codes = state
            .store
            .read_document(&data_file(&state, "signup_codes.json"), json!({}));
        let norm = auth::normalize_email("alice@student.rjuhsd.us");
        let code = codes[&norm]["code"].as_str().unwrap().to_string();
        assert_eq!(code.len(), 6);

        // Verify signup with wrong code fails
        let verify_bad = json!({
            "email": "alice@student.rjuhsd.us",
            "code": "000000"
        });
        let resp_bad =
            verify_signup(&state, &headers, &serde_json::to_vec(&verify_bad).unwrap()).await;
        assert_eq!(resp_bad.status(), StatusCode::BAD_REQUEST);

        // Verify signup with correct code succeeds
        let verify_good = json!({
            "email": "alice@student.rjuhsd.us",
            "code": code
        });
        let resp_good =
            verify_signup(&state, &headers, &serde_json::to_vec(&verify_good).unwrap()).await;
        assert_eq!(resp_good.status(), StatusCode::OK);

        // Verify passwords and tokens
        let passwords = state
            .store
            .read_document(&data_file(&state, "passwords.json"), json!({}));
        assert!(passwords.get(&norm).is_some());
    }

    #[tokio::test]
    async fn test_login_and_logout_flow() {
        std::env::set_var("NODE_ENV", "test");
        let (state, _dir) = test_state();

        // Seed password
        let norm = auth::normalize_email("bob@student.rjuhsd.us");
        let hash = crypto::argon2_hash("Password123!");
        let mut passwords = json!({});
        passwords[norm.clone()] = json!(hash);
        state
            .store
            .write_document(&data_file(&state, "passwords.json"), &passwords)
            .unwrap();

        let headers = HeaderMap::new();
        let login_bad = json!({
            "email": "bob@student.rjuhsd.us",
            "password": "WrongPassword!"
        });
        let resp_bad = login(&state, &headers, &serde_json::to_vec(&login_bad).unwrap()).await;
        assert_eq!(resp_bad.status(), StatusCode::UNAUTHORIZED);

        let login_good = json!({
            "email": "bob@student.rjuhsd.us",
            "password": "Password123!"
        });
        let resp_good = login(&state, &headers, &serde_json::to_vec(&login_good).unwrap()).await;
        assert_eq!(resp_good.status(), StatusCode::OK);

        // Logout
        let resp_logout = logout(&state, &headers);
        assert_eq!(resp_logout.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_request_access_and_claim_token_flow() {
        std::env::set_var("NODE_ENV", "test");
        let (state, _dir) = test_state();

        // Seed account
        let norm = auth::normalize_email("charlie@student.rjuhsd.us");
        let hash = crypto::argon2_hash("OldPassword123!");
        let mut passwords = json!({});
        passwords[norm.clone()] = json!(hash);
        state
            .store
            .write_document(&data_file(&state, "passwords.json"), &passwords)
            .unwrap();

        let headers = HeaderMap::new();
        let reset_req = json!({
            "email": "charlie@student.rjuhsd.us"
        });
        let resp_req =
            request_access(&state, &headers, &serde_json::to_vec(&reset_req).unwrap()).await;
        assert_eq!(resp_req.status(), StatusCode::OK);

        // Find OTP
        let tokens = state
            .store
            .read_document(&data_file(&state, "tokens.json"), json!({}));
        let mut otp = String::new();
        for (_k, t) in tokens.as_object().unwrap() {
            if t["norm_email"] == norm && t["type"] == "reset" {
                otp = t["otp"].as_str().unwrap().to_string();
                break;
            }
        }
        assert_eq!(otp.len(), 6);

        // Claim token with OTP
        let claim_req = json!({
            "token": otp,
            "password": "NewSecretPassword123!"
        });
        let resp_claim =
            claim_token(&state, &headers, &serde_json::to_vec(&claim_req).unwrap()).await;
        assert_eq!(resp_claim.status(), StatusCode::OK);

        // Login with new password
        let login_req = json!({
            "email": "charlie@student.rjuhsd.us",
            "password": "NewSecretPassword123!"
        });
        let resp_login = login(&state, &headers, &serde_json::to_vec(&login_req).unwrap()).await;
        assert_eq!(resp_login.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_verify_2fa_flow() {
        std::env::set_var("NODE_ENV", "test");
        let (state, _dir) = test_state();

        let norm = auth::normalize_email("dave@student.rjuhsd.us");
        let hash = crypto::argon2_hash("Password123!");
        let mut passwords = json!({});
        passwords[norm.clone()] = json!(hash);
        state
            .store
            .write_document(&data_file(&state, "passwords.json"), &passwords)
            .unwrap();

        // Enable TOTP 2FA
        let totp_secret = totp::random_base32(32);
        let sealed_secret = totp::seal_totp_secret(&totp_secret, &state.id_secret);
        let mut profiles = json!({});
        profiles[norm.clone()] = json!({
            "twofa_enabled": true,
            "twofa_type": "totp",
            "totp_secret": sealed_secret
        });
        state
            .store
            .write_document(&data_file(&state, "profiles.json"), &profiles)
            .unwrap();

        let headers = HeaderMap::new();
        let login_req = json!({
            "email": "dave@student.rjuhsd.us",
            "password": "Password123!"
        });
        let resp_login = login(&state, &headers, &serde_json::to_vec(&login_req).unwrap()).await;
        assert_eq!(resp_login.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(resp_login.into_body(), usize::MAX)
            .await
            .unwrap();
        let login_val: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(login_val["twofa_required"], true);
        let temp_token = login_val["temp_token"].as_str().unwrap();

        // Verify with bad code
        let bad_2fa = json!({
            "temp_token": temp_token,
            "code": "000000"
        });
        let resp_bad = verify_2fa(&state, &headers, &serde_json::to_vec(&bad_2fa).unwrap());
        assert_eq!(resp_bad.status(), StatusCode::UNAUTHORIZED);

        // Verify with valid TOTP code
        let step = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            / 30;
        let valid_code = totp::generate_totp(&totp_secret, step);
        let good_2fa = json!({
            "temp_token": temp_token,
            "code": valid_code
        });
        let resp_good = verify_2fa(&state, &headers, &serde_json::to_vec(&good_2fa).unwrap());
        assert_eq!(resp_good.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_newsletter_signup_and_unsubscribe() {
        std::env::set_var("NODE_ENV", "test");
        let (state, _dir) = test_state();

        let email = "eve@student.rjuhsd.us";
        let norm = auth::normalize_email(email);
        let session = auth::create_auth_session(
            &state.store,
            &state.id_secret,
            &norm,
            email,
            "agent",
            "127.0.0.1",
            false,
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_str(&format!(
                "mitch_session={}; id={}",
                session.token, session.sid
            ))
            .unwrap(),
        );

        let req_body = json!({
            "email": email,
            "recaptcha_token": "test"
        });
        let resp =
            newsletter_signup(&state, &headers, &serde_json::to_vec(&req_body).unwrap()).await;
        assert_eq!(resp.status(), StatusCode::OK);

        // Verify extra list
        let extra = state
            .store
            .read_document(&data_file(&state, "newsletter_extra.json"), json!([]));
        assert!(extra.as_array().unwrap().iter().any(|v| v == email));

        // Direct unsubscribe
        let unsub_body = json!({});
        let resp_unsub = newsletter_unsubscribe_direct(
            &state,
            &headers,
            &serde_json::to_vec(&unsub_body).unwrap(),
        );
        assert_eq!(resp_unsub.status(), StatusCode::OK);

        let unsub_list = state
            .store
            .read_document(&data_file(&state, "newsletter_unsub.json"), json!([]));
        assert!(unsub_list.as_array().unwrap().iter().any(|v| v == &norm));
    }

    #[tokio::test]
    async fn test_invite_set_code_and_send() {
        std::env::set_var("NODE_ENV", "test");
        let (state, _dir) = test_state();

        let email = "frank@student.rjuhsd.us";
        let norm = auth::normalize_email(email);
        let session = auth::create_auth_session(
            &state.store,
            &state.id_secret,
            &norm,
            email,
            "agent",
            "127.0.0.1",
            false,
        );

        // Mark as approved premium
        let apps = json!([
            {
                "email": email,
                "status": "approved",
                "type": "premium"
            }
        ]);
        state
            .store
            .write_document(&data_file(&state, "applications.json"), &apps)
            .unwrap();

        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_str(&format!(
                "mitch_session={}; id={}",
                session.token, session.sid
            ))
            .unwrap(),
        );

        // Set code
        let set_req = json!({ "code": "FRANKIE" });
        let resp_set = invite_set_code(&state, &headers, &serde_json::to_vec(&set_req).unwrap());
        assert_eq!(resp_set.status(), StatusCode::OK);

        let codes = state
            .store
            .read_document(&data_file(&state, "invite_codes.json"), json!({}));
        assert_eq!(codes[&norm], "FRANKIE");

        // Send invite
        let send_req = json!({ "to": "grace@student.rjuhsd.us" });
        let resp_send = invite_send(&state, &headers, &serde_json::to_vec(&send_req).unwrap());
        assert_eq!(resp_send.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_sso_bridge_and_exchange() {
        std::env::set_var("NODE_ENV", "test");
        let (state, _dir) = test_state();

        let email = "heidi@student.rjuhsd.us";
        let norm = auth::normalize_email(email);
        let session = auth::create_auth_session(
            &state.store,
            &state.id_secret,
            &norm,
            email,
            "agent",
            "127.0.0.1",
            false,
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_str(&format!(
                "mitch_session={}; id={}",
                session.token, session.sid
            ))
            .unwrap(),
        );
        headers.insert(
            axum::http::header::HOST,
            HeaderValue::from_static("mitchdog.com"),
        );

        let resp_bridge = sso_bridge(&state, &headers, "back=https%3A%2F%2Frjuhsd.school%2Fgames");
        assert_eq!(resp_bridge.status(), StatusCode::FOUND);
        let loc = resp_bridge
            .headers()
            .get(axum::http::header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(loc.contains("/api/sso/exchange?token="));

        // Extract token
        let token = loc
            .split("token=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap();

        // Exchange token on destination host
        let mut dest_headers = HeaderMap::new();
        dest_headers.insert(
            axum::http::header::HOST,
            HeaderValue::from_static("rjuhsd.school"),
        );
        let resp_exchange = sso_exchange(
            &state,
            &Method::GET,
            &dest_headers,
            &format!("token={token}&back=https%3A%2F%2Frjuhsd.school%2Fgames"),
            &[],
        );
        assert_eq!(resp_exchange.status(), StatusCode::FOUND);
        let redir = resp_exchange
            .headers()
            .get(axum::http::header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(redir, "https://rjuhsd.school/games");
    }
}
