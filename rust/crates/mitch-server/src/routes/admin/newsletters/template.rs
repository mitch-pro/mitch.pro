use super::Draft;

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn normalize_img_url(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.starts_with('/') && !trimmed.starts_with("//") {
        format!("https://mitchdog.com{trimmed}")
    } else {
        trimmed.to_string()
    }
}

fn paragraphs(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .split("\n\n")
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            format!(
                "<p style=\"margin:0 0 16px;\">{}</p>",
                escape(p).replace('\n', "<br>")
            )
        })
        .collect()
}

pub(super) fn render(draft: &Draft, unsubscribe: &str) -> String {
    let (outer, paper, panel, ink, muted, line, header) = if draft.theme == "paper" {
        (
            "#f1f3f7", "#ffffff", "#f7f9fc", "#172033", "#536176", "#e3e8ef", "#172033",
        )
    } else {
        (
            "#080e1b", "#111b2e", "#19243a", "#f1f5f9", "#b4c0d4", "#2c3b52", "#0b1120",
        )
    };
    let accent = &draft.accent;
    let r = u8::from_str_radix(&accent[1..3], 16).unwrap_or(0) as u32;
    let g = u8::from_str_radix(&accent[3..5], 16).unwrap_or(0) as u32;
    let b = u8::from_str_radix(&accent[5..7], 16).unwrap_or(0) as u32;
    let accent_ink = if r * 299 + g * 587 + b * 114 > 140_000 {
        "#111b17"
    } else {
        "#ffffff"
    };
    let subject = escape(&draft.subject);
    let title = escape(&draft.title);
    let preheader = escape(&draft.preheader);
    let intro = paragraphs(&draft.intro);
    let signature = escape(&draft.signature);
    let signoff_sub = if draft.signoff_sub.trim().is_empty() {
        String::new()
    } else {
        format!(
            r#"<p style="margin:0;font:12px/1.4 Arial,Helvetica,sans-serif;color:{muted};">{}</p>"#,
            escape(&draft.signoff_sub)
        )
    };
    let avatar_src = normalize_img_url(&draft.avatar_url);
    let avatar_cell = if avatar_src.is_empty() {
        format!(
            r#"<td width="36" height="36" align="center" bgcolor="{accent}" style="background:{accent};border-radius:50%;font:700 13px Arial,Helvetica,sans-serif;color:{accent_ink};">m.</td>"#
        )
    } else {
        format!(
            r#"<td width="36" height="36" align="center" style="width:36px;height:36px;border-radius:50%;overflow:hidden;"><img src="{}" width="36" height="36" alt="" style="display:block;width:36px;height:36px;border-radius:50%;object-fit:cover;border:0;"></td>"#,
            escape(&avatar_src)
        )
    };
    let edition = if draft.edition.trim().is_empty() {
        String::new()
    } else {
        format!("<p style=\"margin:0 0 14px;font:600 13px/1.5 Arial,Helvetica,sans-serif;color:{muted};\">{}</p>", escape(&draft.edition))
    };
    let hero = if draft.hero_url.trim().is_empty() {
        String::new()
    } else {
        let hero_src = normalize_img_url(&draft.hero_url);
        format!("<tr><td class=\"content\" style=\"padding:0 40px 28px;\"><img src=\"{}\" width=\"620\" alt=\"\" style=\"display:block;width:100%;max-width:620px;height:auto;border:0;border-radius:8px;\"></td></tr>", escape(&hero_src))
    };
    let sections: String = draft.sections.iter()
        .filter(|section| !section.title.trim().is_empty() || !section.body.trim().is_empty())
        .map(|section| format!(r#"<tr><td class="content" style="padding:0 40px 16px;">
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" bgcolor="{panel}" style="background:{panel};border:1px solid {line};border-radius:8px;"><tr><td class="section-content" style="padding:24px 26px 8px;">
<h2 style="margin:0 0 14px;font:700 21px/1.35 Arial,Helvetica,sans-serif;letter-spacing:-.45px;color:{ink};">{}</h2>
<div style="font:15px/1.75 Arial,Helvetica,sans-serif;color:{muted};">{}</div>
</td></tr></table></td></tr>"#, escape(&section.title), paragraphs(&section.body)))
        .collect();
    let button = if draft.button_label.trim().is_empty() || draft.button_url.is_empty() {
        String::new()
    } else {
        format!(
            r#"<tr><td class="content" style="padding:12px 40px 32px;"><table role="presentation" cellpadding="0" cellspacing="0"><tr><td bgcolor="{accent}" style="background:{accent};border-radius:6px;"><a href="{}" style="display:inline-block;padding:14px 22px;font:700 14px/1.5 Arial,Helvetica,sans-serif;color:{accent_ink};text-decoration:none;">{}</a></td></tr></table></td></tr>"#,
            escape(&draft.button_url),
            escape(&draft.button_label)
        )
    };
    let unsubscribe = escape(unsubscribe);
    format!(
        r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{subject}</title>
<style>@media(max-width:600px){{.outer{{padding:12px 8px!important}}.content{{padding-left:22px!important;padding-right:22px!important}}.section-content{{padding:20px 18px 4px!important}}.headline{{font-size:34px!important;letter-spacing:-1px!important}}.masthead-credit{{font-size:12px!important}}}}a{{word-break:break-word}}</style></head>
<body style="margin:0;background:{outer};color:{ink};-webkit-text-size-adjust:100%;">
<div style="display:none;font-size:1px;line-height:1px;max-height:0;max-width:0;opacity:0;overflow:hidden;mso-hide:all;">{preheader}</div>
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" bgcolor="{outer}"><tr><td align="center" class="outer" style="padding:32px 16px;">
<table role="presentation" width="700" cellpadding="0" cellspacing="0" bgcolor="{paper}" style="width:100%;max-width:700px;background:{paper};border:1px solid {line};border-radius:12px;overflow:hidden;">
<tr><td height="6" style="background: linear-gradient(to right, #a855f7, #38bdf8);"></td></tr>
<tr><td class="content" bgcolor="{header}" style="padding:24px 40px;background:{header};border-bottom:1px solid {line};"><table role="presentation" width="100%" cellpadding="0" cellspacing="0"><tr><td style="vertical-align:middle;"><a href="https://mitchdog.com" style="text-decoration:none;"><img src="https://mitchdog.com/favicon.ico" width="24" height="24" style="vertical-align:middle;margin-right:10px;border-radius:4px;" alt="mitch.pro"><span style="font:800 24px/1 Arial,Helvetica,sans-serif;letter-spacing:-0.02em;color:#ffffff;vertical-align:middle;">mitch.pro</span><span style="font:300 24px/1 Arial,Helvetica,sans-serif;color:#64748b;vertical-align:middle;margin:0 8px;">/</span><span style="font:800 24px/1 Arial,Helvetica,sans-serif;letter-spacing:-0.02em;color:#38bdf8;vertical-align:middle;">mitchdog.com</span></a></td><td align="right" class="masthead-credit" style="font:500 13px/1.4 Arial,Helvetica,sans-serif;color:#cbd5e1;">mitch.pro</td></tr></table></td></tr>
<tr><td class="content" style="padding:32px 40px 18px;">{edition}<h1 class="headline" style="margin:0;font:750 42px/1.12 Arial,Helvetica,sans-serif;letter-spacing:-1.6px;color:{ink};">{title}</h1></td></tr>
<tr><td class="content" style="padding:0 40px 14px;font:16px/1.8 Arial,Helvetica,sans-serif;color:{muted};">{intro}</td></tr>
{hero}{sections}{button}
<tr><td class="content" style="padding:0 40px 32px;"><table role="presentation" cellpadding="0" cellspacing="0"><tr><td width="48" valign="middle"><table role="presentation" cellpadding="0" cellspacing="0"><tr>{avatar_cell}</tr></table></td><td style="padding-left:6px;"><p style="margin:0 0 3px;font:700 16px/1.4 Arial,Helvetica,sans-serif;color:{ink};">{signature}</p>{signoff_sub}</td></tr></table></td></tr>
<tr><td class="content" style="padding:22px 40px;border-top:1px solid {line};font:11px/1.9 Arial,Helvetica,sans-serif;color:{muted};text-align:center;">
Delivered by <a href="https://mitchdog.com" style="color:{muted};text-decoration:underline;font-weight:600;">mitchdog.com</a> | <a href="https://mitch.pro" style="color:{muted};text-decoration:underline;">mitch.pro</a><br>
To opt-out of these communications, you can <a href="{unsubscribe}" style="color:{muted};text-decoration:underline;">unsubscribe from this list</a>.<br>
For support: email <a href="mailto:support@mitch.pro" style="color:{muted};text-decoration:none;">support@mitch.pro</a><br>
2014 Capitol Ave #100, Sacramento, CA 95811
</td></tr></table></td></tr></table></body></html>"##
    )
}
