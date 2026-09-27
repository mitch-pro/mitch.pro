//! IMAP watcher — port of `mail/imap_watcher.js` as a background task.
//!
//! Contract: keeps an IMAP IDLE connection on INBOX (993/TLS), refetches
//! INBOX + Sent on new mail, writes `data/team_inbox_cache.json` atomically
//! (tmp + rename, pretty 2-space JSON — the same shape `imap_watcher.js`
//! writes, which `server.js` reads via `loadJson`'s disk fallback), auto-replies
//! to INBOX mail whose body contains "SUPPORT" (deduped via
//! `data/autoreply_sent.json`), and notifies ntfy.sh.
//!
//! Note: the cache/autoreply files are written with plain fs writes, exactly
//! like the JS watcher (not through the data store), so the on-disk fallback
//! path of `readDocument` keeps serving them to server.js.

use crate::config::MailConfig;
use crate::send::{deliver, prepare, SendRequest};
use mitch_lib::data::DataStore;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const AUTOREPLY_BODY: &str = "Thank you for contacting mitch.pro support. Please reply with your problem and we will get you into contact with a mitch.pro representative as soon as possible.";

/// A static regex that must compile; failure is a programming error.
#[allow(clippy::expect_used)]
pub(crate) fn static_regex(pattern: &str) -> regex::Regex {
    regex::Regex::new(pattern).expect("static regex")
}

/// Liveness signals for GET /watch/status (the JS shim's takeover check).
#[derive(Default)]
pub struct WatcherStatus {
    last_refresh_ms: AtomicU64,
    /// True once the initial fetch has completed.
    pub ready: std::sync::atomic::AtomicBool,
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl WatcherStatus {
    fn refresh(&self) {
        self.last_refresh_ms.store(now_millis(), Ordering::Relaxed);
        self.ready.store(true, Ordering::Relaxed);
    }

    /// Healthy = initial fetch done and refreshed within the IDLE keepalive
    /// window (the imap crate sends NOOP keepalives every ~30 min).
    pub fn healthy(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
            && now_millis().saturating_sub(self.last_refresh_ms.load(Ordering::Relaxed))
                < 35 * 60 * 1000
    }
}

fn log(msg: &str) {
    tracing::info!("[imap-watcher] {msg}");
}

/// Normalize subject for thread grouping — JS:
/// `subject.replace(/^(re|fwd?):\s*/i, '').trim().toLowerCase()` (one prefix).
pub fn thread_key(subject: &str) -> String {
    static_regex(r"(?i)^(re|fwd?):\s*")
        .replace(subject, "")
        .trim()
        .to_lowercase()
}

/// Byte-faithful port of `extractText(raw)` from `imap_watcher.js`.
pub fn extract_text(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    let boundary = static_regex(r#"(?i)boundary="?([^"\r\n;]+)"?"#)
        .captures(raw)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string());
    if let Some(boundary) = boundary {
        let split_re = static_regex(&format!("--{}", regex::escape(&boundary)));
        let ct = static_regex(r"(?i)content-type:\s*text/plain");
        let mut texts = Vec::new();
        for part in split_re.split(raw) {
            if !ct.is_match(part) {
                continue;
            }
            let (sep_len, idx) = match part.find("\r\n\r\n") {
                Some(i) => (4, i),
                None => match part.find("\n\n") {
                    Some(i) => (2, i),
                    None => continue,
                },
            };
            texts.push(part[idx + sep_len..].trim());
        }
        if !texts.is_empty() {
            return texts.join("\n").trim().to_string();
        }
    }
    if static_regex(r"(?im)^content-type:").is_match(raw) {
        if let Some(idx) = raw.find("\r\n\r\n") {
            return raw[idx + 4..].trim().to_string();
        }
        if let Some(idx) = raw.find("\n\n") {
            return raw[idx + 2..].trim().to_string();
        }
    }
    raw.trim().to_string()
}

/// The `messageId || "from|subject"` identity used for known-IDs and the
/// autoreply dedup set.
fn message_key(m: &serde_json::Value) -> String {
    let id = m.get("messageId").and_then(|v| v.as_str()).unwrap_or("");
    if id.is_empty() {
        format!(
            "{}|{}",
            m.get("from").and_then(|v| v.as_str()).unwrap_or(""),
            m.get("subject").and_then(|v| v.as_str()).unwrap_or("")
        )
    } else {
        id.to_string()
    }
}

/// One IMAP fetch pass: INBOX (support-only) + Sent (external recipients whose
/// threads appear in the inbox), sorted newest-first. Returns the messages.
fn fetch_all(
    session: &mut imap::Session<native_tls::TlsStream<std::net::TcpStream>>,
) -> Result<Vec<serde_json::Value>, String> {
    let inbox = fetch_mailbox(session, "INBOX")?;
    let inbox_keys: std::collections::HashSet<String> = inbox
        .iter()
        .filter_map(|m| {
            m.get("threadKey")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .collect();
    let sent = fetch_mailbox(session, "Sent")?;
    let mut all = inbox;
    all.extend(sent.into_iter().filter(|m| {
        m.get("threadKey")
            .and_then(|v| v.as_str())
            .map(|k| inbox_keys.contains(k))
            .unwrap_or(false)
    }));
    all.sort_by_cached_key(|m| {
        std::cmp::Reverse(
            m.get("date")
                .and_then(|d| d.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|t| t.timestamp_millis())
                .unwrap_or(0),
        )
    });
    Ok(all)
}

/// Port of `fetchMailbox()`: opens `mailbox`, filters support/external
/// messages, builds cache entries. Mailbox-not-found yields an empty list.
fn fetch_mailbox(
    session: &mut imap::Session<native_tls::TlsStream<std::net::TcpStream>>,
    mailbox: &str,
) -> Result<Vec<serde_json::Value>, String> {
    use mail_parser::MessageParser;

    let exists = match session.select(mailbox) {
        Ok(sel) => sel.exists,
        Err(_) => return Ok(Vec::new()),
    };
    let is_inbox = mailbox == "INBOX";
    let mut messages = Vec::new();
    if exists == 0 {
        return Ok(messages);
    }
    let fetches = session
        .fetch("1:*", "(UID FLAGS BODY.PEEK[])")
        .map_err(|e| format!("fetch {mailbox}: {e}"))?;
    for msg in fetches.iter() {
        let Some(source) = msg.body() else { continue };
        let source_str = String::from_utf8_lossy(source).to_string();
        let Some(parsed) = MessageParser::default().parse(source) else {
            continue;
        };
        let from = parsed
            .from()
            .and_then(|f| f.first())
            .and_then(|a| a.address())
            .unwrap_or("");
        let from_name = parsed
            .from()
            .and_then(|f| f.first())
            .and_then(|a| a.name())
            .unwrap_or("");
        let to_addrs: Vec<String> = parsed
            .to()
            .map(|t| {
                t.iter()
                    .filter_map(|a| a.address().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let to = to_addrs.join(", ");

        // Inbox: only support@ emails. Sent: only emails to external addresses.
        if is_inbox && !to.to_lowercase().contains("support@mitch.pro") {
            continue;
        }
        if !is_inbox
            && to_addrs
                .iter()
                .all(|a| a.to_lowercase().ends_with("@mitch.pro"))
        {
            continue;
        }

        let subject = parsed.subject().unwrap_or("(no subject)").to_string();
        let date = parsed
            .date()
            .and_then(|d| chrono::DateTime::<chrono::Utc>::from_timestamp(d.to_timestamp(), 0))
            .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            .unwrap_or_default();
        let message_id = parsed.message_id().unwrap_or_default().to_string();
        let seen = msg.flags().contains(&imap::types::Flag::Seen);

        messages.push(json!({
            "uid": msg.uid,
            "mailbox": mailbox,
            "dir": if is_inbox { "in" } else { "out" },
            "from": from,
            "fromName": from_name,
            "to": to,
            "subject": subject,
            "threadKey": thread_key(&subject),
            "date": date,
            "messageId": message_id.clone(),
            "references": message_id,
            "seen": seen,
            "body": extract_text(&source_str),
        }));
    }
    Ok(messages)
}

fn write_cache(cache_file: &std::path::Path, messages: &[serde_json::Value]) -> Result<(), String> {
    let tmp = cache_file.with_extension("json.tmp");
    let content = serde_json::to_string_pretty(messages).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, content).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, cache_file).map_err(|e| e.to_string())?;
    let inbox = messages.iter().filter(|m| m["dir"] == "in").count();
    let sent = messages.iter().filter(|m| m["dir"] == "out").count();
    log(&format!("Cache updated: {inbox} received, {sent} sent"));
    Ok(())
}

/// Port of `checkForSupportEmails` + `sendSupportAutoreply` + ntfy. The dedup
/// list keeps JS insertion order (a plain array written to disk).
fn check_for_support_emails(
    messages: &[serde_json::Value],
    prev_ids: &std::collections::HashSet<String>,
    cfg: &MailConfig,
    store: &DataStore,
) -> Result<(), String> {
    let autoreply_file = cfg.data_dir.join("autoreply_sent.json");
    let mut sent_list: Vec<String> = std::fs::read_to_string(&autoreply_file)
        .ok()
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .unwrap_or_default();
    let mut sent_set: std::collections::HashSet<String> = sent_list.iter().cloned().collect();
    let changed_start_len = sent_list.len();
    for msg in messages {
        if msg.get("dir").and_then(|v| v.as_str()) != Some("in") {
            continue;
        }
        let from = msg.get("from").and_then(|v| v.as_str()).unwrap_or("");
        let subject = msg.get("subject").and_then(|v| v.as_str()).unwrap_or("");
        let body = msg.get("body").and_then(|v| v.as_str()).unwrap_or("");
        let key = message_key(msg);
        if sent_set.contains(&key) {
            continue;
        }
        if prev_ids.contains(&key) {
            continue; // skip emails we've already seen
        }
        if !body.contains("SUPPORT") {
            continue;
        }
        sent_set.insert(key.clone());
        sent_list.push(key);
        ntfy_blocking(
            &format!("SUPPORT email from {from}: {subject}"),
            "Support Request",
            "high",
            &cfg.ntfy_topic,
            &cfg.ntfy_user,
            &cfg.ntfy_pass,
        );
        // sendSupportAutoreply
        let reply_subject = if subject.starts_with("Re:") {
            subject.to_string()
        } else {
            format!("Re: {subject}")
        };
        let in_reply_to = if msg
            .get("messageId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .is_empty()
        {
            None
        } else {
            Some(format!("<{}>", msg["messageId"].as_str().unwrap_or("")))
        };
        let req = SendRequest {
            sender: "support".into(),
            to: from.to_string(),
            subject: reply_subject,
            body: AUTOREPLY_BODY.into(),
            in_reply_to,
            alt: false,
            raw: false,
            dry_run: false,
        };
        match prepare(store, cfg, &req).and_then(|p| deliver(&p)) {
            Ok(()) => log(&format!("[autoreply] Sent to {from}")),
            Err(e) => log(&format!("[autoreply] Failed to {from}: {e}")),
        }
    }
    if sent_list.len() > changed_start_len {
        let content = serde_json::to_string(&sent_list).map_err(|e| e.to_string())?;
        std::fs::write(&autoreply_file, content).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn ntfy_blocking(msg: &str, title: &str, priority: &str, topic: &str, user: &str, pass: &str) {
    let topic = topic.trim();
    if topic.is_empty() {
        return;
    }
    let url = if topic.starts_with("http://") || topic.starts_with("https://") {
        topic.to_string()
    } else {
        format!("https://ntfy.sh/{}", topic.trim_start_matches('/'))
    };
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut req = client
        .post(url)
        .body(msg.to_string())
        .header("Title", title)
        .header("Priority", priority);
    if !user.is_empty() || !pass.is_empty() {
        req = req.basic_auth(user, Some(pass));
    }
    let _ = req.send();
}

/// One watcher session: connect, initial fetch+cache, IDLE loop. Returns on
/// disconnect; the outer loop reconnects after 10s (like `runWithReconnect`).
fn run_once(cfg: &MailConfig, store: &DataStore, status: &WatcherStatus) -> Result<(), String> {
    let tls = native_tls::TlsConnector::builder()
        .build()
        .map_err(|e| format!("tls: {e}"))?;
    let host = cfg.imap_host.as_str();
    let client =
        imap::connect((host, 993), host, &tls).map_err(|e| format!("imap connect: {e}"))?;
    let user = crate::config::env_trim("SUPPORT_USER");
    let pass = crate::config::env_trim("SUPPORT_PASS");
    let mut session = client
        .login(&user, &pass)
        .map_err(|(e, _)| format!("login: {e:?}"))?;
    log("Connected");

    let mut known_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    // IDLE loop: refetch on every wake (exists events via keepalive), cap at
    // 29 min so liveness stays observable. First pass only seeds known ids
    // (like the JS: initial fetch does not autoreply).
    loop {
        let messages = fetch_all(&mut session)?;
        write_cache(&cfg.data_dir.join("team_inbox_cache.json"), &messages)?;
        status.refresh();
        if status.ready.load(Ordering::Relaxed) {
            check_for_support_emails(&messages, &known_ids, cfg, store)?;
        } else {
            log("IDLE — waiting for new mail…");
        }
        known_ids = messages.iter().map(message_key).collect();

        // Keepalive NOOPs (like imapflow's idle) wake us every ~30 min even
        // without mail, which keeps /watch/status fresh.
        let handle = session.idle().map_err(|e| format!("idle: {e}"))?;
        handle
            .wait_keepalive()
            .map_err(|e| format!("idle wait: {e}"))?;
        log("New mail — refreshing cache…");
    }
}

/// Spawned as a std::thread from main: reconnect loop, `runOnce` + 10s delay.
pub fn spawn_watcher(
    cfg: Arc<MailConfig>,
    store: Arc<DataStore>,
    status: Arc<WatcherStatus>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || loop {
        if let Err(e) = run_once(&cfg, &store, &status) {
            log(&format!("Disconnected: {e}. Reconnecting in 10s…"));
        }
        std::thread::sleep(Duration::from_secs(10));
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_text_matches_js_algorithm() {
        // No headers → raw trim.
        assert_eq!(extract_text("  hello  "), "hello");
        // Single part.
        let simple = "Content-Type: text/plain\r\n\r\nthe body";
        assert_eq!(extract_text(simple), "the body");
        // Multipart with boundary: text/plain part wins.
        let multi = "Content-Type: multipart/alternative; boundary=\"BB\"\r\n\r\n\
            --BB\r\nContent-Type: text/plain\r\n\r\nplain part\r\n\
            --BB\r\nContent-Type: text/html\r\n\r\n<b>html</b>\r\n--BB--";
        assert_eq!(extract_text(multi), "plain part");
        // No text/plain parts → falls through to the header slice, exactly
        // like the JS (which returns everything after the first blank line —
        // including the raw "--CC\r\n..." remnant).
        let no_plain = "Content-Type: multipart/mixed; boundary=CC\r\n\r\n--CC\r\nContent-Type: text/html\r\n\r\nbody-after-header";
        assert_eq!(
            extract_text(no_plain),
            "--CC\r\nContent-Type: text/html\r\n\r\nbody-after-header"
        );
    }

    #[test]
    fn thread_key_strips_one_reply_prefix() {
        assert_eq!(thread_key("Re: Hello"), "hello");
        assert_eq!(
            thread_key("RE:   FWD: x"),
            "fwd: x",
            "JS strips only one prefix"
        );
        assert_eq!(thread_key("(no subject)"), "(no subject)");
    }

    #[test]
    fn support_detection_would_fire() {
        let body = extract_text("Content-Type: text/plain\r\n\r\nSUPPORT please help");
        assert!(body.contains("SUPPORT"));
    }
}
