//! Push notices through ntfy. Failures are logged and swallowed: a broken
//! notifier must never stop the sync loop.

use crate::config::Ntfy;
use std::time::Duration;

/// A bare topic goes to ntfy.sh; a full http(s) URL is used as given.
pub fn resolve_url(topic: &str) -> String {
    let t = topic.trim();
    if t.starts_with("http://") || t.starts_with("https://") {
        t.to_string()
    } else {
        format!("https://ntfy.sh/{}", t.trim_start_matches('/'))
    }
}

/// Header values must be one line; strip anything that could split them.
fn one_line(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

pub fn send(cfg: &Ntfy, title: &str, message: &str, priority: &str) {
    if cfg.topic.is_empty() {
        return;
    }
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[dbsync] ntfy client: {e}");
            return;
        }
    };
    let mut req = client
        .post(resolve_url(&cfg.topic))
        .header("Title", one_line(title))
        .header("Priority", one_line(priority))
        .header("Tags", "floppy_disk")
        .body(message.to_string());
    if !cfg.user.is_empty() || !cfg.pass.is_empty() {
        req = req.basic_auth(&cfg.user, Some(&cfg.pass));
    }
    if let Err(e) = req.send().and_then(|r| r.error_for_status()) {
        eprintln!("[dbsync] ntfy send failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(resolve_url("abc"), "https://ntfy.sh/abc");
        assert_eq!(resolve_url(" /abc "), "https://ntfy.sh/abc");
        assert_eq!(resolve_url("https://n.example/x"), "https://n.example/x");
    }

    #[test]
    fn header_values_lose_newlines() {
        assert_eq!(one_line("a\r\nb"), "ab");
    }
}
