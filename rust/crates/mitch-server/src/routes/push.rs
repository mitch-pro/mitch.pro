//! Push + email side-channels (plan Step 8). These are fire-and-forget
//! channels: failures never alter the HTTP response, matching the JS's
//! `proc.unref()` + silent-catch semantics.
//! - `push_admin_notification` — VAPID web push against data/push_subs.json
//!   via the `web-push` crate (no-op without VAPID env, like the JS gate).
//! - `ntfy_notify` — POST to ntfy.sh with the configured topic.
//! - `send_email_bg` — POST to the mitch-mail service (:6902), the same
//!   transport the mail shims use since Step 2.

#![allow(clippy::expect_used)] // infallible static regexes
use crate::state::AppState;
use std::sync::Arc;

/// `pushAdminNotification(targetEmail, title, message)` — server.js:3125.
/// Reads push_subs.json, best-effort send, 410/404 cleanup, all async.
pub fn push_admin_notification(
    state: &Arc<AppState>,
    target_email: &str,
    title: &str,
    message: &str,
) {
    let vapid_public = std::env::var("VAPID_PUBLIC_KEY")
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    if vapid_public.is_empty() {
        return; // JS parity: `if (!VAPID_PUBLIC) return;`
    }
    let subs = state.store.read_document(
        &state.cfg.data_dir.join("push_subs.json"),
        serde_json::json!({}),
    );
    let norm = mitch_lib::auth::normalize_email(target_email);
    let sub = subs
        .get(target_email)
        .or_else(|| subs.get(norm.as_str()))
        .cloned();
    let Some(sub) = sub else { return };
    let body = message.chars().take(120).collect::<String>();
    let state = state.clone();
    let target_email = target_email.to_string();
    let title = title.to_string();
    let payload = serde_json::json!({
        "title": if title.is_empty() { "Admin notification" } else { &title },
        "body": body,
        "url": "/",
    });
    tokio::spawn(async move {
        let _ = send_web_push(&state, &vapid_public, &target_email, &sub, &payload).await;
    });
}

/// Sends one web-push message; returns true when the endpoint reported
/// 410/404 (subscription gone — callers then delete it like the JS does).
pub(crate) async fn send_web_push(
    state: &Arc<AppState>,
    _vapid_public: &str,
    target_email: &str,
    sub: &serde_json::Value,
    payload: &serde_json::Value,
) -> bool {
    let endpoint = sub
        .get("endpoint")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if endpoint.is_empty() {
        return false;
    }
    let keys = sub.get("keys").cloned().unwrap_or(serde_json::json!({}));
    let p256dh = keys
        .get("p256dh")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let auth = keys
        .get("auth")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let vapid_private = std::env::var("VAPID_PRIVATE_KEY")
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    let info = web_push::SubscriptionInfo::new(&endpoint, &p256dh, &auth);
    let Ok(mut sig_builder) = web_push::VapidSignatureBuilder::from_base64(
        &vapid_private,
        web_push::URL_SAFE_NO_PAD,
        &info,
    ) else {
        tracing::warn!("vapid signature builder init failed");
        return false;
    };
    sig_builder.add_claim("sub", "mailto:support@mitch.pro");
    let Ok(sig) = sig_builder.build() else {
        tracing::warn!("vapid signature build failed");
        return false;
    };
    let payload_str = payload.to_string();
    let mut message = web_push::WebPushMessageBuilder::new(&info);
    message.set_vapid_signature(sig);
    message.set_payload(web_push::ContentEncoding::Aes128Gcm, payload_str.as_bytes());
    let Ok(message) = message.build() else {
        return false;
    };
    use web_push::WebPushClient as _;
    let Ok(client) = web_push::IsahcWebPushClient::new() else {
        return false;
    };
    if let Err(e) = client.send(message).await {
        let gone = matches!(
            e,
            web_push::WebPushError::EndpointNotFound
                | web_push::WebPushError::EndpointNotValid
                | web_push::WebPushError::Unauthorized
        ) || match e {
            web_push::WebPushError::Other(ref code) => {
                code.contains("401")
                    || code.contains("403")
                    || code.contains("404")
                    || code.contains("410")
                    || code.contains("Forbidden")
                    || code.contains("Unauthorized")
                    || code.contains("Gone")
                    || code.contains("NotFound")
                    || code.contains("BadDeviceToken")
                    || code.contains("Unregistered")
            }
            _ => false,
        };
        if gone {
            let file = state.cfg.data_dir.join("push_subs.json");
            let mut subs = state.store.read_document(&file, serde_json::json!({}));
            if let Some(map) = subs.as_object_mut() {
                map.remove(target_email);
                map.remove(&mitch_lib::auth::normalize_email(target_email));
            }
            let _ = state.store.write_document(&file, &subs);
        }
        tracing::warn!("push send failed: {e:?} ({e}) endpoint={endpoint}");
        return gone;
    }
    false
}

/// `sendWebPushClean(subs, email, payload)` (server.js:2263-2276). Loads
/// push_subs.json, looks the subscription up by the NORMALIZED email only,
/// and deletes it from the store on 410/404. Awaited by the friends flows
/// exactly as the JS awaits it.
pub async fn send_web_push_clean(state: &Arc<AppState>, email: &str, payload: &serde_json::Value) {
    let vapid_public = std::env::var("VAPID_PUBLIC_KEY")
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    if vapid_public.is_empty() {
        return; // JS parity: `if (!VAPID_PUBLIC) return;`
    }
    let key = mitch_lib::auth::normalize_email(email);
    if key.is_empty() {
        return;
    }
    let file = state.cfg.data_dir.join("push_subs.json");
    let subs = state.store.read_document(&file, serde_json::json!({}));
    let Some(sub) = subs.get(&key).cloned() else {
        return;
    };
    if send_web_push(state, &vapid_public, &key, &sub, payload).await {
        let mut subs = state.store.read_document(&file, serde_json::json!({}));
        if let Some(map) = subs.as_object_mut() {
            map.remove(&key);
        }
        let _ = state.store.write_document(&file, &subs);
    }
}

/// Resolve a topic or secret to an HTTP/HTTPS URL. If it starts with http:// or https://,
/// it is returned as a custom server URL. Otherwise, it is formatted against https://ntfy.sh/{topic}.
pub fn resolve_ntfy_url(secret_or_topic: &str) -> String {
    let trimmed = secret_or_topic.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("https://ntfy.sh/{}", trimmed.trim_start_matches('/'))
    }
}

/// `ntfy(msg, {title, priority})` — POST to ntfy.sh (silent without topic).
pub fn ntfy_notify(msg: &str, title: &str, priority: &str) {
    let topic = std::env::var("NTFY_TOPIC")
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    if topic.is_empty() {
        return;
    }
    let url = resolve_ntfy_url(&topic);
    let ntfy_user = std::env::var("NTFY_USER")
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    let ntfy_pass = std::env::var("NTFY_PASS")
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    let msg = msg.to_string();
    let title = title.to_string();
    let priority = priority.to_string();
    tokio::spawn(async move {
        let mut req = reqwest::Client::new()
            .post(&url)
            .header("Content-Type", "text/plain")
            .timeout(std::time::Duration::from_secs(5));
        if !ntfy_user.is_empty() || !ntfy_pass.is_empty() {
            req = req.basic_auth(&ntfy_user, Some(&ntfy_pass));
        }
        if !title.is_empty() {
            req = req.header("Title", title);
        }
        if !priority.is_empty() && priority != "default" {
            req = req.header("Priority", priority);
        }
        let _ = req.body(msg).send().await;
    });
}

/// `sendEmailBg(to, subject, body)` — the Step 2 mail-service transport.
/// Masked-recipient + profanity guards from server.js:3471-3486.
pub fn send_email_bg(state: &Arc<AppState>, to: &str, subject: &str, body: &str) {
    // Defensive: refuse masked recipients (`ad***n@…` shape).
    static MASK_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = MASK_RE.get_or_init(|| {
        regex::Regex::new(r"^[A-Za-z0-9._%+-]{2}\*\*\*[A-Za-z0-9._%+-]*@").unwrap_or_else(|e| {
            tracing::error!("mask regex failed: {e}");
            regex::Regex::new("$^").unwrap_or_else(|e| {
                tracing::error!("fallback regex failed: {e}");
                regex::Regex::new("$^").unwrap_or_else(|_| {
                    regex::Regex::new("$^").unwrap_or_else(|e2| {
                        tracing::error!("fallback regex failed: {e2}");
                        regex::Regex::new("$^").unwrap_or_else(|_| unreachable!())
                    })
                })
            })
        })
    });
    if re.is_match(to) {
        tracing::warn!("refusing to send — recipient looks masked: {to}");
        return;
    }
    let sender = if to.trim().to_lowercase().ends_with("@student.rjuhsd.us") {
        "gmail"
    } else {
        "noreply"
    };
    let url = mail_service_url(state);
    let body = body.to_string();
    let subject = subject.to_string();
    let to = to.to_string();
    tokio::spawn(async move {
        let payload = serde_json::json!({
            "sender": sender,
            "to": to,
            "subject": subject,
            "body": body,
            "dry_run": false,
        });
        let client = reqwest::Client::new();
        match client
            .post(format!("{url}/send"))
            .json(&payload)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
        {
            Ok(resp) => {
                if !resp.status().is_success() {
                    let status = resp.status();
                    let err = resp.text().await.unwrap_or_default();
                    tracing::warn!("mail service rejected send to {to} ({status}): {err}");
                }
            }
            Err(e) => {
                tracing::warn!("mail service send to {url}/send failed: {e}");
            }
        }
    });
}

fn mail_service_url(_state: &Arc<AppState>) -> String {
    let host = std::env::var("MAIL_RS_HOST")
        .or_else(|_| std::env::var("MAIL_SERVICE_HOST"))
        .unwrap_or_else(|_| {
            if std::env::var("NODE_ENV").unwrap_or_default() == "production" {
                "mail-rs".to_string()
            } else {
                "127.0.0.1".to_string()
            }
        });
    let port = std::env::var("MAIL_RS_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(6902);
    format!("http://{host}:{port}")
}

#[allow(unused)]
pub fn unused_state_guard(_state: &AppState) {}

/// `verifyRecaptcha(token, ip, sid)` (server.js:3598-3689). Fail-open on
/// missing secrets / unreachable Google; 5s total budget; caches per-sid
/// success for 10 minutes.
pub async fn verify_recaptcha(state: &Arc<AppState>, token: &str, ip: &str, sid: &str) -> bool {
    if std::env::var("NODE_ENV").unwrap_or_default() == "test" {
        return true;
    }
    if !ip.is_empty() && (ip == "66.60.183.124" || ip == "127.0.0.1" || ip == "::1") {
        return true;
    }

    if !sid.is_empty() {
        let last = state
            .last_recaptcha_success
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(sid)
            .copied();
        if let Some(last_success) = last {
            if mitch_lib::school::now_millis() - last_success < 10 * 60 * 1000 {
                return true;
            }
        }
    }

    let trim_env = |name: &str| std::env::var(name).unwrap_or_default().trim().to_string();
    let secret_key = trim_env("SECRET_KEY");
    let recaptcha_secret_key = trim_env("RECAPTCHA_SECRET_KEY");
    let mut recaptcha_secrets: Vec<String> = Vec::new();
    if !recaptcha_secret_key.is_empty() {
        recaptcha_secrets.push(recaptcha_secret_key);
    } else if !secret_key.is_empty() {
        recaptcha_secrets.push(secret_key);
    }

    if recaptcha_secrets.is_empty() {
        return true; // JS parity: no secret configured → fail open.
    }
    if token.is_empty() {
        tracing::info!("[recaptcha] Blocked: empty token received (from IP: {ip})");
        return false;
    }

    let verify_urls: Vec<String> = match std::env::var("RECAPTCHA_VERIFY_URL") {
        Ok(v) if !v.trim().is_empty() => vec![v.trim().to_string()],
        _ => vec![
            "https://www.google.com/recaptcha/api/siteverify".to_string(),
            "https://www.recaptcha.net/recaptcha/api/siteverify".to_string(),
        ],
    };

    let min_score_env = trim_env("RECAPTCHA_MIN_SCORE");
    let min_score: f64 = min_score_env.parse().unwrap_or(0.3);
    let threshold = if min_score.is_finite() {
        min_score
    } else {
        0.3
    };

    async fn call_verify(
        verify_urls: &[String],
        token: &str,
        ip: &str,
        secret: &str,
        threshold: f64,
    ) -> bool {
        {
            if secret.is_empty() {
                return false;
            }
            for verify_url in verify_urls {
                let mut params = Vec::new();
                params.push(format!("secret={}", encode_form(secret)));
                params.push(format!("response={}", encode_form(token)));
                if !ip.is_empty() && ip.parse::<std::net::IpAddr>().is_ok() {
                    params.push(format!("remoteip={}", encode_form(ip)));
                }
                // No per-request timeout: the outer 5s budget (JS's single
                // AbortController) cancels the whole attempt on expiry.
                let result = reqwest::Client::new()
                    .post(verify_url)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(params.join("&"))
                    .send()
                    .await;
                let data = match result {
                    Ok(r) => match r.json::<serde_json::Value>().await {
                        Ok(d) => d,
                        Err(_) => return true, // JS: fetch error → fail open.
                    },
                    Err(_) => return true, // JS: fetch error → fail open.
                };
                if data.get("success").and_then(|v| v.as_bool()) != Some(true) {
                    let errors: Vec<String> = data
                        .get("error-codes")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|e| e.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    if errors
                        .iter()
                        .any(|e| e == "invalid-input-response" || e == "bad-request")
                    {
                        continue; // try next verify host
                    }
                    return false;
                }
                if let Some(score) = data.get("score").and_then(|v| v.as_f64()) {
                    if score < threshold {
                        return false;
                    }
                }
                return true;
            }
            false
        }
    }
    let mut verified = false;
    // JS wraps all secret attempts in ONE AbortController with a 5s budget;
    // an abort leaves `verified` false.
    let attempt = async {
        for secret in &recaptcha_secrets {
            if call_verify(&verify_urls, token, ip, secret, threshold).await {
                return true;
            }
        }
        false
    };
    match tokio::time::timeout(std::time::Duration::from_secs(5), attempt).await {
        Ok(v) => verified = v,
        Err(_) => tracing::info!("[recaptcha] Verification timed out after 5s"),
    }

    if verified && !sid.is_empty() {
        state
            .last_recaptcha_success
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(sid.to_string(), mitch_lib::school::now_millis());
    }
    verified
}

/// `application/x-www-form-urlencoded` value encoding (JS URLSearchParams.set
/// → space as `+`, everything else percent-encoded).
fn encode_form(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for b in v.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(*b as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

pub fn valid_push_subscription(value: &serde_json::Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };
    let endpoint = obj.get("endpoint").and_then(|v| v.as_str()).unwrap_or("");
    let keys = obj.get("keys").and_then(|v| v.as_object());
    let p256dh = keys
        .and_then(|k| k.get("p256dh"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let auth = keys
        .and_then(|k| k.get("auth"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !endpoint.starts_with("https://") {
        return false;
    }
    endpoint.len() <= 2048
        && p256dh.len() >= 40
        && p256dh.len() <= 256
        && auth.len() >= 8
        && auth.len() <= 128
}

pub fn handle_push_routes(
    state: &AppState,
    method: &axum::http::Method,
    path: &str,
    headers: &axum::http::HeaderMap,
    body_bytes: &[u8],
) -> Option<axum::response::Response> {
    if path == "/api/push/vapid-key" && *method == axum::http::Method::GET {
        let vapid_public = std::env::var("VAPID_PUBLIC_KEY")
            .map(|v| v.trim().to_string())
            .unwrap_or_default();
        return Some(crate::errors::json_resp(
            200,
            serde_json::json!({ "publicKey": vapid_public }),
        ));
    }

    if path == "/api/push/subscribe" && *method == axum::http::Method::POST {
        let cookies = crate::routes::me::cookies_of(state, headers);
        let sid = cookies
            .get("studentId")
            .filter(|s| !s.is_empty())
            .or_else(|| cookies.get("id"))
            .unwrap_or("");
        if sid.is_empty()
            || !mitch_lib::auth::valid_id(sid, &state.id_secret)
            || state.is_revoked_id(sid)
        {
            return Some(crate::errors::json_resp(
                401,
                serde_json::json!({ "error": "auth required" }),
            ));
        }
        let email = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, sid)
            .unwrap_or_default();
        if email.is_empty() {
            return Some(crate::errors::json_resp(
                403,
                serde_json::json!({ "error": "email not found" }),
            ));
        }
        let Ok(body) = serde_json::from_slice::<serde_json::Value>(body_bytes) else {
            return Some(crate::errors::json_resp(
                400,
                serde_json::json!({ "error": "bad json" }),
            ));
        };
        if !valid_push_subscription(&body) {
            return Some(crate::errors::json_resp(
                400,
                serde_json::json!({ "error": "invalid push subscription" }),
            ));
        }
        let norm = mitch_lib::auth::normalize_email(&email);
        let subs_file = state.data_dir().join("push_subs.json");
        let mut subs = state.store.read_document(&subs_file, serde_json::json!({}));
        if let Some(map) = subs.as_object_mut() {
            map.insert(norm, body);
            let _ = state.store.write_document(&subs_file, &subs);
        }
        return Some(crate::errors::json_resp(
            200,
            serde_json::json!({ "success": true }),
        ));
    }

    if path == "/api/push/unsubscribe" && *method == axum::http::Method::POST {
        let cookies = crate::routes::me::cookies_of(state, headers);
        let sid = cookies
            .get("studentId")
            .filter(|s| !s.is_empty())
            .or_else(|| cookies.get("id"))
            .unwrap_or("");
        if sid.is_empty()
            || !mitch_lib::auth::valid_id(sid, &state.id_secret)
            || state.is_revoked_id(sid)
        {
            return Some(crate::errors::json_resp(
                401,
                serde_json::json!({ "error": "auth required" }),
            ));
        }
        let email = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, sid)
            .unwrap_or_default();
        if email.is_empty() {
            return Some(crate::errors::json_resp(
                403,
                serde_json::json!({ "error": "email not found" }),
            ));
        }
        let norm = mitch_lib::auth::normalize_email(&email);
        let subs_file = state.data_dir().join("push_subs.json");
        let mut subs = state.store.read_document(&subs_file, serde_json::json!({}));
        if let Some(map) = subs.as_object_mut() {
            map.remove(&norm);
            let _ = state.store.write_document(&subs_file, &subs);
        }
        return Some(crate::errors::json_resp(
            200,
            serde_json::json!({ "success": true }),
        ));
    }

    if path == "/api/ntfy/topic" && *method == axum::http::Method::GET {
        let cookies = crate::routes::me::cookies_of(state, headers);
        let sid = cookies
            .get("studentId")
            .filter(|s| !s.is_empty())
            .or_else(|| cookies.get("id"))
            .unwrap_or("");
        if sid.is_empty()
            || !mitch_lib::auth::valid_id(sid, &state.id_secret)
            || state.is_revoked_id(sid)
        {
            return Some(crate::errors::json_resp(
                401,
                serde_json::json!({ "error": "auth required" }),
            ));
        }
        let email = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, sid)
            .unwrap_or_default();
        if email.is_empty() {
            return Some(crate::errors::json_resp(
                403,
                serde_json::json!({ "error": "email not found" }),
            ));
        }
        let norm = mitch_lib::auth::normalize_email(&email);
        let ntfy_file = state.data_dir().join("ntfy_topics.json");
        let topics = state.store.read_document(&ntfy_file, serde_json::json!({}));
        let topic_val = topics.get(&norm).and_then(|v| v.as_str());
        return Some(crate::errors::json_resp(
            200,
            serde_json::json!({ "topic": topic_val }),
        ));
    }

    if path == "/api/ntfy/topic" && *method == axum::http::Method::POST {
        let cookies = crate::routes::me::cookies_of(state, headers);
        let sid = cookies
            .get("studentId")
            .filter(|s| !s.is_empty())
            .or_else(|| cookies.get("id"))
            .unwrap_or("");
        if sid.is_empty()
            || !mitch_lib::auth::valid_id(sid, &state.id_secret)
            || state.is_revoked_id(sid)
        {
            return Some(crate::errors::json_resp(
                401,
                serde_json::json!({ "error": "auth required" }),
            ));
        }
        let email = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, sid)
            .unwrap_or_default();
        if email.is_empty() {
            return Some(crate::errors::json_resp(
                403,
                serde_json::json!({ "error": "email not found" }),
            ));
        }
        let Ok(body) = serde_json::from_slice::<serde_json::Value>(body_bytes) else {
            return Some(crate::errors::json_resp(
                400,
                serde_json::json!({ "error": "bad json" }),
            ));
        };
        let topic = body.get("topic").and_then(|v| v.as_str()).unwrap_or("").trim();
        if !topic.is_empty() {
            let is_url = topic.starts_with("http://") || topic.starts_with("https://");
            if is_url {
                if topic.len() > 1024 {
                    return Some(crate::errors::json_resp(
                        400,
                        serde_json::json!({ "error": "url is too long" }),
                    ));
                }
            } else {
                let topic_ok = (6..=64).contains(&topic.chars().count())
                    && topic
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
                if !topic_ok {
                    return Some(crate::errors::json_resp(
                        400,
                        serde_json::json!({ "error": "topic must be 6-64 letters, numbers, dashes or underscores, or a full url" }),
                    ));
                }
            }
        }
        let norm = mitch_lib::auth::normalize_email(&email);
        let ntfy_file = state.data_dir().join("ntfy_topics.json");
        let mut topics = state.store.read_document(&ntfy_file, serde_json::json!({}));
        if let Some(map) = topics.as_object_mut() {
            if topic.is_empty() {
                map.remove(&norm);
            } else {
                map.insert(norm, serde_json::json!(topic));
            }
            let _ = state.store.write_document(&ntfy_file, &topics);
        }
        return Some(crate::errors::json_resp(
            200,
            serde_json::json!({ "success": true, "topic": if topic.is_empty() { serde_json::Value::Null } else { serde_json::json!(topic) } }),
        ));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_ntfy_url() {
        // Bare topic
        assert_eq!(resolve_ntfy_url("my-alerts"), "https://ntfy.sh/my-alerts");
        // Bare topic with leading slash
        assert_eq!(resolve_ntfy_url("/my-alerts"), "https://ntfy.sh/my-alerts");
        // Custom HTTPS URL
        assert_eq!(
            resolve_ntfy_url("https://custom-ntfy.example.com/topic123"),
            "https://custom-ntfy.example.com/topic123"
        );
        // Custom HTTP URL
        assert_eq!(
            resolve_ntfy_url("http://192.168.1.50:8080/alerts"),
            "http://192.168.1.50:8080/alerts"
        );
    }
}
