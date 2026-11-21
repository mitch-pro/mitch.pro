//! `formatHtmlEmail()` from `mail/send_email.js` (shared by all three send
//! scripts): plain-text bodies become an HTML email in the site's look. The
//! shell, footer and holiday palette come from `mitch_lib::email_theme`; raw
//! HTML bodies pass through untouched.

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
    format_html_email_in(
        &mitch_lib::email_theme::palette_today(),
        sender,
        subject,
        text_body,
        unsubscribe_url,
        primary,
        alt,
    )
}

/// `format_html_email` with an explicit palette (tests and previews).
pub fn format_html_email_in(
    palette: &mitch_lib::email_theme::Palette,
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
    let footer = mitch_lib::email_theme::footer(
        palette,
        primary,
        alt,
        unsubscribe_url,
        sender.support_line(unsubscribe_url.is_some()),
    );
    mitch_lib::email_theme::shell(
        palette,
        subject,
        "",
        &paragraphs_html(palette, text_body),
        &footer,
        host_label(primary),
        alt,
    )
}

fn paragraphs_html(palette: &mitch_lib::email_theme::Palette, text_body: &str) -> String {
    let escaped = escape_html(text_body);
    // JS: escapedText.split(/\n\n+/) — one or more blank lines split.
    crate::watcher::static_regex(r"\n\n+")
        .split(&escaped)
        .map(|p| mitch_lib::email_theme::paragraph(palette, &p.replace('\n', "<br>")))
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN_HEAD: &str = "<!DOCTYPE html>\n<html lang=\"en\"";
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
        assert!(html.contains(">Hello!</p>"));
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
            .matches("<p style=\"margin:0 0 16px;font-size:15px;")
            .count();
        assert_eq!(count, 2, "JS \\n\\n+ collapses runs of blank lines");
    }
}
