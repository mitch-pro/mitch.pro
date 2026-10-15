//! `errorPage`, `errResp`, `bannedResponse`, `jsonResp` — byte-faithful ports
//! from server.js (canned titles/copy per code, Content-Type only header).
#![allow(clippy::expect_used)] // infallible static responses

use axum::http::header;
use axum::response::Response;

fn error_page(code: u16, title: &str, detail_html: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n<title>{code} — {title}</title>\n<style>\nbody{{margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;\nbackground:#0e0e14;color:#bababa;font-family:system-ui,sans-serif;padding:2rem;}}\n.box{{text-align:center;max-width:420px;}}\n.code{{font-size:5rem;font-weight:800;line-height:1;color:#2a2a38;margin-bottom:.5rem;}}\nh1{{font-size:1.3rem;font-weight:700;color:#e8e6e3;margin:0 0 .75rem;}}\np{{font-size:.9rem;line-height:1.6;color:#888;margin:0 0 1.5rem;}}\np a{{color:#7c6aed;text-decoration:none;}}p a:hover{{text-decoration:underline;}}\ncode{{background:#1a1a24;padding:2px 6px;border-radius:4px;font-size:.85em;color:#a0a0c0;}}\n</style></head><body>\n<div class=\"box\"><div class=\"code\">{code}</div>\n<h1>{title}</h1><p>{detail_html}</p></div>\n</body></html>"
    )
}

pub fn html_esc(v: &str) -> String {
    v.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Shared account/network ban notice. Staff-supplied text is always escaped.
pub fn banned_response(info: &serde_json::Value, school: bool) -> Response {
    let reason = info
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .filter(|reason| !reason.trim().is_empty())
        .unwrap_or("Your account has been banned by site staff.");
    let reviewed = info.get("banned_at").and_then(serde_json::Value::as_i64);
    let expiry = info.get("expires_at").and_then(serde_json::Value::as_i64);
    let date = |ms: i64| {
        chrono::DateTime::from_timestamp_millis(ms)
            .map(|date| date.format("%B %-d, %Y at %H:%M UTC").to_string())
    };
    let title = match (reviewed, expiry) {
        (Some(start), Some(end)) => {
            let hours = (end - start + 30_000) / 3_600_000;
            if hours < 24 {
                format!(
                    "Banned for {} hour{}",
                    hours.max(1),
                    if hours <= 1 { "" } else { "s" }
                )
            } else {
                let days = hours / 24;
                format!("Banned for {days} day{}", if days == 1 { "" } else { "s" })
            }
        }
        _ => "Permanently banned".to_string(),
    };
    let review = reviewed
        .and_then(date)
        .map(|date| format!("<p class=\"review\">Reviewed: {date}</p>"))
        .unwrap_or_default();
    let return_text = match expiry.and_then(date) {
        Some(date) => format!("You can use your account again after {date}. Access returns automatically when the ban ends."),
        None => "This ban has no end date. Your account can only be restored by site staff.".to_string(),
    };
    let html = include_str!("templates/ban.html")
        .replace("{{title}}", &title)
        .replace("{{review}}", &review)
        .replace("{{return}}", &return_text)
        .replace("{{reason}}", &html_esc(reason));
    let html = crate::inject::inject_credit(&html, school);
    Response::builder()
        .status(403)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "private, no-store")
        .header(header::VARY, "Cookie")
        .header("x-content-type-options", "nosniff")
        .body(axum::body::Body::from(html))
        .expect("static response")
}

fn err_title(code: u16) -> &'static str {
    match code {
        400 => "Bad Request",
        401 => "Not Authorised",
        403 => "Forbidden",
        404 => "Page Not Found",
        405 => "Method Not Allowed",
        429 => "Too Many Requests",
        500 => "Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

fn err_detail(code: u16) -> &'static str {
    match code {
        400 => "The request could not be understood.",
        401 => "You need to be logged in to view this page. <a href=\"/enroll.html\">Request access</a>.",
        403 => "You don't have permission to access this. <a href=\"/\">Go home</a>.",
        404 => "This page doesn't exist. <a href=\"/\">Go home</a>.",
        405 => "That request method isn't allowed here.",
        429 => "You're sending too many requests. Slow down and try again.",
        500 => "Something went wrong on our end. <a href=\"mailto:support@mitch.pro\">Contact support</a> if it keeps happening.",
        502 => "Upstream error. Try again in a moment.",
        503 => "The service is temporarily unavailable. Try again shortly.",
        _ => "An unexpected error occurred.",
    }
}

/// `errResp(code, message, explain)` — explicit pair overrides canned copy.
pub fn err_resp(code: u16, message: Option<&str>, explain: Option<&str>) -> Response {
    let title = message.unwrap_or(err_title(code));
    let detail = explain.unwrap_or(err_detail(code));
    Response::builder()
        .status(code)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(axum::body::Body::from(error_page(code, title, detail)))
        .expect("static response")
}

/// `jsonResp(code, obj)`.
pub fn json_resp(code: u16, obj: serde_json::Value) -> Response {
    json_resp_str(code, obj.to_string())
}

/// `jsonResp` for a pre-serialized body — used by the game endpoints whose
/// responses carry passthrough session state, so numbers can be rendered with
/// the exact ECMAScript `Number::toString` via `mitch_lib::data::js_stringify`
/// (serde_json would emit `1.0`/`1e22` where JS emits `1`/`1e+22`).
pub fn json_resp_str(code: u16, body: String) -> Response {
    Response::builder()
        .status(code)
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::CACHE_CONTROL,
            "private, no-cache, no-store, must-revalidate",
        )
        .header(header::PRAGMA, "no-cache")
        .header(header::VARY, "Cookie")
        .header("x-content-type-options", "nosniff")
        .body(axum::body::Body::from(body))
        .expect("static response")
}
