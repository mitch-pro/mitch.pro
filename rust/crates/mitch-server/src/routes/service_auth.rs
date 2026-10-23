//! Dedicated Woodcreek account handoff. Mitch session proof stays encrypted;
//! Woodcreek receives a scoped bearer grant, never a reusable Mitch cookie.
use crate::state::AppState;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use axum::http::{HeaderMap, HeaderValue, Method};
use axum::response::Response;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use mitch_lib::{auth, crypto, service_auth as storage};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

const PREFIX: &str = "/api/woodcreek/auth/";
const CALLBACK: &str = "https://woodcreek.site/auth/callback";
const SESSION_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const PURPOSE: &[u8] = b"woodcreek-account-v1";

#[derive(Serialize, Deserialize)]
struct Grant {
    aud: String,
    exp: i64,
    proof: String,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn key(state: &AppState) -> [u8; 32] {
    crypto::hmac_sha256(&state.id_secret, PURPOSE)
}
fn hash(value: &str) -> String {
    crypto::sha256_hex(value.as_bytes())
}
fn response(status: u16, value: Value) -> Response {
    let mut response = crate::errors::json_resp(status, value);
    response.headers_mut().insert(
        "cache-control",
        HeaderValue::from_static("private, no-store"),
    );
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response
}
fn redirect(target: &str) -> Response {
    let mut response = crate::static_files::redirect(target, 302);
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response
}
fn url_token(value: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn challenge(verifier: &str) -> Option<String> {
    if !(43..=128).contains(&verifier.len())
        || !verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
    {
        return None;
    }
    // SHA-256 is available through the shared helper as hexadecimal.
    let digest = crypto::sha256_hex(verifier.as_bytes());
    let bytes: Option<Vec<u8>> = (0..digest.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&digest[i..i + 2], 16).ok())
        .collect();
    Some(URL_SAFE_NO_PAD.encode(bytes?))
}

pub fn is_path(path: &str) -> bool {
    path.starts_with(PREFIX)
}

/// This grant is accepted only by the dedicated introspection/revoke endpoints.
/// Its AEAD key is purpose-separated from every other use of ID_SECRET.
#[allow(deprecated)]
pub fn issue(state: &AppState, headers: &HeaderMap) -> Option<String> {
    crate::presence::authenticated_email(state, headers)?;
    let cookies = crate::routes::me::cookies_of(state, headers);
    let mut parts = Vec::new();
    for name in ["mitch_session", "password", "studentId", "id"] {
        if let Some(value) = cookies.get(name).filter(|value| !value.is_empty()) {
            parts.push(format!("{name}={}", auth::encode_uri_component(value)));
        }
    }
    let grant = Grant {
        aud: "woodcreek.site".into(),
        exp: now() + SESSION_MS,
        proof: parts.join("; "),
    };
    let plaintext = serde_json::to_vec(&grant).ok()?;
    let nonce: [u8; 12] = rand::random();
    let cipher = Aes256Gcm::new_from_slice(&key(state)).ok()?;
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext.as_ref())
        .ok()?;
    let mut bytes = nonce.to_vec();
    bytes.extend(ciphertext);
    Some(format!("wc1.{}", URL_SAFE_NO_PAD.encode(bytes)))
}

#[allow(deprecated)]
fn open(state: &AppState, token: &str) -> Option<Grant> {
    if token.len() > 4096 {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(token.strip_prefix("wc1.")?).ok()?;
    if bytes.len() < 28 {
        return None;
    }
    let cipher = Aes256Gcm::new_from_slice(&key(state)).ok()?;
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&bytes[..12]), &bytes[12..])
        .ok()?;
    let grant: Grant = serde_json::from_slice(&plaintext).ok()?;
    (grant.aud == "woodcreek.site" && grant.exp > now() && grant.exp <= now() + SESSION_MS)
        .then_some(grant)
}
fn identity(state: &AppState, token: &str) -> Option<(Grant, String)> {
    let grant = open(state, token)?;
    if storage::is_revoked(&state.store, &hash(token), now()) {
        return None;
    }
    let mut headers = HeaderMap::new();
    headers.insert("cookie", HeaderValue::from_str(&grant.proof).ok()?);
    let email = crate::presence::authenticated_email(state, &headers)?;
    Some((grant, email))
}
fn account(state: &AppState, email: &str) -> Value {
    let profile = mitch_lib::profiles::profile_get(&state.store, email);
    let processed = mitch_lib::profile::process_member_fields(
        &state.store,
        state.data_dir(),
        email,
        Some(&profile),
        Some(email),
    );
    let username = profile
        .get("username")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| mitch_lib::profile::default_username_for_email(email));
    let pfp = mitch_lib::profile::sanitize_profile_image_url(
        profile.get("pfp").and_then(Value::as_str).unwrap_or(""),
        true,
        1000,
        120_000,
    );
    let cosmetics = mitch_lib::shop::cosmetics_get(&state.store, email).unwrap_or(json!({}));
    let color = mitch_lib::shop::public_active_color(
        &state.store,
        email,
        cosmetics.get("activeColor").unwrap_or(&Value::Null),
    );
    json!({"username":username,"displayName":processed.get("displayName"),"pfp":pfp,"profileUrl":"https://mitchdog.com/profile/",
        "activeColor":color,"activeBadge":cosmetics.get("activeBadge"),"activeProfileEffect":cosmetics.get("activeProfileEffect")})
}

pub fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    search: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Response {
    if path == format!("{PREFIX}authorize") && method == Method::GET {
        let query: std::collections::HashMap<String, String> =
            form_urlencoded::parse(search.trim_start_matches('?').as_bytes())
                .into_owned()
                .collect();
        let state_value = query.get("state").map(String::as_str).unwrap_or("");
        let code_challenge = query
            .get("code_challenge")
            .map(String::as_str)
            .unwrap_or("");
        if !url_token(state_value, 32, 128)
            || !url_token(code_challenge, 43, 43)
            || query.get("code_challenge_method").map(String::as_str) != Some("S256")
        {
            return response(400, json!({"error":"Invalid sign-in request."}));
        }
        let Some(token) = issue(state, headers) else {
            let next = format!("{PREFIX}authorize?{}", search.trim_start_matches('?'));
            return redirect(&format!(
                "/enroll/?next={}",
                auth::encode_uri_component(&next)
            ));
        };
        let code = URL_SAFE_NO_PAD.encode(crypto::random_bytes(32));
        if storage::save_code(
            &state.store,
            &hash(&code),
            code_challenge,
            &token,
            now() + 90_000,
            now(),
        )
        .is_err()
        {
            return response(503, json!({"error":"Sign-in is temporarily unavailable."}));
        }
        return redirect(&format!("{CALLBACK}?code={code}&state={state_value}"));
    }
    if method != Method::POST {
        return response(405, json!({"error":"Method not allowed."}));
    }
    if body.len() > 8192 {
        return response(413, json!({"error":"Request is too large."}));
    }
    if path == format!("{PREFIX}token") {
        let data: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let code = data.get("code").and_then(Value::as_str).unwrap_or("");
        let verifier = data
            .get("code_verifier")
            .and_then(Value::as_str)
            .unwrap_or("");
        let Some(challenge) = challenge(verifier).filter(|_| url_token(code, 43, 43)) else {
            return response(400, json!({"error":"Invalid sign-in code."}));
        };
        let Some(token) = storage::take_code(&state.store, &hash(code), &challenge, now()) else {
            return response(
                400,
                json!({"error":"Sign-in code expired or already used."}),
            );
        };
        let Some((grant, email)) = identity(state, &token) else {
            return response(401, json!({"error":"Sign in again."}));
        };
        return response(
            200,
            json!({"access_token":token,"token_type":"Bearer","expires_in":(grant.exp-now())/1000,"account":account(state,&email)}),
        );
    }
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    let Some((grant, email)) = identity(state, token) else {
        return response(401, json!({"active":false,"error":"Sign in again."}));
    };
    if path == format!("{PREFIX}session") {
        return response(
            200,
            json!({"active":true,"expires_at":grant.exp,"account":account(state,&email)}),
        );
    }
    if path == format!("{PREFIX}revoke") {
        return if storage::revoke(&state.store, &hash(token), grant.exp, now()).is_ok() {
            response(200, json!({"success":true}))
        } else {
            response(
                503,
                json!({"error":"Sign out could not finish. Please try again."}),
            )
        };
    }
    response(404, json!({"error":"Not found."}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pkce_matches_rfc7636_and_rejects_weak_verifiers() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk").as_deref(),
            Some("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM")
        );
        assert!(challenge("short").is_none());
        assert!(challenge(&"!".repeat(43)).is_none());
    }
}
