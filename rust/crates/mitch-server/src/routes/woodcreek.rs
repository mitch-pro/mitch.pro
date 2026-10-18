//! Authenticated HTTP/WebSocket bridge to the shared Woodcreek Blooket service.
#![allow(clippy::expect_used)]

use crate::state::AppState;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

const MOUNT: &str = "/blooket-bot/";
const CONTROL_LIMIT: &str = "/api/blooket-fleet/control";
const MAX_COMMAND: usize = 262_144;
const MAX_RESPONSE: usize = 12 * 1024 * 1024;

pub fn is_path(path: &str) -> bool {
    path.starts_with(MOUNT)
}

fn error(status: u16, message: &str) -> Response {
    crate::routes::me::json_response(status, json!({"error":message}))
}

fn backend() -> Option<url::Url> {
    let raw = std::env::var("WOODCREEK_BACKEND_URL")
        .unwrap_or_else(|_| "https://woodcreek.site/".to_string());
    let url = url::Url::parse(&raw).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none())
    .then_some(url)
}

fn allowed_path(relative: &str, method: &Method) -> bool {
    if relative.contains('\\')
        || relative.contains('%')
        || relative.split('/').any(|part| matches!(part, "." | ".."))
    {
        return false;
    }
    let read = method == Method::GET || method == Method::HEAD;
    match relative {
        ""
        | "index.html"
        | "components.css"
        | "woodcreek.css"
        | "woodcreek.js"
        | "bookmarklets"
        | "bookmarklets.html"
        | "api/state"
        | "api/preview-names"
        | "api/bot/screenshot"
        | "api/cheats/catalog"
        | "api/flaresolverr/status" => read,
        "api/fleet/launch"
        | "api/start"
        | "api/launch"
        | "api/fleet/stop"
        | "api/stop-all"
        | "api/stop"
        | "api/bot/retry"
        | "api/bot/retry-all"
        | "api/fleet/force-answer"
        | "api/fleet/shuffle-blooks"
        | "api/fleet/trim-to-one"
        | "api/fleet/inject"
        | "api/fleet/settings"
        | "api/flaresolverr/refresh" => method == Method::POST,
        _ => {
            read && (relative.starts_with("assets/") || relative.starts_with("cheats/"))
                && relative
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"/_-.".contains(&c))
        }
    }
}

fn mounted_html(html: &str) -> String {
    let html = html
        .replacen("<html", "<html data-app-base=\"/blooket-bot\"", 1)
        .replace("src=\"/", "src=\"/blooket-bot/")
        .replace("href=\"/", "href=\"/blooket-bot/");
    crate::inject::inject_presence(&html, Some(MOUNT))
}

/// Every route has the same strict session check, including assets and upgrades.
#[allow(clippy::too_many_arguments)]
pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    search: &str,
    headers: &HeaderMap,
    body: &[u8],
    upgrade: Option<WebSocketUpgrade>,
) -> Response {
    let Some(email) = crate::presence::authenticated_email(state, headers) else {
        if path == MOUNT && method == Method::GET {
            return crate::static_files::redirect("/enroll/?next=%2Fblooket-bot%2F", 302);
        }
        return error(401, "Sign in to use Blooket tools.");
    };
    let Some(relative) = path.strip_prefix(MOUNT) else {
        return error(404, "Not found.");
    };
    if relative == "socket" {
        if !crate::hosts::same_origin_request(headers, Some(&state.cfg)) {
            return error(403, "Connection origin is not allowed.");
        }
        let Some(upgrade) = upgrade else {
            return error(400, "WebSocket connection required.");
        };
        let Some(mut url) = backend().and_then(|base| base.join("socket").ok()) else {
            return error(503, "Blooket tools are temporarily unavailable.");
        };
        let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
        let _ = url.set_scheme(scheme);
        let st = Arc::clone(state);
        let headers = headers.clone();
        return upgrade
            .max_message_size(MAX_COMMAND)
            .on_upgrade(move |client| relay(st, client, url.to_string(), email, headers));
    }
    if !allowed_path(relative, method) {
        return error(404, "Not found.");
    }
    if *method == Method::POST {
        if headers
            .get("x-mitch-requested-with")
            .and_then(|value| value.to_str().ok())
            != Some("1")
            || !crate::hosts::same_origin_request(headers, Some(&state.cfg))
        {
            return error(403, "csrf_blocked");
        }
        if body.len() > MAX_COMMAND {
            return error(413, "Request is too large.");
        }
        if serde_json::from_slice::<Value>(body).is_err() {
            return error(400, "Invalid request.");
        }
        if state.rate_limiter.rate_limited(&email, CONTROL_LIMIT) {
            return error(429, "Please wait before sending another command.");
        }
    }
    let Some(mut url) = backend().and_then(|base| base.join(relative).ok()) else {
        return error(503, "Blooket tools are temporarily unavailable.");
    };
    if search.len() > 4096 {
        return error(414, "Request URL is too long.");
    }
    url.set_query((!search.is_empty()).then_some(search.trim_start_matches('?')));
    static CLIENT: OnceLock<Result<reqwest::Client, reqwest::Error>> = OnceLock::new();
    let Ok(client) = CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(25))
            .redirect(reqwest::redirect::Policy::none())
            .build()
    }) else {
        return error(503, "Blooket tools are temporarily unavailable.");
    };
    // Mitch cookies, authorization, and forwarding headers stay on Mitch.
    let mut request = client.request(method.clone(), url);
    for name in ["accept", "content-type"] {
        if let Some(value) = headers.get(name) {
            request = request.header(name, value);
        }
    }
    if *method == Method::POST {
        request = request.body(body.to_vec());
    }
    let Ok(mut upstream) = request.send().await else {
        return error(502, "Blooket tools could not connect. Please try again.");
    };
    let status = upstream.status();
    if status.is_redirection() {
        return error(502, "Blooket tools could not connect. Please try again.");
    }
    let content_type = upstream
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let mut bytes = Vec::new();
    if *method != Method::HEAD {
        loop {
            match upstream.chunk().await {
                Ok(Some(chunk)) if bytes.len() + chunk.len() <= MAX_RESPONSE => {
                    bytes.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                _ => return error(502, "Blooket tools could not load this response."),
            }
        }
    }
    let bytes = if *method != Method::HEAD && content_type.contains("text/html") {
        mounted_html(&String::from_utf8_lossy(&bytes)).into_bytes()
    } else {
        bytes
    };
    Response::builder()
        .status(status)
        .header("content-type", content_type)
        .header("cache-control", "private, no-store")
        .header("vary", "Cookie")
        .header("x-content-type-options", "nosniff")
        .body(axum::body::Body::from(bytes))
        .expect("static bridge response")
}

async fn relay(
    state: Arc<AppState>,
    mut client: WebSocket,
    url: String,
    email: String,
    headers: HeaderMap,
) {
    use tokio_tungstenite::tungstenite::Message as UpstreamMessage;
    let connected = tokio::time::timeout(
        Duration::from_secs(20),
        tokio_tungstenite::connect_async(url),
    )
    .await;
    let Ok(Ok((mut upstream, _))) = connected else {
        let _ = client
            .send(Message::Close(Some(CloseFrame {
                code: 1011,
                reason: "Blooket service unavailable".into(),
            })))
            .await;
        return;
    };
    let mut check = tokio::time::interval(Duration::from_secs(15));
    loop {
        tokio::select! {
            _ = check.tick() => {
                if crate::presence::authenticated_email(&state, &headers).as_deref() != Some(&email) {
                    let _ = client.send(Message::Close(Some(CloseFrame {code:1008,reason:"Session expired".into()}))).await;
                    break;
                }
                if client.send(Message::Ping(Vec::new().into())).await.is_err() { break; }
            }
            frame = client.recv() => match frame {
                Some(Ok(Message::Text(text))) => {
                    if text.len() > MAX_COMMAND || serde_json::from_str::<Value>(&text).is_err()
                        || state.rate_limiter.rate_limited(&email, CONTROL_LIMIT) { continue; }
                    if upstream.send(UpstreamMessage::Text(text.to_string().into())).await.is_err() { break; }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                _ => {}
            },
            frame = upstream.next() => match frame {
                Some(Ok(UpstreamMessage::Text(text))) => {
                    if client.send(Message::Text(text.to_string().into())).await.is_err() { break; }
                }
                Some(Ok(UpstreamMessage::Close(_))) | Some(Err(_)) | None => break,
                _ => {}
            }
        }
    }
    let _ = upstream.close(None).await;
    let _ = client.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_paths_are_fixed_and_methods_are_restricted() {
        assert!(allowed_path("", &Method::GET));
        assert!(allowed_path("api/state", &Method::GET));
        assert!(allowed_path("api/fleet/launch", &Method::POST));
        assert!(!allowed_path("api/fleet/launch", &Method::GET));
        assert!(!allowed_path("api/state", &Method::POST));
        assert!(allowed_path(
            "cheats/global/autoAnswer.min.js",
            &Method::GET
        ));
        for path in [
            "https://other.site/",
            "../api/state",
            "assets/../../secret",
            "assets/%2e%2e/secret",
            "assets/\\secret",
            "server.mjs",
        ] {
            assert!(!allowed_path(path, &Method::GET), "{path}");
        }
    }

    #[test]
    fn mounted_dashboard_keeps_resources_local_and_tracks_presence() {
        let source = "<html lang=\"en\"><head><link href=\"/woodcreek.css\"></head><body><a href=\"https://play.blooket.com/\">Blooket</a><script src=\"/woodcreek.js\"></script></body></html>";
        let result = mounted_html(source);
        assert!(result.contains("data-app-base=\"/blooket-bot\""));
        assert!(result.contains("href=\"/blooket-bot/woodcreek.css\""));
        assert!(result.contains("src=\"/blooket-bot/woodcreek.js\""));
        assert!(result.contains("href=\"https://play.blooket.com/\""));
        assert!(result.contains("src=\"/presence.js?v=1\""));
    }
}
