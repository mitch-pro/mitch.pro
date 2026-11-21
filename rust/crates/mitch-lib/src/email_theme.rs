//! Look of every HTML email the site sends: one shell, one footer and a few
//! building blocks, colored like the site's dark theme and switched to the
//! holiday palette during a season (`seasons`, same windows as the website; see
//! `docs/holiday-themes.md`). The palettes mirror `webserver/theme-tokens.css`.
//!
//! Emails are dark only. Mail clients can't read the visitor's theme the way the
//! website does, and most ignore `prefers-color-scheme` rules for inline styles.
//!
//! New emails should build their body from the helpers here (`heading`,
//! `paragraph`, `button`, `code_box`, `panel`, `fine_print`) and wrap it with
//! `shell`. Older builders that hardcode the pre-redesign purple palette go
//! through `restyle`, which maps those colors onto the current palette.

use chrono::{FixedOffset, NaiveDate, Utc};

/// One complete color set. Every value is a `#rrggbb` string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// Season name from `seasons`, or `"default"`.
    pub name: &'static str,
    /// Small symbol shown in the header during the season; empty otherwise.
    pub glyph: &'static str,
    pub bg: &'static str,
    pub card: &'static str,
    pub panel: &'static str,
    pub ink: &'static str,
    pub text: &'static str,
    pub muted: &'static str,
    pub faint: &'static str,
    pub line: &'static str,
    pub accent: &'static str,
    pub accent2: &'static str,
    /// Text color that is readable on `accent`.
    pub accent_ink: &'static str,
}

pub const DEFAULT: Palette = Palette {
    name: "default",
    glyph: "",
    bg: "#121415",
    card: "#1b1e20",
    panel: "#25292b",
    ink: "#f4f5f1",
    text: "#d9dedb",
    muted: "#a6adaf",
    faint: "#828b8c",
    line: "#363a3c",
    accent: "#ff6f99",
    accent2: "#ff8bad",
    accent_ink: "#2a0a14",
};

const HALLOWEEN: Palette = Palette {
    name: "halloween",
    glyph: "\u{1F383}",
    bg: "#0f0c14",
    card: "#17121f",
    panel: "#241b30",
    ink: "#f6efe6",
    text: "#e3d9d0",
    muted: "#b9a9bd",
    faint: "#92829a",
    line: "#372a47",
    accent: "#ff8a1f",
    accent2: "#ffa24d",
    accent_ink: "#1a1006",
};

const THANKSGIVING: Palette = Palette {
    name: "thanksgiving",
    glyph: "\u{1F342}",
    bg: "#141210",
    card: "#1d1a16",
    panel: "#2a241e",
    ink: "#f6f1ea",
    text: "#e3dccf",
    muted: "#b3a794",
    faint: "#8f8472",
    line: "#3b332b",
    accent: "#f28c38",
    accent2: "#ffa45c",
    accent_ink: "#1a1208",
};

const CHRISTMAS: Palette = Palette {
    name: "christmas",
    glyph: "\u{1F384}",
    bg: "#07170f",
    card: "#0d2418",
    panel: "#143222",
    ink: "#f6f4ea",
    text: "#dce6dd",
    muted: "#b4cdb9",
    faint: "#8fae97",
    line: "#22503a",
    accent: "#f0494f",
    accent2: "#ff6f74",
    accent_ink: "#ffffff",
};

const NEWYEAR: Palette = Palette {
    name: "newyear",
    glyph: "\u{2728}",
    bg: "#090c1b",
    card: "#10152b",
    panel: "#192040",
    ink: "#f4f4fb",
    text: "#dcdcec",
    muted: "#aeb3d4",
    faint: "#868cb3",
    line: "#283159",
    accent: "#f5c451",
    accent2: "#ffd77a",
    accent_ink: "#1c1403",
};

const VALENTINES: Palette = Palette {
    name: "valentines",
    glyph: "\u{1F496}",
    bg: "#150c11",
    card: "#201219",
    panel: "#2e1a25",
    ink: "#fbeff3",
    text: "#ecd9e0",
    muted: "#c4a3b1",
    faint: "#9d7c8b",
    line: "#43232f",
    accent: "#ff5d8f",
    accent2: "#ff85ab",
    accent_ink: "#1f0610",
};

/// Palette for a season name from `seasons`; unknown names get the default.
pub fn palette_named(name: &str) -> Palette {
    match name {
        "halloween" => HALLOWEEN,
        "thanksgiving" => THANKSGIVING,
        "christmas" => CHRISTMAS,
        "newyear" => NEWYEAR,
        "valentines" => VALENTINES,
        _ => DEFAULT,
    }
}

/// Palette for a calendar date.
pub fn palette_for(date: NaiveDate) -> Palette {
    match crate::seasons::current(date) {
        Some((name, _)) => palette_named(name),
        None => DEFAULT,
    }
}

/// Palette for right now. Seasons change at midnight Pacific (the school's
/// time); a fixed UTC-8 offset is close enough for a color scheme.
pub fn palette_today() -> Palette {
    let today = match FixedOffset::west_opt(8 * 3600) {
        Some(zone) => Utc::now().with_timezone(&zone).date_naive(),
        None => Utc::now().date_naive(),
    };
    palette_for(today)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#039;")
}

const FONT: &str =
    "'DM Sans',-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif";

/// `rgba()` of a `#rrggbb` color.
pub fn rgba(hex: &str, alpha: &str) -> String {
    let channel = |i: usize| {
        hex.get(i..i + 2)
            .and_then(|h| u8::from_str_radix(h, 16).ok())
            .unwrap_or(0)
    };
    format!(
        "rgba({}, {}, {}, {})",
        channel(1),
        channel(3),
        channel(5),
        alpha
    )
}

// ── Building blocks ────────────────────────────────────────────────────────

/// Title at the top of an email body.
pub fn heading(p: &Palette, text: &str) -> String {
    format!(
        "<h1 style=\"margin:0 0 14px;font-family:{FONT};font-size:24px;line-height:1.25;font-weight:700;letter-spacing:-0.02em;color:{};\">{}</h1>",
        p.ink,
        escape(text)
    )
}

/// Body paragraph. `inner_html` is inserted as is: escape user text first.
pub fn paragraph(p: &Palette, inner_html: &str) -> String {
    format!(
        "<p style=\"margin:0 0 16px;font-size:15px;line-height:1.65;color:{};\">{inner_html}</p>",
        p.text
    )
}

/// Primary call-to-action button.
pub fn button(p: &Palette, url: &str, label: &str) -> String {
    format!(
        "<table role=\"presentation\" cellpadding=\"0\" cellspacing=\"0\" style=\"margin:8px 0 20px;\"><tr><td bgcolor=\"{a}\" style=\"background-color:{a};border-radius:10px;\"><a href=\"{}\" style=\"display:inline-block;padding:12px 24px;font-family:{FONT};font-size:15px;font-weight:700;color:{};text-decoration:none;\">{}</a></td></tr></table>",
        escape(url),
        p.accent_ink,
        escape(label),
        a = p.accent
    )
}

/// One-time code in a framed box.
pub fn code_box(p: &Palette, code: &str) -> String {
    format!(
        "<div style=\"margin:0 0 20px;padding:18px;text-align:center;border-radius:12px;background-color:{};border:1px solid {};\"><span style=\"font-family:'SFMono-Regular',Menlo,Consolas,monospace;font-size:32px;font-weight:700;letter-spacing:6px;color:{};\">{}</span></div>",
        p.panel,
        p.line,
        p.accent,
        escape(code)
    )
}

/// Raised box around a few related lines. `inner_html` is inserted as is.
pub fn panel(p: &Palette, inner_html: &str) -> String {
    format!(
        "<div style=\"margin:0 0 20px;padding:18px 20px;border-radius:12px;background-color:{};border:1px solid {};\">{inner_html}</div>",
        p.panel, p.line
    )
}

/// Small print under the main content.
pub fn fine_print(p: &Palette, inner_html: &str) -> String {
    format!(
        "<p style=\"margin:0 0 12px;font-size:13px;line-height:1.5;color:{};\">{inner_html}</p>",
        p.faint
    )
}

/// Footer lines: where the mail came from, how to unsubscribe, how to get help.
/// `support_student_line` adds the school address next to the support one.
pub fn footer(
    p: &Palette,
    primary: &str,
    alt: &str,
    unsubscribe_url: Option<&str>,
    support_student_line: bool,
) -> String {
    let label = |url: &str| {
        let rest = url.split_once("://").map_or(url, |(_, r)| r);
        escape(rest.trim_end_matches('/'))
    };
    let link = |url: &str, text: &str, underline: bool| {
        format!(
            "<a href=\"{}\" style=\"color:{};text-decoration:{};\">{text}</a>",
            escape(url),
            p.muted,
            if underline { "underline" } else { "none" }
        )
    };
    let delivered = format!(
        "Delivered by {} | {}",
        link(alt, &label(alt), true),
        link(primary, &label(primary), true)
    );
    let unsub = unsubscribe_url
        .map(|u| {
            format!(
                "<p style=\"margin:0 0 8px;\">To opt-out of these communications, you can {}.</p>",
                link(u, "unsubscribe from this list", true)
            )
        })
        .unwrap_or_default();
    // The school address is configuration, not source: SUPPORT_SCHOOL_EMAIL.
    let student = std::env::var("SUPPORT_SCHOOL_EMAIL")
        .ok()
        .map(|e| e.trim().to_string())
        .filter(|e| support_student_line && !e.is_empty())
        .map(|e| format!(" or {}", escape(&e)))
        .unwrap_or_default();
    format!(
        "<div style=\"margin-top:28px;padding-top:18px;border-top:1px solid {line};font-size:12px;line-height:1.55;color:{faint};text-align:center;\"><p style=\"margin:0 0 8px;\">{delivered}</p>{unsub}<p style=\"margin:0 0 8px;\">For support: email SUPPORT to {support}{student}</p><p style=\"margin:0;font-size:11px;\">2014 Capitol Ave #100, Sacramento, CA 95811</p></div>",
        line = p.line,
        faint = p.faint,
        support = link("mailto:support@mitch.pro", "support@mitch.pro", false),
    )
}

/// Wraps `content_html` and `footer_html` in the email shell. `alt_url` is the
/// host the logo is loaded from; `brand_label` is the wordmark.
pub fn shell(
    p: &Palette,
    subject: &str,
    preheader: &str,
    content_html: &str,
    footer_html: &str,
    brand_label: &str,
    alt_url: &str,
) -> String {
    let glyph = if p.glyph.is_empty() {
        String::new()
    } else {
        format!(
            "<td align=\"right\" style=\"font-size:20px;line-height:1;\" aria-hidden=\"true\">{}</td>",
            p.glyph
        )
    };
    let pre = if preheader.is_empty() {
        String::new()
    } else {
        format!(
            "<div style=\"display:none;font-size:1px;line-height:1px;max-height:0;max-width:0;opacity:0;overflow:hidden;mso-hide:all;\">{}</div>",
            escape(preheader)
        )
    };
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\" style=\"background:{bg};\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n<meta name=\"color-scheme\" content=\"dark\">\n<meta name=\"supported-color-schemes\" content=\"dark\">\n<style>:root{{color-scheme:dark;}}a{{word-break:break-word;}}@media(max-width:600px){{.em-pad{{padding-left:20px!important;padding-right:20px!important;}}}}</style>\n<title>{subject}</title>\n</head>\n<body style=\"margin:0;padding:0;background-color:{bg};font-family:{FONT};color:{ink};-webkit-font-smoothing:antialiased;-webkit-text-size-adjust:100%;\">\n{pre}<table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" bgcolor=\"{bg}\" style=\"background-color:{bg};\"><tr><td align=\"center\" style=\"padding:32px 16px;\">\n<table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" bgcolor=\"{card}\" style=\"max-width:560px;background-color:{card};border:1px solid {line};border-radius:16px;overflow:hidden;\">\n<tr><td height=\"4\" bgcolor=\"{accent}\" style=\"height:4px;font-size:0;line-height:0;background-color:{accent};background-image:linear-gradient(90deg,{accent},{accent2});\">&nbsp;</td></tr>\n<tr><td class=\"em-pad\" style=\"padding:24px 32px 6px;\"><table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\"><tr><td style=\"vertical-align:middle;\"><img src=\"{alt}/favicon.ico\" width=\"22\" height=\"22\" alt=\"\" style=\"vertical-align:middle;margin-right:9px;border-radius:5px;border:0;\"><span style=\"font-size:19px;font-weight:700;letter-spacing:-0.02em;color:{ink};vertical-align:middle;\">{brand}</span></td>{glyph}</tr></table></td></tr>\n<tr><td class=\"em-pad\" style=\"padding:14px 32px 30px;font-size:15px;line-height:1.65;color:{text};\">\n{content_html}\n{footer_html}\n</td></tr>\n</table>\n</td></tr></table>\n</body>\n</html>",
        bg = p.bg,
        card = p.card,
        line = p.line,
        ink = p.ink,
        text = p.text,
        accent = p.accent,
        accent2 = p.accent2,
        subject = escape(subject),
        brand = escape(brand_label),
        alt = alt_url.trim_end_matches('/'),
    )
}

/// Maps the colors older email builders hardcoded (a purple/sky palette on
/// navy) onto `p`, so they match the redesign without being rewritten. Status
/// colors (green, amber, red) are left alone.
pub fn restyle(p: &Palette, html: &str) -> String {
    let mut out = html.to_string();
    // Gradient buttons first, because they also set the text color.
    for gradient in [
        "background: linear-gradient(135deg, #a855f7, #6366f1); color: #ffffff;",
        "background: linear-gradient(135deg, #a855f7, #38bdf8); color: #ffffff;",
    ] {
        out = out.replace(
            gradient,
            &format!("background-color: {}; color: {};", p.accent, p.accent_ink),
        );
    }
    for (from, to) in [
        ("#f4f4f5", p.ink),
        ("#cbd5e1", p.text),
        ("#94a3b8", p.muted),
        ("#64748b", p.faint),
        ("#475569", p.faint),
        ("#a855f7", p.accent),
        ("#c084fc", p.accent),
        ("#6366f1", p.accent),
        ("#38bdf8", p.accent),
    ] {
        out = out.replace(from, to);
    }
    for tint in [(168, 85, 247), (99, 102, 241), (56, 189, 248)] {
        for spacing in [", ", ","] {
            let needle = format!(
                "rgba({}{spacing}{}{spacing}{}{spacing}",
                tint.0, tint.1, tint.2
            );
            while let Some(at) = out.find(&needle) {
                let rest = &out[at + needle.len()..];
                let Some(end) = rest.find(')') else { break };
                let alpha = rest[..end].trim().to_string();
                let replacement = rgba(p.accent, &alpha);
                out.replace_range(at..at + needle.len() + end + 1, &replacement);
            }
        }
    }
    for needle in ["rgba(255, 255, 255, 0.03)", "rgba(255,255,255,0.03)"] {
        out = out.replace(needle, p.panel);
    }
    for needle in ["rgba(255,255,255,0.08)", "rgba(255, 255, 255, 0.08)"] {
        out = out.replace(needle, p.line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn palette_follows_the_season() {
        assert_eq!(palette_for(date(2026, 11, 21)).name, "thanksgiving");
        assert_eq!(palette_for(date(2026, 12, 10)).name, "christmas");
        assert_eq!(palette_for(date(2027, 2, 10)).name, "valentines");
        assert_eq!(palette_for(date(2027, 5, 1)).name, "default");
    }

    #[test]
    fn restyle_replaces_legacy_colors_but_keeps_status_colors() {
        let p = THANKSGIVING;
        let html = "<a style=\"background: linear-gradient(135deg, #a855f7, #6366f1); color: #ffffff;\">go</a><b style=\"color:#f4f4f5;border:1px solid rgba(168, 85, 247, 0.3)\">x</b><i style=\"color:#10b981\">ok</i>";
        let out = restyle(&p, html);
        assert!(!out.contains("#a855f7") && !out.contains("#6366f1"));
        assert!(out.contains("background-color: #f28c38; color: #1a1208;"));
        assert!(out.contains("rgba(242, 140, 56, 0.3)"));
        assert!(out.contains("#10b981"));
    }

    #[test]
    fn shell_escapes_and_carries_the_palette() {
        let p = VALENTINES;
        let html = shell(
            &p,
            "a <b>",
            "",
            "<p>hi</p>",
            "",
            "mitch.pro",
            "https://mitchdog.com/",
        );
        assert!(html.contains("<title>a &lt;b&gt;</title>"));
        assert!(html.contains("#ff5d8f"));
        assert!(html.contains("src=\"https://mitchdog.com/favicon.ico\""));
        assert!(html.contains(p.glyph));
    }

    #[test]
    fn footer_has_unsubscribe_only_when_given() {
        let with = footer(
            &DEFAULT,
            "https://mitch.pro",
            "https://mitchdog.com",
            Some("https://x/u"),
            false,
        );
        let without = footer(
            &DEFAULT,
            "https://mitch.pro",
            "https://mitchdog.com",
            None,
            true,
        );
        assert!(with.contains("unsubscribe from this list"));
        assert!(!without.contains("unsubscribe"));
    }
}
