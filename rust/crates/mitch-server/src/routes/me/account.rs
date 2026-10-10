//! `/api/me/change-email(/confirm)`, `/api/me/change-password`,
//! `/api/me/logout-other`, `GET /api/me` (server.js:16035-16077,
//! 17202-17259, 18736-18821) plus `renameEmailReferences`
//! (server.js:2626-2693) and `authSuccessResponse` (server.js:2949-2957).
//!
//! `change-password` deliberately matches the JS and accepts ANY method.

use super::security::verify_password_change_second_factor;
use super::{cookies_of, data_file, json_response, me_uid, parse_body_strict};
use crate::routes::push::{ntfy_notify, send_email_bg, verify_recaptcha};
use crate::state::{AppState, PendingEmailChange};
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::auth;
use mitch_lib::jsval;
use serde_json::{json, Value};
use std::sync::Arc;

pub(crate) async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Option<Response> {
    if path == "/api/profile" && *method == Method::GET {
        return Some(get_profile(state, headers));
    }
    if path.starts_with("/api/profile/") && *method == Method::GET {
        let slug = &path["/api/profile/".len()..];
        return Some(get_public_profile(state, headers, slug));
    }
    if path == "/api/profile" && *method == Method::POST {
        return Some(post_profile(state, headers, body_bytes));
    }
    if path == "/api/me/change-email" && method == Method::POST {
        return Some(change_email(state, headers, body_bytes));
    }
    if path == "/api/me/change-email/confirm" && method == Method::POST {
        return Some(change_email_confirm(state, headers, body_bytes));
    }
    // JS: `if (path === '/api/me/change-password')` — no method check.
    if path == "/api/me/change-password" {
        return Some(change_password(state, headers, body_bytes).await);
    }
    if path == "/api/me/logout-other" && method == Method::POST {
        return Some(logout_other(state, headers));
    }
    if path == "/api/me" {
        return Some(me_root(state, headers));
    }
    None
}

fn get_public_profile(state: &Arc<AppState>, headers: &HeaderMap, slug_raw: &str) -> Response {
    let slug = auth::decode_uri_component(slug_raw);
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    let viewer_email = auth::email_from_sid(&state.store, &state.id_secret, &sid);
    let actual_email = mitch_lib::profile::email_from_hash(
        &state.store,
        state.data_dir(),
        &state.id_secret,
        &slug,
    )
    .unwrap_or(slug);
    let profiles = state
        .store
        .read_document(&data_file(state, "profiles.json"), json!({}));
    let norm = auth::normalize_email(&actual_email);
    let mut profile = profiles.get(&norm).cloned().unwrap_or_else(|| {
        json!({
            "email": actual_email,
            "username": mitch_lib::profile::default_username_for_email(&actual_email),
            "displayName": "",
            "bio": "",
            "pfp": "",
            "background": ""
        })
    });
    let cosm = state
        .store
        .read_document(&data_file(state, "cosmetics.json"), json!({}));
    let processed = mitch_lib::profile::process_member_fields(
        &state.store,
        state.data_dir(),
        &actual_email,
        Some(&profile),
        viewer_email.as_deref(),
    );
    if let Some(obj) = profile.as_object_mut() {
        obj.remove("totp_secret");
        obj.remove("totpSecret");
        obj.remove("pendingTotpSecret");
        obj.remove("twofa_enabled");
        obj.remove("twofa_type");
        obj.remove("twofaEnabled");
        obj.remove("twofaType");
        obj.remove("twoFactorEnabled");
        let pfp = obj.get("pfp").and_then(|v| v.as_str()).unwrap_or("");
        obj.insert(
            "pfp".into(),
            json!(mitch_lib::profile::sanitize_profile_image_url(
                pfp, true, 1000, 120_000
            )),
        );
        let bg = obj.get("background").and_then(|v| v.as_str()).unwrap_or("");
        obj.insert(
            "background".into(),
            json!(mitch_lib::profile::sanitize_profile_image_url(
                bg, false, 1000, 0
            )),
        );
        let website = obj.get("website").and_then(|v| v.as_str()).unwrap_or("");
        obj.insert(
            "website".into(),
            json!(mitch_lib::profile::sanitize_profile_website_url(website)),
        );
    }
    let is_premium = auth::is_premium_email(&state.store, &actual_email);
    let is_admin = auth::is_admin_email(&state.store, &actual_email);
    let is_moderator = auth::is_moderator_email(&state.store, &actual_email);
    let is_tester = auth::is_tester_email(&state.store, &actual_email);
    let user_stats = state
        .store
        .read_document(&data_file(state, "user_stats.json"), json!({}));
    let stats = user_stats.get(&norm).cloned().unwrap_or(json!({}));
    let achievements_doc = state
        .store
        .read_document(&data_file(state, "achievements.json"), json!({}));
    let achievements = achievements_doc.get(&norm).cloned().unwrap_or(json!([]));
    let coins = mitch_lib::coins::get_coins(&state.store, state.data_dir(), &actual_email);
    let user_cosm = cosm.get(&norm).cloned().unwrap_or(json!({}));

    let mut resp_obj = profile;
    if let Some(map) = resp_obj.as_object_mut() {
        map.insert(
            "displayName".into(),
            processed.get("displayName").cloned().unwrap_or(json!("")),
        );
        map.insert(
            "email".into(),
            processed.get("email").cloned().unwrap_or(json!("")),
        );
        map.insert("isPremium".into(), json!(is_premium));
        map.insert("isAdmin".into(), json!(is_admin));
        map.insert("isModerator".into(), json!(is_moderator));
        map.insert("isTester".into(), json!(is_tester));
        map.insert("unlimitedCoins".into(), json!(false));
        map.insert("coins".into(), json!(coins));
        map.insert("stats".into(), stats);
        map.insert("achievements".into(), achievements);
        map.insert(
            "totalAchievementsCount".into(),
            json!(mitch_lib::achievements::ACHIEVEMENT_DEFINITIONS.len()),
        );
        map.insert(
            "activeColor".into(),
            mitch_lib::shop::public_active_color(
                &state.store,
                &actual_email,
                user_cosm.get("activeColor").unwrap_or(&Value::Null),
            )
            .map(|c| json!(c))
            .unwrap_or(Value::Null),
        );
        map.insert(
            "activeBadge".into(),
            jsval::or(user_cosm.get("activeBadge"), json!(Value::Null)),
        );
        map.insert(
            "activeProfileEffect".into(),
            jsval::or(user_cosm.get("activeProfileEffect"), json!(Value::Null)),
        );
    }
    json_response(200, resp_obj)
}

fn get_profile(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &sid) else {
        return json_response(401, json!({ "error": "not logged in" }));
    };
    let profiles = state
        .store
        .read_document(&data_file(state, "profiles.json"), json!({}));
    let norm = auth::normalize_email(&email);
    let mut profile = profiles.get(&norm).cloned().unwrap_or_else(|| {
        json!({
            "username": mitch_lib::profile::default_username_for_email(&email),
            "displayName": "",
            "bio": "",
            "pfp": "",
            "background": ""
        })
    });
    let cosm = state
        .store
        .read_document(&data_file(state, "cosmetics.json"), json!({}));
    let processed = mitch_lib::profile::process_member_fields(
        &state.store,
        state.data_dir(),
        &email,
        Some(&profile),
        Some(&email),
    );
    if let Some(obj) = profile.as_object_mut() {
        obj.remove("totp_secret");
        obj.remove("totpSecret");
        obj.remove("pendingTotpSecret");
        let pfp = obj.get("pfp").and_then(|v| v.as_str()).unwrap_or("");
        obj.insert(
            "pfp".into(),
            json!(mitch_lib::profile::sanitize_profile_image_url(
                pfp, true, 1000, 120_000
            )),
        );
        let bg = obj.get("background").and_then(|v| v.as_str()).unwrap_or("");
        obj.insert(
            "background".into(),
            json!(mitch_lib::profile::sanitize_profile_image_url(
                bg, false, 1000, 0
            )),
        );
        let website = obj.get("website").and_then(|v| v.as_str()).unwrap_or("");
        obj.insert(
            "website".into(),
            json!(mitch_lib::profile::sanitize_profile_website_url(website)),
        );
    }
    let is_premium = auth::is_premium_email(&state.store, &email);
    let is_admin = auth::is_admin_email(&state.store, &email);
    let is_moderator = auth::is_moderator_email(&state.store, &email);
    let is_tester = auth::is_tester_email(&state.store, &email);
    let user_stats = state
        .store
        .read_document(&data_file(state, "user_stats.json"), json!({}));
    let stats = user_stats.get(&norm).cloned().unwrap_or(json!({}));
    let achievements_doc = state
        .store
        .read_document(&data_file(state, "achievements.json"), json!({}));
    let achievements = achievements_doc.get(&norm).cloned().unwrap_or(json!([]));
    let coins = mitch_lib::coins::get_coins(&state.store, state.data_dir(), &email);
    let user_cosm = cosm.get(&norm).cloned().unwrap_or(json!({}));

    let mut resp_obj = profile;
    if let Some(map) = resp_obj.as_object_mut() {
        map.insert(
            "displayName".into(),
            map.get("displayName")
                .filter(|v| jsval::truthy(v))
                .cloned()
                .unwrap_or(json!("")),
        );
        map.insert(
            "email".into(),
            processed.get("email").cloned().unwrap_or(json!("")),
        );
        map.insert("isPremium".into(), json!(is_premium));
        map.insert("isAdmin".into(), json!(is_admin));
        map.insert("isModerator".into(), json!(is_moderator));
        map.insert("isTester".into(), json!(is_tester));
        map.insert("unlimitedCoins".into(), json!(false));
        map.insert("coins".into(), json!(coins));
        map.insert("stats".into(), stats);
        map.insert("achievements".into(), achievements);
        map.insert(
            "totalAchievementsCount".into(),
            json!(mitch_lib::achievements::ACHIEVEMENT_DEFINITIONS.len()),
        );
        map.insert(
            "activeColor".into(),
            mitch_lib::shop::public_active_color(
                &state.store,
                &email,
                user_cosm.get("activeColor").unwrap_or(&Value::Null),
            )
            .map(|c| json!(c))
            .unwrap_or(Value::Null),
        );
        map.insert(
            "activeBadge".into(),
            jsval::or(user_cosm.get("activeBadge"), json!(Value::Null)),
        );
        map.insert(
            "activeProfileEffect".into(),
            jsval::or(user_cosm.get("activeProfileEffect"), json!(Value::Null)),
        );
        map.insert(
            "profileBonusClaimed".into(),
            map.get("profileBonusClaimed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
                .into(),
        );
    }
    json_response(200, resp_obj)
}

fn post_profile(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &sid) else {
        return json_response(401, json!({ "error": "not logged in" }));
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let norm = auth::normalize_email(&email);
    let is_premium = auth::is_premium_email(&state.store, &email);
    let profiles_path = data_file(state, "profiles.json");
    let mut profiles = state.store.read_document(&profiles_path, json!({}));
    let existing = profiles.get(&norm).cloned().unwrap_or(json!({}));

    let req_username = body
        .get("username")
        .and_then(|v| v.as_str())
        .or_else(|| existing.get("username").and_then(|v| v.as_str()))
        .unwrap_or("");
    let username = mitch_lib::profile::normalize_username(req_username);
    let username = if username.is_empty() {
        mitch_lib::profile::default_username_for_email(&norm)
    } else {
        username
    };

    let pfp_raw = body.get("pfp").and_then(|v| v.as_str()).unwrap_or("");
    let bg_raw = body
        .get("background")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let website_raw = body.get("website").and_then(|v| v.as_str()).unwrap_or("");

    let safe_pfp = mitch_lib::profile::sanitize_profile_image_url(pfp_raw, true, 1000, 120_000);
    let safe_bg = mitch_lib::profile::sanitize_profile_image_url(bg_raw, false, 1000, 0);
    let safe_website = mitch_lib::profile::sanitize_profile_website_url(website_raw);

    if !pfp_raw.trim().is_empty() && safe_pfp.is_empty() {
        return json_response(
            400,
            json!({ "error": "Profile picture must be http(s) or a small PNG/JPEG/WebP/GIF image." }),
        );
    }
    if is_premium && !bg_raw.trim().is_empty() && safe_bg.is_empty() {
        return json_response(
            400,
            json!({ "error": "Background image must be an http(s) image URL." }),
        );
    }
    if !website_raw.trim().is_empty() && safe_website.is_empty() {
        return json_response(400, json!({ "error": "Website must be an http(s) URL." }));
    }

    let display_name = body
        .get("displayName")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let display_name = &display_name[..display_name.len().min(40)];

    let nickname = body
        .get("nickname")
        .and_then(|v| v.as_str())
        .or_else(|| existing.get("nickname").and_then(|v| v.as_str()))
        .unwrap_or("")
        .trim();
    let nickname = &nickname[..nickname.len().min(40)];

    let bio = body
        .get("bio")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let bio = &bio[..bio.len().min(300)];

    let grad_year = body
        .get("gradYear")
        .and_then(|v| v.as_str())
        .or_else(|| existing.get("gradYear").and_then(|v| v.as_str()))
        .unwrap_or("")
        .trim();
    let grad_year = &grad_year[..grad_year.len().min(20)];

    let gender = body
        .get("gender")
        .and_then(|v| v.as_str())
        .or_else(|| existing.get("gender").and_then(|v| v.as_str()))
        .unwrap_or("")
        .trim();
    let gender = &gender[..gender.len().min(40)];

    let referral_source = body
        .get("referralSource")
        .and_then(|v| v.as_str())
        .or_else(|| existing.get("referralSource").and_then(|v| v.as_str()))
        .unwrap_or("")
        .trim();
    let referral_source = &referral_source[..referral_source.len().min(80)];

    let mut record = existing.clone();
    if let Some(map) = record.as_object_mut() {
        map.insert("email".into(), json!(email));
        map.insert("username".into(), json!(username));
        map.insert("nickname".into(), json!(nickname));
        map.insert("displayName".into(), json!(display_name));
        map.insert("bio".into(), json!(bio));
        map.insert("website".into(), json!(safe_website));
        map.insert("pfp".into(), json!(safe_pfp));
        map.insert(
            "background".into(),
            json!(if is_premium {
                safe_bg
            } else {
                existing
                    .get("background")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            }),
        );
        map.insert("gradYear".into(), json!(grad_year));
        map.insert("gender".into(), json!(gender));
        map.insert("referralSource".into(), json!(referral_source));
    } else {
        record = json!({
            "email": email,
            "username": username,
            "nickname": nickname,
            "displayName": display_name,
            "bio": bio,
            "website": safe_website,
            "pfp": safe_pfp,
            "background": if is_premium { safe_bg } else { String::new() },
            "gradYear": grad_year,
            "gender": gender,
            "referralSource": referral_source,
        });
    }

    let now = mitch_lib::school::now_millis();
    if let Some(map) = record.as_object_mut() {
        map.insert("updatedAt".into(), json!(now));
    }

    if let Some(map) = profiles.as_object_mut() {
        map.insert(norm.clone(), record.clone());
    }
    let _ = state.store.write_document(&profiles_path, &profiles);

    crate::ws::broadcast_profile_change(state, &username, now);

    let state_clone = Arc::clone(state);
    let uid_clone = sid.to_string();
    let dn_clone = record
        .get("displayName")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let pfp_clone = record
        .get("pfp")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let bio_clone = record
        .get("bio")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    tokio::spawn(async move {
        crate::routes::matrix::sync_profile_to_matrix(
            &state_clone,
            &uid_clone,
            dn_clone.as_deref(),
            pfp_clone.as_deref(),
            bio_clone.as_deref(),
            None,
        )
        .await;
    });

    json_response(200, json!({ "profile": record }))
}

fn change_email(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let cookies = cookies_of(state, headers);
    let uid = me_uid(&cookies);
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &uid) else {
        return json_response(401, json!({ "error": "not logged in" }));
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let new_email = jsval::string(&jsval::or(body.get("newEmail"), json!("")))
        .trim()
        .to_lowercase();
    let new_norm = auth::normalize_email(&new_email);
    if !new_email.contains('@') || !new_norm.contains('@') {
        return json_response(400, json!({ "error": "invalid email" }));
    }
    let old_norm = auth::normalize_email(&email);
    if old_norm == new_norm {
        return json_response(400, json!({ "error": "new email matches current email" }));
    }
    let passwords = state
        .store
        .read_document(&data_file(state, "passwords.json"), json!({}));
    if passwords.get(&new_norm).is_some() {
        return json_response(400, json!({ "error": "email already in use" }));
    }
    let code = format!("{}", (100000.0 + js_rand() * 900000.0) as i64);
    let token = mitch_lib::totp::create_temp_token();
    state
        .pending_email_changes
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            token.clone(),
            PendingEmailChange {
                old_norm,
                new_norm,
                new_email: new_email.clone(),
                code: code.clone(),
                expires: mitch_lib::school::now_millis() + 30 * 60 * 1000,
                attempts: 0,
            },
        );
    let html = super::security::make_verification_code_html(
        state,
        "Email Change Request",
        &code,
        30.0,
        &new_email,
    );
    send_email_bg(
        state,
        &new_email,
        "Confirm your mitch.pro email change",
        &html,
    );
    json_response(200, json!({ "ok": true, "change_token": token }))
}

fn change_email_confirm(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let cookies = cookies_of(state, headers);
    let uid = me_uid(&cookies);
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &uid) else {
        return json_response(401, json!({ "error": "not logged in" }));
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    // JS: String(body.change_token || body.token || '').trim()
    let token = jsval::string(&jsval::or(
        body.get("change_token").filter(|v| jsval::truthy(v)),
        jsval::or(body.get("token"), json!("")),
    ))
    .trim()
    .to_string();
    let mut changes = state
        .pending_email_changes
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let valid = match changes.get(&token) {
        Some(rec) => {
            mitch_lib::school::now_millis() <= rec.expires
                && rec.old_norm == auth::normalize_email(&email)
        }
        None => false,
    };
    if !valid {
        changes.remove(&token);
        drop(changes);
        return json_response(401, json!({ "error": "email change expired" }));
    }
    let rec = changes
        .get_mut(&token)
        .unwrap_or_else(|| unreachable!("entry checked above"));
    rec.attempts += 1;
    if rec.attempts > 5 {
        changes.remove(&token);
        drop(changes);
        return json_response(429, json!({ "error": "too many attempts" }));
    }
    let supplied = jsval::string(&jsval::or(body.get("code"), json!("")))
        .trim()
        .to_string();
    if supplied != rec.code {
        drop(changes);
        return json_response(400, json!({ "error": "invalid code" }));
    }
    let rec = changes
        .remove(&token)
        .unwrap_or_else(|| unreachable!("entry checked above"));
    drop(changes);
    rename_email_references(state, &rec.old_norm, &rec.new_norm, &rec.new_email);
    auth::invalidate_auth_sessions_for_email(&state.store, &rec.old_norm, None);
    auth::rotate_session_generation(&state.store, &rec.new_norm);
    ntfy_notify(
        &format!("Email changed: {} -> {}", rec.old_norm, rec.new_norm),
        "Security",
        "",
    );
    auth_success_response(
        state,
        headers,
        json!({ "ok": true }),
        &rec.new_norm,
        &rec.new_email,
    )
}

async fn change_password(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Response {
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "success": false, "message": "bad json" }));
    };

    let ip = crate::handler::get_real_ip(headers, None);
    let cookies = cookies_of(state, headers);
    let uid = me_uid(&cookies);
    // JS: verifyRecaptcha(body.recaptcha_token || '', ip) — no sid.
    if !verify_recaptcha(
        state,
        &jsval_string_of(body.get("recaptcha_token")),
        &ip,
        "",
    )
    .await
    {
        return json_response(
            400,
            json!({ "success": false, "message": "reCAPTCHA failed." }),
        );
    }
    if !state.check_password_cookie(headers, non_empty(&uid)) {
        return json_response(
            401,
            json!({ "success": false, "message": "Auth required." }),
        );
    }

    // JS: emailFromSid may return null → normalizeEmail(null) is '' and the
    // flow continues, failing at the stored-password lookup.
    let email = auth::email_from_sid(&state.store, &state.id_secret, &uid).unwrap_or_default();
    let norm = auth::normalize_email(&email);

    let current_password = jsval_string_of(body.get("currentPassword"));
    let new_password = jsval_string_of(body.get("newPassword"));
    let confirm_password = jsval_string_of(body.get("confirmPassword"));
    if current_password.is_empty() || new_password.is_empty() || confirm_password.is_empty() {
        return json_response(
            400,
            json!({ "success": false, "message": "All fields required." }),
        );
    }
    let (pwd_ok, pwd_error) = is_secure_password(state, &new_password);
    if !pwd_ok {
        return json_response(400, json!({ "success": false, "message": pwd_error }));
    }
    if new_password != confirm_password {
        return json_response(
            400,
            json!({ "success": false, "message": "New passwords do not match." }),
        );
    }

    let passwords_file = data_file(state, "passwords.json");
    let mut passwords = state.store.read_document(&passwords_file, json!({}));
    let stored = passwords.get(&norm).and_then(|v| v.as_str()).unwrap_or("");
    if stored.is_empty() || !mitch_lib::crypto::argon2_verify(stored, &current_password) {
        return json_response(
            401,
            json!({ "success": false, "message": "Current password incorrect." }),
        );
    }

    // JS: body.verificationCode || body.emailCode
    let second_factor_code = jsval::or(
        body.get("verificationCode").filter(|v| jsval::truthy(v)),
        jsval::or(body.get("emailCode"), json!("")),
    );
    let verified = verify_password_change_second_factor(state, &norm, Some(&second_factor_code));
    if !verified.ok {
        return json_response(
            verified.status,
            json!({ "success": false, "message": verified.error }),
        );
    }

    if let Some(map) = passwords.as_object_mut() {
        map.insert(
            norm.clone(),
            json!(mitch_lib::crypto::argon2_hash(&new_password)),
        );
    }
    let _ = state.store.write_document(&passwords_file, &passwords);
    auth::rotate_session_generation(&state.store, &norm);

    auth_success_response(state, headers, json!({ "success": true }), &norm, &email)
}

fn logout_other(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let cookies = cookies_of(state, headers);
    let uid = me_uid(&cookies);
    if !state.check_password_cookie(headers, non_empty(&uid)) {
        return json_response(
            401,
            json!({ "success": false, "message": "Auth required." }),
        );
    }
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &uid) else {
        return json_response(
            401,
            json!({ "success": false, "message": "Auth required." }),
        );
    };
    let norm = auth::normalize_email(&email);
    auth::rotate_session_generation(&state.store, &norm);
    auth_success_response(state, headers, json!({ "success": true }), &norm, &email)
}

/// `GET /api/me` (server.js:18736-18821).
fn me_root(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let cookies = cookies_of(state, headers);
    let uid = me_uid(&cookies);
    if !auth::valid_id(&uid, &state.id_secret) {
        return json_response(401, json!({ "error": "Not authenticated" }));
    }
    if let Some(ban) = auth::banned_info_for_sid(&state.store, &state.id_secret, &uid) {
        let reason = ban
            .get("reason")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("This account is banned from the website.");
        return json_response(
            403,
            json!({ "error": "account banned", "banned": true, "reason": reason }),
        );
    }
    let email = auth::email_from_sid(&state.store, &state.id_secret, &uid);
    let has_email = email.is_some();
    let email_str = email.clone().unwrap_or_default();
    let is_premium = has_email && auth::is_premium_email(&state.store, &email_str);
    let node_env_test = std::env::var("NODE_ENV").unwrap_or_default() == "test";
    let is_admin = auth::is_admin_id(&state.store, &state.id_secret, &uid, node_env_test);
    let is_moderator = auth::is_moderator_id(&state.store, &state.id_secret, &uid);
    let is_co_owner = has_email && auth::is_co_owner_email(&state.store, &email_str);
    let is_owner = has_email && auth::is_owner_email(&state.store, &email_str);
    let is_blog_contributor = has_email && is_blog_contributor_email(state, &email_str);
    let can_grant_premium =
        mitch_lib::admin::can_grant_premium_id(&state.store, &state.id_secret, &uid);

    let norm = email
        .as_ref()
        .map(|e| auth::normalize_email(e))
        .unwrap_or_default();
    let stats = if has_email {
        state
            .store
            .read_document(&data_file(state, "user_stats.json"), json!({}))
            .get(&norm)
            .cloned()
            .unwrap_or(json!({}))
    } else {
        json!({})
    };
    let cosmetics = if has_email {
        state
            .store
            .read_document(&data_file(state, "cosmetics.json"), json!({}))
            .get(&norm)
            .cloned()
            .unwrap_or(json!({}))
    } else {
        json!({})
    };

    // Server-derived fallback key from before password-wrapped backups
    // existed (server.js:18753-18779).
    let mut pub_key_hex: Option<String> = None;
    let mut encrypted_private_jwk: Option<Value> = None;
    let mut iv_hex: Option<String> = None;
    let mut kdf_salt_hex = String::new();
    let mut kdf_iterations: i64 = 0;
    let mut key_history: Vec<Value> = Vec::new();
    let mut legacy_jwk: Option<Value> = None;
    if let Some(ref email) = email {
        let (jwk, legacy_pub_hex) = mitch_lib::e2e::derive_user_e2e_keys(&state.id_secret, email);
        legacy_jwk = Some(jwk);
        let norm = auth::normalize_email(email);
        let entry = state
            .store
            .read_document(&data_file(state, "e2e_keys.json"), json!({}))
            .get(&norm)
            .cloned();
        match entry {
            Some(entry) if entry.is_object() => {
                pub_key_hex = entry
                    .get("pubKeyHex")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                encrypted_private_jwk = entry.get("encryptedPrivateJwk").cloned();
                iv_hex = entry
                    .get("ivHex")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                kdf_salt_hex = entry
                    .get("kdfSaltHex")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                kdf_iterations = entry
                    .get("kdfIterations")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                key_history = entry
                    .get("history")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().take(5).cloned().collect())
                    .unwrap_or_default();
            }
            _ => {
                pub_key_hex = Some(legacy_pub_hex);
            }
        }
    }

    let my_profile = if has_email {
        state
            .store
            .read_document(&data_file(state, "profiles.json"), json!({}))
            .get(&norm)
            .cloned()
            .unwrap_or(json!({}))
    } else {
        json!({})
    };
    let my_username = if has_email {
        let raw = jsval::string(&jsval::or(
            my_profile.get("username").filter(|v| jsval::truthy(v)),
            json!(mitch_lib::profile::default_username_for_email(&norm)),
        ));
        mitch_lib::profile::normalize_username(&raw)
    } else {
        String::new()
    };
    let display_name = jsval::string(&jsval::or(
        my_profile.get("displayName").filter(|v| jsval::truthy(v)),
        jsval::or(
            my_profile.get("nickname").filter(|v| jsval::truthy(v)),
            jsval::or(Some(&json!(my_username)), json!("")),
        ),
    ));

    let happy_hour_active = state
        .happy_hour_active
        .load(std::sync::atomic::Ordering::Relaxed);
    let computed = state.happy_hour();
    let happy_hour_message = if happy_hour_active {
        format!(
            "HAPPY HOUR IS ACTIVE! Earn 2X Coins on games & canvas! (Runs {}) 🎰",
            mitch_lib::school::format_school_hour(computed)
        )
    } else {
        format!(
            "Happy Hour today: {} (Based on yesterday's least used school hour!) 🍻",
            mitch_lib::school::format_school_hour(computed)
        )
    };

    let role = if is_co_owner {
        "co-owner"
    } else if is_owner {
        "owner"
    } else if is_admin {
        "admin"
    } else if is_moderator {
        "moderator"
    } else {
        "member"
    };

    json_response(
        200,
        json!({
            "email": mitch_lib::admin::mask_email(&email_str),
            "rawEmail": email.as_ref().map(|e| json!(e)).unwrap_or(Value::Null),
            "normEmail": norm,
            "displayEmail": email.as_ref().map(|e| mitch_lib::profile::display_email(&state.store, state.data_dir(), &state.id_secret, e)).unwrap_or_default(),
            "username": my_username,
            "displayName": display_name,
            "isPremium": is_premium,
            "isAdmin": is_admin,
            "isModerator": is_moderator,
            "isOwner": is_owner,
            "isCoOwner": is_co_owner,
            "role": role,
            "isBlogContributor": is_blog_contributor,
            "canBlogPost": has_email && can_write_blog_email(state, &email_str),
            "canGrantPremium": can_grant_premium,
            "vipUntil": stats.get("vip_casino_until").and_then(|v| v.as_i64()).unwrap_or(0),
            "activeTheme": jsval::string(&jsval::or(cosmetics.get("activeTheme"), json!(""))),
            "activeAi": jsval::string(&jsval::or(cosmetics.get("activeAi"), json!(""))),
            "jwk": legacy_jwk,
            "legacyJwk": legacy_jwk,
            "pubKeyHex": pub_key_hex,
            "encryptedPrivateJwk": encrypted_private_jwk,
            "ivHex": iv_hex,
            "kdfSaltHex": kdf_salt_hex,
            "kdfIterations": kdf_iterations,
            "keyHistory": key_history,
            "premium_email": jsval_or_null(stats.get("premium_email")),
            "happyHour": {
                "active": happy_hour_active,
                "message": happy_hour_message,
            },
        }),
    )
}

// ── shared helpers ───────────────────────────────────────────────────────────

/// `authSuccessResponse(req, payload, normEmail, originalEmail)` — creates the
/// session and sets the four cookies (server.js:2949-2957).
pub(crate) fn auth_success_response(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    payload: Value,
    norm_email: &str,
    original_email: &str,
) -> Response {
    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let ip = crate::handler::get_real_ip(headers, None);
    let session = auth::create_auth_session(
        &state.store,
        &state.id_secret,
        norm_email,
        original_email,
        user_agent,
        &ip,
        false,
    );
    let secure_flag = std::env::var("SESSION_COOKIE_SECURE").unwrap_or_default();
    let node_env_production = std::env::var("NODE_ENV").unwrap_or_default() == "production";
    let max_age = auth::AUTH_SESSION_TTL_MS / 1000;
    let cookie_value = |value: String| {
        axum::http::HeaderValue::from_str(&value)
            .unwrap_or_else(|_| unreachable!("cookie header is ASCII-safe"))
    };
    let mut obj = payload;
    if let Some(map) = obj.as_object_mut() {
        map.insert("id".to_string(), json!(session.sid));
        map.insert(
            "email".to_string(),
            json!(if original_email.is_empty() {
                norm_email
            } else {
                original_email
            }),
        );
    }
    let mut resp = json_response(200, obj);
    let append = |resp: &mut Response, value: String| {
        resp.headers_mut()
            .append(axum::http::header::SET_COOKIE, cookie_value(value));
    };
    append(
        &mut resp,
        auth::set_cookie_header(
            "mitch_session",
            &session.token,
            &secure_flag,
            node_env_production,
            max_age,
            true,
        ),
    );
    append(
        &mut resp,
        auth::set_cookie_header(
            "studentId",
            &session.sid,
            &secure_flag,
            node_env_production,
            max_age,
            false,
        ),
    );
    append(
        &mut resp,
        auth::clear_cookie_header("password", &secure_flag, node_env_production, false),
    );
    append(
        &mut resp,
        auth::clear_cookie_header("id", &secure_flag, node_env_production, false),
    );
    resp
}

/// `isSecurePassword(password)` (server.js:2111-2127) → (valid, error).
pub(crate) fn is_secure_password(state: &Arc<AppState>, password: &str) -> (bool, &'static str) {
    // JS .length is the UTF-16 length.
    if password.is_empty() || password.encode_utf16().count() < 8 {
        return (false, "Password must be at least 8 characters long.");
    }
    let bad: Vec<String> = state
        .store
        .read_document(&data_file(state, "bad_passwords.json"), json!([]))
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_lowercase()))
                .collect()
        })
        .unwrap_or_default();
    if bad.contains(&password.to_lowercase()) {
        return (false, "Password is too common and insecure.");
    }
    (true, "")
}

/// `isBlogContributorEmail(email)` (server.js:6446-6450).
pub(crate) fn is_blog_contributor_email(state: &Arc<AppState>, email: &str) -> bool {
    if email.is_empty() {
        return false;
    }
    let norm = auth::normalize_email(email);
    let raw = state
        .store
        .read_document(&data_file(state, "blog_contributors.json"), json!([]));
    let contributors: Vec<String> = match &raw {
        Value::Array(a) => a
            .iter()
            .filter(|v| jsval::truthy(v))
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        Value::Object(m) => m
            .iter()
            .filter(|(_, active)| active.as_bool() != Some(false))
            .map(|(email, _)| email.clone())
            .collect(),
        _ => Vec::new(),
    };
    contributors
        .iter()
        .any(|c| auth::normalize_email(c) == norm)
}

/// `canWriteBlogEmail(email)` (server.js:6452-6454).
pub(crate) fn can_write_blog_email(state: &Arc<AppState>, email: &str) -> bool {
    !email.is_empty()
        && (auth::is_admin_email(&state.store, email)
            || auth::is_moderator_email(&state.store, email)
            || is_blog_contributor_email(state, email))
}

/// `renameEmailReferences(oldNorm, newNorm, newEmail)` (server.js:2626-2693).
/// The `rebuildCoreTablesFromDocuments()` tail is deferred with the rest of
/// the SQL-table sync work.
pub(crate) fn rename_email_references(
    state: &Arc<AppState>,
    old_norm: &str,
    new_norm: &str,
    new_email: &str,
) {
    // coins is a real table now, not a JSON blob key to rename in the
    // generic loop below — it needs its own UPDATE.
    mitch_lib::coins::rename_coins_email(&state.store, old_norm, new_norm);
    let key_maps = [
        "passwords.json",
        "profiles.json",
        "user_stats.json",
        "achievements.json",
        "daily_logins.json",
        "cosmetics.json",
        "coin_gifts.json",
        "invite_codes.json",
        "invite_claims.json",
        "invite_sent.json",
        "premium_gifts_sent.json",
        "e2e_keys.json",
        "dm_cleared.json",
        "push_subs.json",
        "sebastians_claims.json",
        "piccolo_sessions.json",
    ];
    for file in key_maps {
        let path = data_file(state, file);
        let mut obj = state.store.read_document(&path, json!({}));
        let Some(value) = obj.get(old_norm).cloned() else {
            continue;
        };
        if let Some(map) = obj.as_object_mut() {
            map.insert(new_norm.to_string(), value);
            map.remove(old_norm);
            if file == "profiles.json" {
                if let Some(rec) = map.get_mut(new_norm) {
                    if let Some(inner) = rec.as_object_mut() {
                        inner.insert("email".to_string(), json!(new_email));
                    }
                }
            }
        }
        let _ = state.store.write_document(&path, &obj);
    }

    // names table — any sid pointing at the old email moves to the new one.
    auth::names_update_email_for_norm(&state.store, old_norm, new_email);

    // tokens.json — email + norm_email on matching records.
    let tokens_path = data_file(state, "tokens.json");
    let mut tokens = state.store.read_document(&tokens_path, json!({}));
    if let Some(map) = tokens.as_object_mut() {
        for rec in map.values_mut() {
            let rec_norm = rec
                .get("email")
                .or_else(|| rec.get("norm_email"))
                .and_then(|v| v.as_str())
                .map(auth::normalize_email)
                .unwrap_or_default();
            if rec_norm == old_norm {
                if let Some(inner) = rec.as_object_mut() {
                    inner.insert("email".to_string(), json!(new_email));
                    inner.insert("norm_email".to_string(), json!(new_norm));
                }
            }
        }
    }
    let _ = state.store.write_document(&tokens_path, &tokens);

    // friends.json — move the key, then remap list entries.
    let friends_path = data_file(state, "friends.json");
    let mut friends = state.store.read_document(&friends_path, json!({}));
    if let Some(map) = friends.as_object_mut() {
        if let Some(list) = map.remove(old_norm) {
            map.insert(new_norm.to_string(), list);
        }
        for value in map.values_mut() {
            if let Some(arr) = value.as_array_mut() {
                for f in arr.iter_mut() {
                    if let Some(s) = f.as_str() {
                        if auth::normalize_email(s) == old_norm {
                            *f = json!(new_norm);
                        }
                    }
                }
            }
        }
    }
    let _ = state.store.write_document(&friends_path, &friends);

    // friend_requests.json — remap from/to.
    let requests_path = data_file(state, "friend_requests.json");
    let mut requests = state.store.read_document(&requests_path, json!([]));
    if let Some(arr) = requests.as_array_mut() {
        for req in arr.iter_mut() {
            for key in ["from", "to"] {
                if let Some(s) = req.get(key).and_then(|v| v.as_str()) {
                    if auth::normalize_email(s) == old_norm {
                        if let Some(inner) = req.as_object_mut() {
                            inner.insert(key.to_string(), json!(new_norm));
                        }
                    }
                }
            }
        }
    }
    let _ = state.store.write_document(&requests_path, &requests);

    // Array documents: every string equal (normalized) to oldNorm becomes the
    // NEW DISPLAY email — nested values included (server.js:2679-2690).
    for file in [
        "dms.json",
        "public_chat.json",
        "premium_chat.json",
        "applications.json",
        "sessions.json",
    ] {
        let path = data_file(state, file);
        let mut data = state.store.read_document(&path, json!([]));
        rewrite_norm_strings(&mut data, old_norm, new_email);
        let _ = state.store.write_document(&path, &data);
    }
}

/// The recursive string rewrite from server.js:2681-2688.
fn rewrite_norm_strings(value: &mut Value, old_norm: &str, new_email: &str) {
    match value {
        Value::String(s) => {
            if auth::normalize_email(s) == old_norm {
                *value = json!(new_email);
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                rewrite_norm_strings(v, old_norm, new_email);
            }
        }
        Value::Object(map) => {
            for v in map.values_mut() {
                rewrite_norm_strings(v, old_norm, new_email);
            }
        }
        _ => {}
    }
}

/// `Math.random()` — f64 in [0, 1) like JS.
pub(crate) fn js_rand() -> f64 {
    use rand::Rng;
    rand::rng().random::<f64>()
}

/// `String(x || '')` for optional body fields.
fn jsval_string_of(v: Option<&Value>) -> String {
    jsval::string(&jsval::or(v, json!("")))
}

/// The check_password_cookie sid argument (None when empty, like JS passing
/// an empty string that fails validId).
fn non_empty(s: &str) -> Option<&str> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// `stats.premium_email || null` — null when absent.
fn jsval_or_null(v: Option<&Value>) -> Value {
    match v {
        Some(val) if jsval::truthy(val) => val.clone(),
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::HeaderValue;

    fn test_state() -> (Arc<AppState>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "mitch-server-account-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap_or_default();
        let cfg = crate::hosts::SiteConfig::load();
        let cfg = crate::hosts::SiteConfig {
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
    async fn public_profile_by_slug_returns_sanitized_profile() {
        let (state, _dir) = test_state();
        let email = "testuser@student.rjuhsd.us";
        let norm = auth::normalize_email(email);
        let profiles_file = data_file(&state, "profiles.json");
        let _ = state.store.write_document(
            &profiles_file,
            &json!({
                norm.clone(): {
                    "email": email,
                    "username": "testuser",
                    "displayName": "Test User",
                    "bio": "Hello world",
                    "pfp": "https://example.com/pfp.png",
                    "totp_secret": "SUPER_SECRET",
                    "twofa_enabled": true
                }
            }),
        );

        // Fetch by slug (username)
        let resp = get_public_profile(&state, &HeaderMap::new(), "testuser");
        assert_eq!(resp.status(), 200);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let val: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            val.get("username").and_then(|v| v.as_str()),
            Some("testuser")
        );
        assert_eq!(
            val.get("displayName").and_then(|v| v.as_str()),
            Some("Test User")
        );
        assert_eq!(val.get("bio").and_then(|v| v.as_str()), Some("Hello world"));
        assert!(val.get("totp_secret").is_none());
        assert!(val.get("twofa_enabled").is_none());
        assert_eq!(val.get("coins").and_then(|v| v.as_f64()), Some(0.0));
    }

    #[tokio::test]
    async fn authenticated_profile_get_and_post() {
        let (state, _dir) = test_state();
        let email = "alice@student.rjuhsd.us";
        let sess = auth::create_auth_session(
            &state.store,
            &state.id_secret,
            &auth::normalize_email(email),
            email,
            "",
            "",
            false,
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_str(&format!(
                "mitch_session={}; studentId={}",
                sess.token, sess.sid
            ))
            .unwrap(),
        );

        // GET profile initially
        let resp = get_profile(&state, &headers);
        assert_eq!(resp.status(), 200);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let val: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(val.get("username").and_then(|v| v.as_str()), Some("alice"));

        // POST profile update
        let body = json!({
            "displayName": "Alice Wonderland",
            "bio": "Curiouser and curiouser",
            "pfp": "https://example.com/alice.png",
            "website": "https://alice.test"
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let post_resp = post_profile(&state, &headers, &body_bytes);
        assert_eq!(post_resp.status(), 200);

        // Public profile lookup shows new display name and website
        let pub_resp = get_public_profile(&state, &HeaderMap::new(), "alice");
        assert_eq!(pub_resp.status(), 200);
        let pub_bytes = to_bytes(pub_resp.into_body(), usize::MAX).await.unwrap();
        let pub_val: Value = serde_json::from_slice(&pub_bytes).unwrap();
        assert_eq!(
            pub_val.get("displayName").and_then(|v| v.as_str()),
            Some("Alice Wonderland")
        );
        assert_eq!(
            pub_val.get("bio").and_then(|v| v.as_str()),
            Some("Curiouser and curiouser")
        );
        assert_eq!(
            pub_val.get("website").and_then(|v| v.as_str()),
            Some("https://alice.test/")
        );
    }
}
