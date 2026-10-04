//! `/livekit/*` — the Matrix VoIP / LiveKit SFU surface (plan Step 13 batch 2).
//!
//! Ports (server.js):
//! - the OPTIONS preflight block (9637-9647) and the token endpoint
//!   `/livekit/sfu/get` + `/livekit/get_token` (9649-9698). The JS runs these
//!   BEFORE the maintenance/ban/password gates (9636 < 10500), so handler.rs
//!   dispatches them from the pre-gate slot — that placement IS the parity:
//!   anonymous token generation succeeds.
//! - the `/livekit/rtc` signaling WebSocket proxy (11725-11735 upgrade gate,
//!   24933-24961 open, 25041-25051 message) — runs AFTER the password gate,
//!   like the JS upgrade block at 11725.

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::Response;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use mitch_lib::jsval;
use serde_json::{json, Map, Value};
use std::sync::Arc;

use crate::state::AppState;

// ── token generation (server.js:7866-7892) ───────────────────────────────────

fn api_key() -> String {
    std::env::var("LIVEKIT_API_KEY").unwrap_or_else(|_| "mitchlivekit".to_string())
}

/// `LIVEKIT_API_SECRET` (server.js:7867) — env override, else
/// `createHmac('sha256', ID_SECRET).update('livekit-secret').digest('hex')`.
/// The default is the hex STRING; both are used verbatim as the HMAC key.
fn api_secret_bytes(_id_secret: &[u8]) -> Vec<u8> {
    let raw = std::env::var("LIVEKIT_API_SECRET").unwrap_or_default();
    if !raw.is_empty() {
        return raw.into_bytes();
    }
    // Matches server.js:8066 and docker-compose.yml LiveKit server keys:
    // process.env.LIVEKIT_API_SECRET || 'mitch-secret-livekit-matrix-key-2026'
    b"mitch-secret-livekit-matrix-key-2026".to_vec()
}

/// `generateLiveKitToken({identity, name, roomName})` (server.js:7889-7912).
/// The `||` fallbacks (`identity || 'anonymous'`, `name || identity ||
/// 'Anonymous'`, `roomName || 'default'`) are applied here exactly as in the
/// JS; callers pass the raw JS values. `name`/`room` stay `Value` because a
/// non-string truthy body value (e.g. a number) reaches the JWT payload
/// unchanged — JSON.stringify's semantics.
pub fn generate_livekit_token(
    id_secret: &[u8],
    identity: &Value,
    name: &Value,
    room: &Value,
) -> String {
    let header =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let now = mitch_lib::school::now_millis() / 1000;
    let sub = jsval::or(Some(identity), json!("anonymous"));
    let name = jsval::or(Some(name), jsval::or(Some(identity), json!("Anonymous")));
    let room = jsval::or(Some(room), json!("default"));
    let payload = json!({
        "iss": api_key(),
        "sub": sub,
        "name": name,
        "iat": now,
        "exp": now + 86400,
        "nbf": now - 10,
        "video": {
            "room": room,
            "roomJoin": true,
            "canPublish": true,
            "canSubscribe": true,
            "canPublishData": true,
        },
    });
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        serde_json::to_string(&payload)
            .unwrap_or_default()
            .as_bytes(),
    );
    let sig =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mitch_lib::crypto::hmac_sha256(
            &api_secret_bytes(id_secret),
            format!("{header}.{payload}").as_bytes(),
        ));
    format!("{header}.{payload}.{sig}")
}

// ── pre-gate HTTP dispatch (server.js:9637-9698) ─────────────────────────────

fn cors_headers() -> Vec<(&'static str, HeaderValue)> {
    vec![
        ("access-control-allow-origin", HeaderValue::from_static("*")),
        (
            "access-control-allow-methods",
            HeaderValue::from_static("GET, POST, OPTIONS"),
        ),
        (
            "access-control-allow-headers",
            HeaderValue::from_static(
                "Origin, X-Requested-With, Content-Type, Accept, Authorization",
            ),
        ),
    ]
}

fn json_with_cors(code: u16, obj: Value, max_age: bool) -> Response {
    let mut resp = crate::routes::me::json_response(code, obj);
    for (k, v) in cors_headers() {
        resp.headers_mut().insert(k, v);
    }
    if max_age {
        resp.headers_mut()
            .insert("access-control-max-age", HeaderValue::from_static("86400"));
    }
    resp
}

/// The pre-gate dispatch. `body_bytes` is the raw request body (the JS reads
/// `req.text()` only on POST). Returns `None` for paths this module does not
/// own so the caller falls through to the rest of the pipeline.
pub fn handle_http(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    search: &str,
    body_bytes: &[u8],
) -> Option<Response> {
    if !path.starts_with("/livekit") {
        return None;
    }
    if method == Method::OPTIONS {
        return Some(json_with_cors(
            StatusCode::NO_CONTENT.as_u16(),
            Value::Null,
            true,
        ));
    }
    if (path == "/livekit/sfu/get"
        || path == "/livekit/get_token"
        || path == "/livekit/token"
        || path == "/livekit/jwt")
        && (method == Method::POST || method == Method::GET)
    {
        return Some(token_endpoint(state, method, headers, search, body_bytes));
    }
    None
}

/// Called only when `handle_http` returned `None` for a `/livekit/*` path
/// and the request is not a WebSocket upgrade (that case is handled by
/// `handle_rtc_upgrade` instead) — see `handle_http_passthrough`.
pub async fn handle_http_fallback(
    method: &Method,
    path: &str,
    search: &str,
    body_bytes: &[u8],
) -> Option<Response> {
    if !path.starts_with("/livekit") {
        return None;
    }
    Some(handle_http_passthrough(method, path, search, body_bytes).await)
}

fn token_endpoint(
    state: &Arc<AppState>,
    method: &Method,
    headers: &HeaderMap,
    search: &str,
    body_bytes: &[u8],
) -> Response {
    // JS: POST parses the body (catch → {}); GET walks the search params.
    let mut body = Map::new();
    if method == Method::POST {
        if let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(body_bytes) {
            body = map;
        }
    } else {
        for (k, v) in form_urlencoded::parse(search.as_bytes()) {
            body.insert(k.into_owned(), json!(v.into_owned()));
        }
    }

    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cookies.get("id").unwrap_or(""))
        .to_string();
    let cookie_email = if sid.is_empty() {
        String::new()
    } else {
        mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid).unwrap_or_default()
    };
    let norm_email = if cookie_email.is_empty() {
        String::new()
    } else {
        mitch_lib::auth::normalize_email(&cookie_email)
    };
    let profiles = state
        .store
        .read_document(&state.data_dir().join("profiles.json"), json!({}));
    let prof = if norm_email.is_empty() {
        json!({})
    } else {
        profiles.get(&norm_email).cloned().unwrap_or(json!({}))
    };

    // JS: `body.room || body.room_id || body.roomId || 'default'`.
    let room = jsval::or(
        body.get("room"),
        jsval::or(
            body.get("room_id"),
            jsval::or(body.get("roomId"), json!("default")),
        ),
    );

    // JS: `body.member?.claimed_user_id || body.claimed_user_id || body.user_id || body.userId || body.openid_token?.user_id || ''`.
    let member_claimed = body
        .get("member")
        .filter(|v| jsval::truthy(v))
        .and_then(|m| m.get("claimed_user_id"))
        .filter(|v| jsval::truthy(v))
        .cloned();
    let openid_claimed = body
        .get("openid_token")
        .filter(|v| jsval::truthy(v))
        .and_then(|o| o.get("user_id"))
        .filter(|v| jsval::truthy(v))
        .cloned();
    let raw_user_id_val = jsval::or(
        member_claimed.as_ref(),
        jsval::or(
            body.get("claimed_user_id"),
            jsval::or(
                body.get("user_id"),
                jsval::or(
                    body.get("userId"),
                    jsval::or(openid_claimed.as_ref(), json!("")),
                ),
            ),
        ),
    );
    // JS then builds the string; a non-string truthy body value would throw
    // in the JS (`.startsWith` on a number) — we stringify instead.
    let mut raw_user_id = jsval::string(&raw_user_id_val);
    if raw_user_id.is_empty() && !norm_email.is_empty() {
        let username = jsval::string(&jsval::or(
            prof.get("username").filter(|v| jsval::truthy(v)),
            json!(mitch_lib::profile::default_username_for_email(&norm_email)),
        ));
        raw_user_id = format!("@{username}:mitch.pro");
    }
    if raw_user_id.is_empty() {
        // Math.random().toString(36).slice(2, 8) — the first 6 base-36
        // fractional digits (the product with 36^6 is exactly that).
        let r = crate::routes::me::security::js_rand();
        let mut n = (r * 2176782336.0) as u64; // 36^6
        let mut s = [0u8; 6];
        for i in (0..6).rev() {
            s[i] = char::from_digit((n % 36) as u32, 36).unwrap_or('0') as u8;
            n /= 36;
        }
        raw_user_id = format!("@user_{}:mitch.pro", String::from_utf8_lossy(&s));
    }

    // JS: startsWith('@') ? raw : `@${raw.replace(/[^a-zA-Z0-9._=-]/g,'')}:mitch.pro`.
    let identity = if raw_user_id.starts_with('@') {
        raw_user_id
    } else {
        let cleaned: String = raw_user_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || "._=-".contains(*c))
            .collect();
        format!("@{cleaned}:mitch.pro")
    };
    let identity_val = json!(identity);

    // JS: `body.name || body.member?.display_name || prof.displayName || identity.split(':')[0].replace(/^@/,'')`.
    let member_display_name = body
        .get("member")
        .filter(|v| jsval::truthy(v))
        .and_then(|m| m.get("display_name"))
        .filter(|v| jsval::truthy(v))
        .cloned();
    let identity_default_name = json!(identity
        .split(':')
        .next()
        .unwrap_or("")
        .trim_start_matches('@'));
    let display_name = jsval::or(
        body.get("name"),
        jsval::or(
            member_display_name.as_ref(),
            jsval::or(prof.get("displayName"), identity_default_name),
        ),
    );

    let jwt = generate_livekit_token(&state.id_secret, &identity_val, &display_name, &room);
    let host = crate::hosts::request_host(headers);
    let host = if host.is_empty() {
        "mitch.pro".to_string()
    } else {
        host
    };
    // JS: case-sensitive startsWith (no lowercasing).
    let is_wss = !host.starts_with("localhost") && !host.starts_with("127.0.0.1");
    let ws_url = format!(
        "{}{host}/livekit/rtc",
        if is_wss { "wss://" } else { "ws://" }
    );
    json_with_cors(
        200,
        json!({
            "url": ws_url,
            "jwt": jwt,
            "token": jwt,
            "access_token": jwt
        }),
        false,
    )
}

// ── /livekit/rtc WS proxy (server.js:11725-11735, 24933-24961, 25041-25051) ──

/// The post-gate upgrade arm. Returns `None` when the request is not a
/// `/livekit/rtc*` websocket so the caller falls through (the JS only
/// intercepts on an `upgrade: websocket` header; a plain GET reaches the
/// static/404 handling below it).
pub fn handle_rtc_upgrade(
    state: &Arc<AppState>,
    path: &str,
    search: &str,
    upgrade: Option<WebSocketUpgrade>,
) -> Option<Response> {
    if !path.starts_with("/livekit/rtc") {
        return None;
    }
    let Some(on_upgrade) = upgrade else {
        // The JS only 400s when `server.upgrade` fails on a websocket request;
        // a missing/bad handshake reaches us as `None` here.
        return Some(crate::routes::me::json_response(
            400,
            json!({ "error": "websocket upgrade failed" }),
        ));
    };
    let _ = state;
    let search = search.to_string();
    Some(on_upgrade.on_upgrade(move |socket| async move {
        run_rtc_proxy(socket, search).await;
    }))
}

/// server.js:24934 — env host, else the docker detection.
fn livekit_upstream_addr() -> (String, String) {
    let livekit_host = std::env::var("LIVEKIT_HOST").unwrap_or_else(|_| {
        if std::env::var("DOCKER_ENV").as_deref() == Ok("1")
            || std::path::Path::new("/.dockerenv").exists()
        {
            "livekit".to_string()
        } else {
            "127.0.0.1".to_string()
        }
    });
    let livekit_port = std::env::var("LIVEKIT_PORT").unwrap_or_else(|_| "7880".to_string());
    (livekit_host, livekit_port)
}

/// Plain-HTTP passthrough for any `/livekit/*` request that isn't a token
/// endpoint or a WebSocket upgrade — e.g. the LiveKit client SDK's
/// `/rtc/.../validate` pre-flight reachability check. Without this, those
/// requests fell through to the generic 404 handler, which adds no CORS
/// headers — harmless same-origin, but it silently breaks every call made
/// from a different mitch.pro-family domain (mitchdog.com, rjuhsd.school,
/// sexypickleclub.com) than the one LiveKit itself is reachable on, since
/// the browser blocks the cross-origin fetch outright before it ever
/// reaches this server.
pub async fn handle_http_passthrough(
    method: &Method,
    path: &str,
    search: &str,
    body_bytes: &[u8],
) -> Response {
    let (livekit_host, livekit_port) = livekit_upstream_addr();
    let Some(suffix) = path.strip_prefix("/livekit") else {
        return json_with_cors(StatusCode::NOT_FOUND.as_u16(), json!({ "error": "not found" }), false);
    };
    let query_part = if search.is_empty() {
        String::new()
    } else if search.starts_with('?') {
        search.to_string()
    } else {
        format!("?{search}")
    };
    let upstream_url = format!("http://{livekit_host}:{livekit_port}{suffix}{query_part}");

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(_) => {
            return json_with_cors(
                StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                json!({ "error": "client build failed" }),
                false,
            )
        }
    };
    let reqwest_method =
        reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);
    let mut req = client.request(reqwest_method, &upstream_url);
    if !body_bytes.is_empty() {
        req = req.body(body_bytes.to_vec());
    }
    let upstream_resp = match req.send().await {
        Ok(r) => r,
        Err(_) => {
            return json_with_cors(
                StatusCode::BAD_GATEWAY.as_u16(),
                json!({ "error": "LiveKit upstream unreachable" }),
                false,
            )
        }
    };
    let status = upstream_resp.status().as_u16();
    let content_type = upstream_resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let bytes = upstream_resp.bytes().await.unwrap_or_default();
    let mut builder = Response::builder().status(status);
    if let Some(headers) = builder.headers_mut() {
        if let Ok(ct) = HeaderValue::from_str(&content_type) {
            headers.insert(axum::http::header::CONTENT_TYPE, ct);
        }
        for (k, v) in cors_headers() {
            headers.insert(k, v);
        }
    }
    builder
        .body(axum::body::Body::from(bytes))
        .unwrap_or_else(|_| {
            json_with_cors(
                StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                json!({ "error": "response build failed" }),
                false,
            )
        })
}

async fn run_rtc_proxy(client: WebSocket, search: String) {
    let (livekit_host, livekit_port) = livekit_upstream_addr();
    let query_part = if search.is_empty() {
        String::new()
    } else if search.starts_with('?') {
        search.clone()
    } else {
        format!("?{search}")
    };
    let upstream_url = format!("ws://{livekit_host}:{livekit_port}/rtc{query_part}");

    let (mut client_sink, mut client_stream) = client.split();
    // In the JS the `new WebSocket` connect is async: a failure fires
    // upstream.onerror → close(1011, 'LiveKit upstream connection failed')
    // (the sync-throw catch is dead for valid URLs in Bun). connect_async
    // here is the same window.
    let Ok((upstream, _)) = tokio_tungstenite::connect_async(&upstream_url).await else {
        tracing::warn!("[livekit-proxy] Failed to connect upstream");
        let _ = client_sink
            .send(Message::Close(Some(CloseFrame {
                code: 1011,
                reason: "LiveKit upstream connection failed".into(),
            })))
            .await;
        return;
    };
    let (mut upstream_sink, mut upstream_stream) = upstream.split();
    // Messages the client sent while the connect was in flight were already
    // buffered by the socket, so the JS CONNECTING/pending branch has no
    // separate state here; the ≤50 cap only mattered for that window.

    // The JS runs two independent event pumps; the select below is the same
    // shape, biased to the upstream leg so echoes never starve behind a
    // busy client.
    loop {
        tokio::select! {
            biased;
            frame = upstream_stream.next() => match frame {
                // JS upstream.onmessage → ws.send(e.data) — raw forward.
                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t))) => {
                    if client_sink.send(Message::Text(t.to_string().into())).await.is_err() {
                        break;
                    }
                }
                Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(b))) => {
                    if client_sink.send(Message::Binary(b.to_vec().into())).await.is_err() {
                        break;
                    }
                }
                Some(Ok(tokio_tungstenite::tungstenite::Message::Close(c))) => {
                    // JS upstream.onclose → ws.close(ev?.code || 1000, reason).
                    let out = match c {
                        Some(cf) => Message::Close(Some(CloseFrame {
                            code: cf.code.into(),
                            reason: cf.reason.to_string().into(),
                        })),
                        // ev.code undefined → `|| 1000`.
                        None => Message::Close(Some(CloseFrame {
                            code: 1000,
                            reason: "".into(),
                        })),
                    };
                    let _ = client_sink.send(out).await;
                    break;
                }
                // Ping/Pong are answered by tungstenite on the upstream leg
                // and by axum on the client leg — the JS never sees them
                // either (Bun handles protocol frames internally).
                Some(Ok(_)) => {}
                // JS upstream.onerror → close(1011, 'LiveKit upstream
                // connection failed'); a clean upstream end (None) has no
                // onclose event with data in the JS and surfaces the same
                // way through the socket teardown.
                Some(Err(_)) => {
                    tracing::warn!("[livekit-proxy] Upstream error");
                    let _ = client_sink
                        .send(Message::Close(Some(CloseFrame {
                            code: 1011,
                            reason: "LiveKit upstream connection failed".into(),
                        })))
                        .await;
                    break;
                }
                None => {
                    let _ = client_sink.send(Message::Close(None)).await;
                    break;
                }
            },
            frame = client_stream.next() => {
                match frame {
                    // JS client message → upstream.send if OPEN; while the
                    // JS had a CONNECTING window for pending≤50, our connect
                    // has already resolved — just forward. A closed upstream
                    // already left this loop via the upstream leg.
                    Some(Ok(Message::Text(t))) => {
                        if upstream_sink
                            .send(tokio_tungstenite::tungstenite::Message::Text(
                                t.to_string().into(),
                            ))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Some(Ok(Message::Binary(b))) => {
                        if upstream_sink
                            .send(tokio_tungstenite::tungstenite::Message::Binary(b.to_vec().into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    // JS ws.onclose → upstream.close().
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => {
                        let _ = upstream_sink.close().await;
                        break;
                    }
                    Some(Ok(_)) => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"test-id-secret";

    // api_key()/api_secret_bytes() read process-global env; the env-swapping
    // test and the default-claim tests must not overlap (a parallel flake
    // otherwise).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn decode_part(part: &str) -> String {
        use base64::Engine;
        String::from_utf8(
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(part)
                .unwrap_or_default(),
        )
        .unwrap_or_default()
    }

    #[test]
    fn token_shape_and_claims() {
        let _g = env_lock();
        let jwt = generate_livekit_token(
            SECRET,
            &json!("@me:mitch.pro"),
            &json!("Mitch"),
            &json!("room-1"),
        );
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(decode_part(parts[0]), r#"{"alg":"HS256","typ":"JWT"}"#);
        let payload: Value = serde_json::from_str(&decode_part(parts[1])).unwrap_or_default();
        assert_eq!(payload["iss"], "mitchlivekit");
        assert_eq!(payload["sub"], "@me:mitch.pro");
        assert_eq!(payload["name"], "Mitch");
        assert_eq!(payload["video"]["room"], "room-1");
        assert_eq!(payload["video"]["roomJoin"], true);
        assert_eq!(payload["video"]["canPublishData"], true);
        assert_eq!(payload["exp"], payload["iat"].as_i64().unwrap_or(0) + 86400);
        assert_eq!(payload["nbf"], payload["iat"].as_i64().unwrap_or(0) - 10);
        // Sig verifies against the default secret:
        // 'mitch-secret-livekit-matrix-key-2026'.
        let secret = b"mitch-secret-livekit-matrix-key-2026";
        let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            mitch_lib::crypto::hmac_sha256(
                secret,
                format!("{}.{}", parts[0], parts[1]).as_bytes(),
            ),
        );
        assert_eq!(parts[2], expected);
    }

    #[test]
    fn token_fallbacks() {
        let jwt = generate_livekit_token(SECRET, &json!("@x:mitch.pro"), &json!(""), &json!(""));
        let parts: Vec<&str> = jwt.split('.').collect();
        let payload: Value = serde_json::from_str(&decode_part(parts[1])).unwrap_or_default();
        assert_eq!(payload["name"], "@x:mitch.pro");
        assert_eq!(payload["video"]["room"], "default");
    }

    #[test]
    fn env_overrides() {
        // Temporarily swap env; the two token gens must differ in iss/secret.
        let _g = env_lock();
        std::env::set_var("LIVEKIT_API_KEY", "envkey");
        std::env::set_var("LIVEKIT_API_SECRET", "envsecret");
        let a = generate_livekit_token(SECRET, &json!("@a"), &json!(""), &json!(""));
        std::env::remove_var("LIVEKIT_API_KEY");
        std::env::remove_var("LIVEKIT_API_SECRET");
        let b = generate_livekit_token(SECRET, &json!("@a"), &json!(""), &json!(""));
        assert_ne!(a, b);
        assert!(a.starts_with("eyJ")); // {"a"… base64url
    }
}
