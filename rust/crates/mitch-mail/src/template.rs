//! Byte-exact port of `formatHtmlEmail()` from `mail/send_email.js`
//! (shared by all three send scripts). The HTML literal below is copied
//! verbatim from the JS template string; do not re-indent it.

/// The "or mitchell.fogler@student.rjuhsd.us" suffix appears in different
/// footers per sender: gmail only when an unsubscribe link is present,
/// support always, noreply never.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sender {
    Gmail,
    Noreply,
    Support,
}

impl Sender {
    pub fn support_line(self, has_unsubscribe: bool) -> bool {
        match self {
            Sender::Gmail => has_unsubscribe,
            Sender::Support => true,
            Sender::Noreply => false,
        }
    }
}

/// `formatHtmlEmail`'s HTML-content detection: raw HTML bodies pass through.
pub fn is_html_body(text_body: &str) -> bool {
    let trimmed = text_body.trim_start();
    trimmed.starts_with('<')
        || crate::watcher::static_regex(r"(?i)<[a-z][\s\S]*>").is_match(text_body)
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#039;")
}

/// Paragraph conversion: escape, split on blank lines, join `<br>` inside
/// each paragraph. Order matches the JS exactly (escape happens first).
/// Display label for a site URL: the host without scheme or trailing slash.
/// Branding follows `data/site.json`, so nothing here hardcodes a domain.
pub fn host_label(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.trim_end_matches('/')
}

fn footer_html(
    unsubscribe_url: Option<&str>,
    primary: &str,
    alt: &str,
    support_line: bool,
) -> String {
    let support_suffix = if support_line {
        " or mitchell.fogler@student.rjuhsd.us"
    } else {
        ""
    };
    let (primary_label, alt_label) = (host_label(primary), host_label(alt));
    let delivered = format!(
        "          Delivered by <a href=\"{alt}\" style=\"color: #64748b; text-decoration: underline; font-weight: 600;\">{alt_label}</a> | <a href=\"{primary}\" style=\"color: #64748b; text-decoration: underline;\">{primary_label}</a>"
    );
    if let Some(unsub) = unsubscribe_url {
        format!(
            "\n      <div style=\"margin-top: 32px; padding-top: 16px; border-top: 1px solid rgba(255,255,255,0.08); font-size: 12px; color: #64748b; line-height: 1.5; text-align: center;\">\n        <p style=\"margin: 0 0 8px;\">\n{delivered}\n        </p>\n        <p style=\"margin: 0 0 8px;\">\n          To opt-out of these communications, you can <a href=\"{unsub}\" style=\"color: #38bdf8; text-decoration: underline;\">unsubscribe from this list</a>.\n        </p>\n        <p style=\"margin: 0 0 8px;\">\n          For support: email SUPPORT to <a href=\"mailto:support@mitch.pro\" style=\"color: #64748b; text-decoration: none;\">support@mitch.pro</a>{support_suffix}\n        </p>\n        <p style=\"margin: 8px 0 0; font-size: 11px; color: #475569;\">\n          2014 Capitol Ave #100, Sacramento, CA 95811\n        </p>\n      </div>\n    "
        )
    } else {
        format!(
            "\n      <div style=\"margin-top: 32px; padding-top: 16px; border-top: 1px solid rgba(255,255,255,0.08); font-size: 12px; color: #64748b; line-height: 1.5; text-align: center;\">\n        <p style=\"margin: 0 0 8px;\">\n{delivered}\n        </p>\n        <p style=\"margin: 0;\">\n          For support: email SUPPORT to <a href=\"mailto:support@mitch.pro\" style=\"color: #64748b; text-decoration: none;\">support@mitch.pro</a>{support_suffix}\n        </p>\n        <p style=\"margin: 8px 0 0; font-size: 11px; color: #475569;\">\n          2014 Capitol Ave #100, Sacramento, CA 95811\n        </p>\n      </div>\n    "
        )
    }
}

/// Port of `formatHtmlEmail(subject, textBody, unsubscribeUrl, primaryUrl,
/// altUrl)` — returns the trimmed HTML document.
pub fn format_html_email(
    sender: Sender,
    subject: &str,
    text_body: &str,
    unsubscribe_url: Option<&str>,
    primary: &str,
    alt: &str,
) -> String {
    if is_html_body(text_body) {
        return text_body.to_string();
    }

    let paragraphs = paragraphs_html(text_body);
    let (primary_label, alt_label) = (host_label(primary), host_label(alt));
    let footer = footer_html(
        unsubscribe_url,
        primary,
        alt,
        sender.support_line(unsubscribe_url.is_some()),
    );

    format!(
        "<!DOCTYPE html>\n<html lang=\"en\" style=\"background:#06060c;\">\n<head>\n  <meta charset=\"utf-8\">\n  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n  <meta name=\"color-scheme\" content=\"light dark\">\n  <meta name=\"supported-color-schemes\" content=\"light dark\">\n  <style> :root {{ color-scheme: light dark; supported-color-schemes: light dark; }} </style>\n  <title>{subject}</title>\n</head>\n<body style=\"margin: 0; padding: 0; background-color: #06060c; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif; color: #f8fafc; -webkit-font-smoothing: antialiased;\">\n  <table border=\"0\" cellpadding=\"0\" cellspacing=\"0\" width=\"100%\" bgcolor=\"#06060c\" style=\"background-color: #06060c; border-collapse: collapse;\">\n    <tr>\n      <td align=\"center\" bgcolor=\"#06060c\" style=\"background-color: #06060c; padding: 40px 20px;\">\n        <table border=\"0\" cellpadding=\"0\" cellspacing=\"0\" width=\"100%\" style=\"max-width: 580px; background-color: #0f172a; border-radius: 16px; overflow: hidden; border: 1px solid #0f172a; box-shadow: 0 20px 40px rgba(0,0,0,0.5);\">\n          <tr>\n            <td height=\"6\" style=\"background: linear-gradient(to right, #a855f7, #38bdf8);\"></td>\n          </tr>\n          <tr>\n            <td style=\"padding: 32px 32px 16px;\">\n              <table border=\"0\" cellpadding=\"0\" cellspacing=\"0\" width=\"100%\">\n                <tr>\n                  <td style=\"vertical-align: middle;\">\n                    <img src=\"{alt}/favicon.ico\" width=\"24\" height=\"24\" style=\"vertical-align: middle; margin-right: 10px; border-radius: 4px;\" alt=\"{primary_label}\">\n                    <span style=\"font-size: 24px; font-weight: 800; color: #ffffff; vertical-align: middle; letter-spacing: -0.02em;\">{primary_label}</span>\n                    <span style=\"font-size: 24px; font-weight: 300; color: #64748b; vertical-align: middle; margin: 0 8px;\">/</span>\n                    <span style=\"font-size: 24px; font-weight: 800; color: #38bdf8; vertical-align: middle; letter-spacing: -0.02em;\">{alt_label}</span>\n                  </td>\n                </tr>\n              </table>\n            </td>\n          </tr>\n          <tr>\n            <td style=\"padding: 0 32px 32px; font-size: 15px; color: #cbd5e1; line-height: 1.6;\">\n              {paragraphs}\n              {footer}\n            </td>\n          </tr>\n        </table>\n      </td>\n    </tr>\n  </table>\n</body>\n</html>"
    )
}

fn paragraphs_html(text_body: &str) -> String {
    let escaped = escape_html(text_body);
    // JS: escapedText.split(/\n\n+/) — one or more blank lines split.
    crate::watcher::static_regex(r"\n\n+")
        .split(&escaped)
        .map(|p| {
            format!(
                "<p style=\"margin: 0 0 16px; line-height: 1.6;\">{}</p>",
                p.replace('\n', "<br>")
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN_HEAD: &str = "<!DOCTYPE html>\n<html lang=\"en\" style=\"background:#06060c;\">";
    const GOLDEN_TAIL: &str = "</body>\n</html>";

    #[test]
    fn plain_text_becomes_branded_html() {
        let html = format_html_email(
            Sender::Noreply,
            "Test Subject",
            "Hello!\n\nSecond para\nwith break",
            Some("https://mitch.pro/unsubscribe/tok"),
            "https://mitch.pro",
            "https://mitchdog.com",
        );
        assert!(html.starts_with(GOLDEN_HEAD));
        assert!(html.ends_with(GOLDEN_TAIL));
        assert!(html.contains("<title>Test Subject</title>"));
        assert!(html.contains("<p style=\"margin: 0 0 16px; line-height: 1.6;\">Hello!</p>"));
        assert!(html.contains("Second para<br>with break"));
        assert!(html.contains("unsubscribe from this list"));
        assert!(
            !html.contains("mitchell.fogler@student.rjuhsd.us"),
            "noreply footer has no student line"
        );
    }

    #[test]
    fn gmail_footer_includes_student_line_only_with_unsubscribe() {
        let with = format_html_email(
            Sender::Gmail,
            "s",
            "b",
            Some("https://mitch.pro/unsubscribe/t"),
            "https://mitch.pro",
            "https://mitchdog.com",
        );
        let without = format_html_email(
            Sender::Gmail,
            "s",
            "b",
            None,
            "https://mitch.pro",
            "https://mitchdog.com",
        );
        assert!(with.contains(" or mitchell.fogler@student.rjuhsd.us"));
        assert!(!without.contains("mitchell.fogler"));
        assert!(!without.contains("unsubscribe from this list"));
    }

    #[test]
    fn support_footer_always_includes_student_line() {
        let raw = format_html_email(
            Sender::Support,
            "s",
            "b",
            None,
            "https://mitch.pro",
            "https://mitch.88chan.me",
        );
        assert!(raw.contains(" or mitchell.fogler@student.rjuhsd.us"));
    }

    #[test]
    fn html_bodies_pass_through_unchanged() {
        let body = "<div>already html</div>";
        let html = format_html_email(Sender::Noreply, "s", body, None, "p", "a");
        assert_eq!(html, body);
    }

    #[test]
    fn escaping_matches_js() {
        let html = format_html_email(
            Sender::Noreply,
            "s",
            "a & b < c > d \" e ' f",
            None,
            "p",
            "a",
        );
        assert!(html.contains("a &amp; b &lt; c &gt; d &quot; e &#039; f"));
    }

    #[test]
    fn blank_line_splitting_matches_js_regex() {
        let html = format_html_email(Sender::Noreply, "s", "one\n\n\n\ntwo", None, "p", "a");
        let count = html
            .matches("<p style=\"margin: 0 0 16px; line-height: 1.6;\">")
            .count();
        assert_eq!(count, 2, "JS \\n\\n+ collapses runs of blank lines");
    }
}
