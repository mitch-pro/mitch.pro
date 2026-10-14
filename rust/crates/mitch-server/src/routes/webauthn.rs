//! WebAuthn / Passkeys HTTP endpoints (server.js:16288-16503, 26046-26058).
//!
//! Routes:
//! - POST /api/webauthn/login/options
//! - POST /api/webauthn/login/verify
//! - POST /api/webauthn/register/options
//! - POST /api/webauthn/register/verify
//! - POST /api/webauthn/credentials/rename
//! - POST /api/webauthn/credentials/delete
//! - GET  /api/webauthn/credentials

use std::path::PathBuf;
use std::sync::Arc;

use axum::http::{HeaderMap, Method};
use axum::response::Response;
use base64::Engine;
use serde_json::{json, Value};

use crate::errors::json_resp;
use crate::handler::get_real_ip;
use crate::routes::auth::resolve_login_identifier;
use crate::routes::me::account::auth_success_response;
use crate::routes::me::security::two_factor_config;
use crate::routes::push::send_email_bg;
use crate::state::{AppState, PendingTwoFactor};

fn data_file(state: &AppState, name: &str) -> PathBuf {
    state.cfg.data_dir.join(name)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn base64url(data: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

/// Builds one `PublicKeyCredentialDescriptor` (`{id, type, transports?}`)
/// for a stored credential, to list in `allowCredentials`/
/// `excludeCredentials`. `transports` is OMITTED (never sent as `null`)
/// when the stored record has none: the WebAuthn spec types it as an
/// optional `sequence<DOMString>`, and browsers throw a client-side
/// TypeError ("cannot be converted to a sequence") out of
/// `navigator.credentials.create()`/`.get()` if it's present but `null` --
/// which previously broke every login/registration attempt for an account
/// with even one credential whose transports were never recorded.
fn credential_descriptor(c: &Value) -> Value {
    let mut desc = serde_json::Map::new();
    desc.insert(
        "id".to_string(),
        c.get("id").cloned().unwrap_or(Value::Null),
    );
    desc.insert("type".to_string(), json!("public-key"));
    if let Some(transports) = c.get("transports").filter(|v| v.is_array()) {
        desc.insert("transports".to_string(), transports.clone());
    }
    Value::Object(desc)
}

pub fn webauthn_rp_for_host(state: &AppState, headers: &HeaderMap) -> Option<(String, String)> {
    let host = crate::hosts::request_host(headers).to_lowercase();
    let h = host.split(':').next().unwrap_or("").to_string();
    let node_env = std::env::var("NODE_ENV").unwrap_or_default();
    if node_env == "test" && (h == "localhost" || h == "127.0.0.1") {
        let origin = if host.contains(':') {
            format!("http://{host}")
        } else {
            format!("http://{h}")
        };
        return Some((h, origin));
    }
    let origins: Vec<String> = crate::hosts::mitch_sso_origins(&state.cfg)
        .into_iter()
        .collect();
    mitch_lib::webauthn::rp_for_host(&h, &origins)
}

fn get_cookies(headers: &HeaderMap) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let Some(val) = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
    else {
        return map;
    };
    for part in val.split(';') {
        let trimmed = part.trim();
        if let Some((k, v)) = trimmed.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map
}

fn passkeys_for_email(passkeys: &Value, norm_email: &str, rp_id: Option<&str>) -> Vec<Value> {
    let list = passkeys
        .get(norm_email)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    list.into_iter()
        .filter(|c| {
            if let Some(target_rp) = rp_id {
                c.get("rpId").and_then(|v| v.as_str()) == Some(target_rp)
            } else {
                true
            }
        })
        .collect()
}

pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Option<Response> {
    if path == "/api/webauthn/login/options" && *method == Method::POST {
        return Some(login_options(state, headers, body_bytes).await);
    }
    if path == "/api/webauthn/login/verify" && *method == Method::POST {
        return Some(login_verify(state, headers, body_bytes).await);
    }
    if path == "/api/webauthn/register/options" && *method == Method::POST {
        return Some(register_options(state, headers, body_bytes).await);
    }
    if path == "/api/webauthn/register/verify" && *method == Method::POST {
        return Some(register_verify(state, headers, body_bytes).await);
    }
    if (path == "/api/webauthn/credentials/rename" || path == "/api/webauthn/credentials/delete")
        && *method == Method::POST
    {
        return Some(credentials_modify(state, path, headers, body_bytes).await);
    }
    if path == "/api/webauthn/credentials" && *method == Method::GET {
        return Some(credentials_list(state, headers));
    }
    None
}

async fn login_options(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(rp) = webauthn_rp_for_host(state, headers) else {
        return json_resp(
            400,
            json!({ "success": false, "message": "Passkeys are not available on this domain." }),
        );
    };
    let body: Value = match serde_json::from_slice(body_bytes) {
        Ok(v) => v,
        Err(_) => {
            return json_resp(400, json!({ "success": false, "message": "bad json" }));
        }
    };

    let mut allow = Vec::new();
    let mut hint_email = String::new();
    let typed = body
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if !typed.is_empty() {
        if let Some(norm) = resolve_login_identifier(state, &typed.to_lowercase()) {
            let passkeys = state
                .store
                .read_document(&data_file(state, "passkeys.json"), json!({}));
            let matching = passkeys_for_email(&passkeys, &norm, Some(&rp.0));
            if !matching.is_empty() {
                allow = matching;
                hint_email = norm;
            }
        }
    }

    let challenge = base64url(&mitch_lib::crypto::random_bytes(32));
    let allow_credentials: Vec<Value> = allow.iter().map(credential_descriptor).collect();

    let options = json!({
        "rpId": rp.0,
        "challenge": challenge,
        "allowCredentials": allow_credentials,
        "timeout": 60000,
        "userVerification": "preferred",
    });

    state
        .webauthn_challenges
        .issue("login", &hint_email, &rp.0, Some(challenge));

    json_resp(200, json!({ "success": true, "options": options }))
}

async fn login_verify(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(rp) = webauthn_rp_for_host(state, headers) else {
        return json_resp(
            400,
            json!({ "success": false, "message": "Passkeys are not available on this domain." }),
        );
    };
    let body: Value = match serde_json::from_slice(body_bytes) {
        Ok(v) => v,
        Err(_) => {
            return json_resp(400, json!({ "success": false, "message": "bad json" }));
        }
    };

    let client_data_b64 = body
        .get("response")
        .and_then(|r| r.get("clientDataJSON"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let client_data_bytes = match mitch_lib::webauthn::decode_b64url_bytes(client_data_b64) {
        Ok(b) => b,
        Err(_) => {
            return json_resp(
                400,
                json!({ "success": false, "message": "Sign-in expired. Try again." }),
            );
        }
    };
    let client_data: Value = match serde_json::from_slice(&client_data_bytes) {
        Ok(v) => v,
        Err(_) => {
            return json_resp(
                400,
                json!({ "success": false, "message": "Sign-in expired. Try again." }),
            );
        }
    };

    let challenge = client_data
        .get("challenge")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let Some(issued) = state.webauthn_challenges.take(challenge, "login") else {
        return json_resp(
            400,
            json!({ "success": false, "message": "Sign-in expired. Try again." }),
        );
    };

    let passkeys_file = data_file(state, "passkeys.json");
    let mut passkeys = state.store.read_document(&passkeys_file, json!({}));

    let target_id = body.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let mut found_cred: Option<(Value, String, usize)> = None;

    if let Some(map) = passkeys.as_object() {
        'search: for (norm_email, list_val) in map {
            if let Some(list) = list_val.as_array() {
                for (idx, c) in list.iter().enumerate() {
                    if c.get("id").and_then(|v| v.as_str()) == Some(target_id) {
                        found_cred = Some((c.clone(), norm_email.clone(), idx));
                        break 'search;
                    }
                }
            }
        }
    }

    let Some((mut cred, cred_email, cred_idx)) = found_cred else {
        return json_resp(
            401,
            json!({ "success": false, "message": "Unknown passkey." }),
        );
    };

    if cred.get("rpId").and_then(|v| v.as_str()) != Some(&rp.0) {
        return json_resp(
            401,
            json!({ "success": false, "message": "Unknown passkey." }),
        );
    }

    if let Err(e) =
        mitch_lib::webauthn::verify_client_data(client_data_b64, "webauthn.get", &issued.key, &rp.1)
    {
        tracing::warn!("[webauthn] verify_client_data failed: {e}");
        return json_resp(
            401,
            json!({ "success": false, "message": "Passkey verification failed." }),
        );
    }

    let auth_data_b64 = body
        .get("response")
        .and_then(|r| r.get("authenticatorData"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let auth_data = match mitch_lib::webauthn::decode_b64url_bytes(auth_data_b64) {
        Ok(b) => b,
        Err(_) => {
            return json_resp(
                401,
                json!({ "success": false, "message": "Passkey verification failed." }),
            );
        }
    };

    let sig_b64 = body
        .get("response")
        .and_then(|r| r.get("signature"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let sig_bytes = match mitch_lib::webauthn::decode_b64url_bytes(sig_b64) {
        Ok(b) => b,
        Err(_) => {
            return json_resp(
                401,
                json!({ "success": false, "message": "Passkey verification failed." }),
            );
        }
    };

    let pub_key_b64 = cred.get("publicKey").and_then(|v| v.as_str()).unwrap_or("");
    let cose_key_bytes = match base64::engine::general_purpose::STANDARD.decode(pub_key_b64) {
        Ok(b) => b,
        Err(_) => {
            return json_resp(
                401,
                json!({ "success": false, "message": "Passkey verification failed." }),
            );
        }
    };

    if let Err(e) = mitch_lib::webauthn::verify_authentication_signature(
        &cose_key_bytes,
        &auth_data,
        client_data_b64,
        &sig_bytes,
    ) {
        tracing::warn!("[webauthn] signature verification failed: {e}");
        return json_resp(
            401,
            json!({ "success": false, "message": "Passkey verification failed." }),
        );
    }

    if let Some((new_counter, _flags)) =
        mitch_lib::webauthn::parse_auth_data_counter_flags(&auth_data)
    {
        let prev_counter = cred.get("counter").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        if new_counter > 0 && new_counter <= prev_counter {
            tracing::warn!("[webauthn] possible cloned authenticator: counter did not increase");
        }
        if let Some(map) = cred.as_object_mut() {
            map.insert("counter".to_string(), json!(new_counter));
        }
    }
    if let Some(map) = cred.as_object_mut() {
        map.insert("lastUsedAt".to_string(), json!(now_ms()));
    }

    if let Some(arr) = passkeys.get_mut(&cred_email).and_then(|v| v.as_array_mut()) {
        if cred_idx < arr.len() {
            arr[cred_idx] = cred;
        }
    }
    let _ = state.store.write_document(&passkeys_file, &passkeys);

    let ip = get_real_ip(headers, None);
    let twofa = two_factor_config(state, &cred_email);
    if twofa.enabled {
        let temp_token = mitch_lib::totp::create_temp_token();
        let mut rec = PendingTwoFactor {
            norm_email: cred_email.clone(),
            twofa_type: twofa.type_.clone(),
            code: None,
            attempts: 0,
            expires: now_ms() + 5 * 60 * 1000,
        };
        if twofa.type_ == "email" {
            let code = format!("{:06}", (rand::random::<u32>() % 900_000) + 100_000);
            rec.code = Some(code.clone());
            let html = crate::routes::auth::make_verification_code_html(
                state,
                "Login Two-Factor Authentication",
                &code,
                5,
                &cred_email,
            );
            send_email_bg(state, &cred_email, "Your mitch.pro login code", &html);
        }
        if let Ok(mut map) = state.pending_two_factor.lock() {
            map.insert(temp_token.clone(), rec);
        }
        let details = json!({ "email": cred_email, "type": twofa.type_, "ip": ip }).to_string();
        mitch_lib::log::append_app_log(
            &state.store,
            "info",
            "webauthn",
            "Passkey login requires 2FA",
            Some(&details),
        )
        .await;
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

    let details = json!({ "email": cred_email, "ip": ip }).to_string();
    mitch_lib::log::append_app_log(
        &state.store,
        "info",
        "webauthn",
        "Passkey login successful",
        Some(&details),
    )
    .await;

    auth_success_response(
        state,
        headers,
        json!({ "success": true }),
        &cred_email,
        &cred_email,
    )
}

async fn register_options(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    _body_bytes: &[u8],
) -> Response {
    let Some(rp) = webauthn_rp_for_host(state, headers) else {
        return json_resp(
            400,
            json!({ "success": false, "message": "Passkeys are not available on this domain." }),
        );
    };

    let cookies = get_cookies(headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .cloned()
        .unwrap_or_default();
    if sid.is_empty() || !mitch_lib::auth::valid_id(&sid, &state.id_secret) {
        return json_resp(401, json!({ "success": false, "message": "auth required" }));
    }

    let Some(email) = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid) else {
        return json_resp(
            403,
            json!({ "success": false, "message": "email not found" }),
        );
    };
    let norm_email = mitch_lib::auth::normalize_email(&email);

    let passkeys = state
        .store
        .read_document(&data_file(state, "passkeys.json"), json!({}));
    let existing = passkeys_for_email(&passkeys, &norm_email, Some(&rp.0));

    let challenge = base64url(&mitch_lib::crypto::random_bytes(32));
    let user_id = base64url(norm_email.as_bytes());
    let site = state
        .store
        .read_document(&data_file(state, "site.json"), json!({}));
    let site_name = site
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("mitch.pro");
    let rp_name = site_name;

    let exclude_credentials: Vec<Value> = existing.iter().map(credential_descriptor).collect();

    let options = json!({
        "challenge": challenge,
        "rp": {
            "name": rp_name,
            "id": rp.0,
        },
        "user": {
            "id": user_id,
            "name": norm_email,
            "displayName": email,
        },
        // ES256 only: verify_registration/cose_es256_key (mitch-lib) only
        // understand COSE kty=2 (EC2) / alg=-7 (ES256) keys. Advertising
        // -8 (EdDSA), -257 (RS256), or the non-standard -48 here let an
        // authenticator pick a key type the server could never verify --
        // it would register "successfully" client-side and then fail
        // every future login/registration-exclusion check with "stored
        // key not COSE ES256", a 400 the client only ever sees as a
        // generic "Passkey registration failed."
        "pubKeyCredParams": [
            { "alg": -7, "type": "public-key" }
        ],
        "timeout": 60000,
        "attestation": "none",
        "excludeCredentials": exclude_credentials,
        "authenticatorSelection": {
            "residentKey": "preferred",
            "userVerification": "preferred",
            "requireResidentKey": false
        },
        "extensions": {
            "credProps": true
        },
        "hints": []
    });

    state
        .webauthn_challenges
        .issue("register", &norm_email, &rp.0, Some(challenge));

    json_resp(200, json!({ "success": true, "options": options }))
}

async fn register_verify(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Response {
    let Some(rp) = webauthn_rp_for_host(state, headers) else {
        return json_resp(
            400,
            json!({ "success": false, "message": "Passkeys are not available on this domain." }),
        );
    };

    let cookies = get_cookies(headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .cloned()
        .unwrap_or_default();
    if sid.is_empty() || !mitch_lib::auth::valid_id(&sid, &state.id_secret) {
        return json_resp(401, json!({ "success": false, "message": "auth required" }));
    }

    let Some(email) = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid) else {
        return json_resp(
            403,
            json!({ "success": false, "message": "email not found" }),
        );
    };
    let norm_email = mitch_lib::auth::normalize_email(&email);

    let body: Value = match serde_json::from_slice(body_bytes) {
        Ok(v) => v,
        Err(_) => {
            return json_resp(400, json!({ "success": false, "message": "bad json" }));
        }
    };

    let client_data_b64 = body
        .get("response")
        .and_then(|r| r.get("clientDataJSON"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let client_data_bytes = match mitch_lib::webauthn::decode_b64url_bytes(client_data_b64) {
        Ok(b) => b,
        Err(_) => {
            return json_resp(
                400,
                json!({ "success": false, "message": "Registration expired. Try again." }),
            );
        }
    };
    let client_data: Value = match serde_json::from_slice(&client_data_bytes) {
        Ok(v) => v,
        Err(_) => {
            return json_resp(
                400,
                json!({ "success": false, "message": "Registration expired. Try again." }),
            );
        }
    };

    let challenge = client_data
        .get("challenge")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let Some(issued) = state.webauthn_challenges.take(challenge, "register") else {
        return json_resp(
            400,
            json!({ "success": false, "message": "Registration expired. Try again." }),
        );
    };

    if issued.email != norm_email {
        return json_resp(
            400,
            json!({ "success": false, "message": "Registration expired. Try again." }),
        );
    }

    let attestation_b64 = body
        .get("response")
        .and_then(|r| r.get("attestationObject"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let reg_info = match mitch_lib::webauthn::verify_registration(
        attestation_b64,
        client_data_b64,
        &issued.key,
        &rp.1,
        &rp.0,
    ) {
        Ok(info) => info,
        Err(e) => {
            tracing::warn!("[webauthn] verify_registration failed: {e}");
            return json_resp(
                400,
                json!({ "success": false, "message": "Passkey registration failed." }),
            );
        }
    };

    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let raw_name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let name = if !raw_name.is_empty() {
        raw_name[..raw_name.len().min(60)].to_string()
    } else {
        mitch_lib::webauthn::guess_credential_name(user_agent)
    };

    let transports = body
        .get("response")
        .and_then(|r| r.get("transports"))
        .cloned()
        .unwrap_or(Value::Null);

    let record = json!({
        "id": reg_info.credential_id,
        "publicKey": base64::engine::general_purpose::STANDARD.encode(&reg_info.cose_public_key),
        "counter": reg_info.counter,
        "transports": transports,
        "deviceType": "singleDevice",
        "backedUp": false,
        "name": name,
        "rpId": rp.0,
        "aaguid": reg_info.aaguid_hex,
        "createdAt": now_ms(),
        "lastUsedAt": Value::Null,
    });

    let passkeys_file = data_file(state, "passkeys.json");
    let mut passkeys = state.store.read_document(&passkeys_file, json!({}));

    if !passkeys.is_object() {
        passkeys = json!({});
    }
    let Some(obj) = passkeys.as_object_mut() else {
        return json_resp(
            500,
            json!({ "success": false, "message": "storage corrupt" }),
        );
    };
    let list = obj
        .entry(&norm_email)
        .or_insert_with(|| json!([]))
        .as_array_mut();

    if let Some(arr) = list {
        let rec_id = record.get("id").and_then(|v| v.as_str());
        let idx = arr
            .iter()
            .position(|c| c.get("id").and_then(|v| v.as_str()) == rec_id);
        if let Some(i) = idx {
            arr[i] = record.clone();
        } else {
            arr.push(record.clone());
        }
    }

    let _ = state.store.write_document(&passkeys_file, &passkeys);

    let ip = get_real_ip(headers, None);
    let details = json!({ "email": norm_email, "rpId": rp.0, "ip": ip }).to_string();
    mitch_lib::log::append_app_log(
        &state.store,
        "info",
        "webauthn",
        "Passkey registered",
        Some(&details),
    )
    .await;

    json_resp(
        200,
        json!({
            "success": true,
            "credential": mitch_lib::webauthn::public_credential_view(&record)
        }),
    )
}

async fn credentials_modify(
    state: &Arc<AppState>,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Response {
    let cookies = get_cookies(headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .cloned()
        .unwrap_or_default();
    if sid.is_empty() || !mitch_lib::auth::valid_id(&sid, &state.id_secret) {
        return json_resp(401, json!({ "success": false, "message": "auth required" }));
    }

    let Some(email) = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid) else {
        return json_resp(
            403,
            json!({ "success": false, "message": "email not found" }),
        );
    };
    let norm_email = mitch_lib::auth::normalize_email(&email);

    let body: Value = match serde_json::from_slice(body_bytes) {
        Ok(v) => v,
        Err(_) => {
            return json_resp(400, json!({ "success": false, "message": "bad json" }));
        }
    };

    let id = body.get("id").and_then(|v| v.as_str()).unwrap_or("");
    if id.is_empty() {
        return json_resp(
            404,
            json!({ "success": false, "message": "Passkey not found." }),
        );
    }

    let passkeys_file = data_file(state, "passkeys.json");
    let mut passkeys = state.store.read_document(&passkeys_file, json!({}));

    let mut found = false;
    if let Some(obj) = passkeys.as_object_mut() {
        if let Some(list) = obj.get_mut(&norm_email).and_then(|v| v.as_array_mut()) {
            let idx = list
                .iter()
                .position(|c| c.get("id").and_then(|v| v.as_str()) == Some(id));
            if let Some(i) = idx {
                found = true;
                if path == "/api/webauthn/credentials/rename" {
                    let name = body
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim();
                    if name.is_empty() {
                        return json_resp(
                            400,
                            json!({ "success": false, "message": "Name required." }),
                        );
                    }
                    let truncated = &name[..name.len().min(60)];
                    if let Some(cred_map) = list[i].as_object_mut() {
                        cred_map.insert("name".to_string(), json!(truncated));
                    }
                } else {
                    list.remove(i);
                }
            }
            if list.is_empty() {
                obj.remove(&norm_email);
            }
        }
    }

    if !found {
        return json_resp(
            404,
            json!({ "success": false, "message": "Passkey not found." }),
        );
    }

    let _ = state.store.write_document(&passkeys_file, &passkeys);
    json_resp(200, json!({ "success": true }))
}

fn credentials_list(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let rp = webauthn_rp_for_host(state, headers);
    let cookies = get_cookies(headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .cloned()
        .unwrap_or_default();
    if sid.is_empty() || !mitch_lib::auth::valid_id(&sid, &state.id_secret) {
        return json_resp(401, json!({ "success": false, "message": "auth required" }));
    }

    let Some(email) = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid) else {
        return json_resp(
            403,
            json!({ "success": false, "message": "email not found" }),
        );
    };
    let norm_email = mitch_lib::auth::normalize_email(&email);

    let passkeys = state
        .store
        .read_document(&data_file(state, "passkeys.json"), json!({}));
    let creds = passkeys_for_email(&passkeys, &norm_email, None);
    let public_creds: Vec<Value> = creds
        .iter()
        .map(mitch_lib::webauthn::public_credential_view)
        .collect();

    json_resp(
        200,
        json!({
            "success": true,
            "currentRpId": rp.map(|r| r.0),
            "credentials": public_creds
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: a stored credential with no transports recorded must
    /// produce a descriptor with the key OMITTED, never `"transports":
    /// null` -- the latter crashes navigator.credentials.create()/.get()
    /// client-side with a TypeError before any network request is sent,
    /// which broke every login/registration for an account with such a
    /// credential (e.g. one created by an authenticator/browser combo that
    /// never reports transports).
    #[test]
    fn credential_descriptor_omits_null_transports() {
        let cred = json!({ "id": "abc123", "transports": Value::Null });
        let desc = credential_descriptor(&cred);
        assert_eq!(desc.get("id"), Some(&json!("abc123")));
        assert_eq!(desc.get("type"), Some(&json!("public-key")));
        assert!(
            desc.get("transports").is_none(),
            "transports must be omitted, not null: {desc:?}"
        );

        let cred_missing = json!({ "id": "def456" });
        let desc2 = credential_descriptor(&cred_missing);
        assert!(desc2.get("transports").is_none());

        let cred_with = json!({ "id": "ghi789", "transports": ["usb", "nfc"] });
        let desc3 = credential_descriptor(&cred_with);
        assert_eq!(desc3.get("transports"), Some(&json!(["usb", "nfc"])));
    }
}
