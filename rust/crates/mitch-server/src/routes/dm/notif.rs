//! DM notification fan-out helpers (server.js:2050-2056, 2186-2258,
//! 2208-2237): per-user ntfy.sh topics (`ntfy_topics.json`), quiet hours, and
//! the `notifAllowed` master gate. Web push itself lives in `push.rs`.

use crate::routes::me::notifications::get_notif_prefs;
use crate::state::AppState;
use mitch_lib::jsval;
use mitch_lib::school::now_millis;
use serde_json::{json, Value};
use std::sync::Arc;

/// `loadNtfyTopics()` (server.js:2189) — `{ email: topic }`.
fn load_ntfy_topics(state: &AppState) -> Value {
    state
        .store
        .read_document(&state.cfg.data_dir.join("ntfy_topics.json"), json!({}))
}

/// `notificationUrl(path)` (server.js:2053-2056) — leading-slash normalize.
pub(crate) fn notification_url(path: &str) -> String {
    let clean = if path.is_empty() { "/" } else { path };
    if clean.starts_with('/') {
        clean.to_string()
    } else {
        format!("/{clean}")
    }
}

/// `notifQuietNow(p)` (server.js:2212-2232).
pub(crate) fn notif_quiet_now(p: &Value) -> bool {
    if !jsval::truthy(p) || !jsval::truthy(&jsval::or(p.get("quietEnabled"), json!(false))) {
        return false;
    }
    let parse = |raw: Option<&Value>| -> Option<i64> {
        // /^(\d{1,2}):(\d{2})$/ — applied to String(s || '').
        let gate = jsval::or(raw, json!(""));
        let s = jsval::string(&gate);
        let b = s.as_bytes();
        let colon = b.iter().position(|&c| c == b':')?;
        let (h, m) = (&b[..colon], &b[colon + 1..]);
        if h.is_empty() || h.len() > 2 || m.len() != 2 {
            return None;
        }
        if !h.iter().all(|c| c.is_ascii_digit()) || !m.iter().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let hh: i64 = std::str::from_utf8(h).ok()?.parse().ok()?;
        let mm: i64 = std::str::from_utf8(m).ok()?.parse().ok()?;
        let v = hh * 60 + mm;
        if (0..1440).contains(&v) {
            Some(v)
        } else {
            None
        }
    };
    let (Some(start), Some(end)) = (parse(p.get("quietStart")), parse(p.get("quietEnd"))) else {
        return false;
    };
    if start == end {
        return false;
    }
    // Number.isFinite(p.tzOffset) ? clamp(round(tzOffset)) : 0.
    let tz = p.get("tzOffset").cloned().unwrap_or(Value::Null);
    let off = match &tz {
        Value::Number(n) => n
            .as_f64()
            .filter(|f| f.is_finite())
            .map(|f| (f.round() as i64).clamp(-840, 840))
            .unwrap_or(0),
        _ => 0,
    };
    // tzOffset uses the JS Date convention (minutes to ADD to local to get
    // UTC), so local minutes-of-day = utc minutes-of-day - tzOffset.
    let utc_min = (now_millis() / 60_000) % 1440;
    let local_min = ((utc_min - off) % 1440 + 1440) % 1440;
    if start < end {
        local_min >= start && local_min < end
    } else {
        // Window wraps midnight.
        local_min >= start || local_min < end
    }
}

/// `notifAllowed(norm, key)` (server.js:2233-2237) — category off OR inside
/// quiet hours. Only an explicit `false` blocks.
pub(crate) fn notif_allowed(state: &AppState, norm: &str, key: &str) -> bool {
    let p = get_notif_prefs(state, norm);
    if p.get(key) == Some(&Value::Bool(false)) {
        return false;
    }
    !notif_quiet_now(&p)
}

/// `new URL(site().primary).origin` — `None` when the primary isn't a usable
/// absolute URL (the JS try/catch keeps the relative click path).
fn site_primary_origin(state: &AppState) -> Option<String> {
    let primary = std::fs::read_to_string(state.cfg.data_dir.join("site.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v.get("primary").and_then(|p| p.as_str()).map(String::from))
        .unwrap_or_else(|| "https://mitch.pro".to_string());
    let scheme_end = primary.find("://")?;
    let rest = &primary[scheme_end + 3..];
    let host_end = rest.find('/').unwrap_or(rest.len());
    let host = &rest[..host_end];
    if host.is_empty() {
        return None;
    }
    Some(format!("{}://{}", &primary[..scheme_end], host))
}

/// `ntfyNotify(email, title, body, url)` (server.js:2238-2258) — the per-user
/// topic variant (`ntfy_topics.json`), distinct from push.rs's env-topic
/// helper. Fire-and-forget: failures never alter the HTTP response.
pub(crate) fn ntfy_notify_user(
    state: &Arc<AppState>,
    email: &str,
    title: &str,
    body: &str,
    url: &str,
) {
    let topics = load_ntfy_topics(state);
    let norm = mitch_lib::auth::normalize_email(email);
    let Some(raw_topic) = topics.get(norm.as_str()) else {
        return;
    };
    let topic = jsval::string(raw_topic).trim().to_string();
    if topic.is_empty() {
        return;
    }
    let target_url = if topic.starts_with("http://") || topic.starts_with("https://") {
        if topic.len() > 1024 {
            return;
        }
        topic
    } else {
        // /^[a-zA-Z0-9_-]{6,64}$/
        let topic_ok = (6..=64).contains(&topic.chars().count())
            && topic
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if !topic_ok {
            return;
        }
        format!("https://ntfy.sh/{}", topic.trim_start_matches('/'))
    };
    // ntfy's Click header must be absolute, so resolve relative notification
    // paths against the configured primary origin.
    let mut click_url = url.to_string();
    if click_url.starts_with('/') {
        if let Some(origin) = site_primary_origin(state) {
            click_url = format!("{origin}{click_url}");
        }
    }
    let body = js_slice400(body);
    let title = if title.is_empty() {
        "Mitch.pro".to_string()
    } else {
        js_slice120(title)
    };
    tokio::spawn(async move {
        let mut req = reqwest::Client::new()
            .post(target_url)
            .header("Title", title)
            .header("Priority", "default")
            .header("Tags", "lock")
            .timeout(std::time::Duration::from_secs(5));
        if !click_url.is_empty() {
            req = req.header("Click", click_url);
        }
        let _ = req.body(body).send().await;
    });
}

/// `String(x || '').slice(0, n)` at the two ntfy call sizes.
fn js_slice400(s: &str) -> String {
    jsval::js_slice_utf16(s, 400)
}
fn js_slice120(s: &str) -> String {
    jsval::js_slice_utf16(s, 120)
}
