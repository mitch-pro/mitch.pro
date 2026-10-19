use super::Draft;

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
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
    let edition = if draft.edition.trim().is_empty() {
        String::new()
    } else {
        format!("<p style=\"margin:0 0 14px;font:600 13px/1.5 Arial,Helvetica,sans-serif;color:{muted};\">{}</p>", escape(&draft.edition))
    };
    let hero = if draft.hero_url.is_empty() {
        String::new()
    } else {
        format!("<tr><td class=\"content\" style=\"padding:0 40px 28px;\"><img src=\"{}\" width=\"620\" alt=\"\" style=\"display:block;width:100%;max-width:620px;height:auto;border:0;border-radius:8px;\"></td></tr>", escape(&draft.hero_url))
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
<tr><td class="content" bgcolor="{header}" style="padding:24px 40px;background:{header};border-bottom:4px solid {accent};"><table role="presentation" width="100%" cellpadding="0" cellspacing="0"><tr><td><a href="https://mitchdog.com" style="font:800 24px/1 Arial,Helvetica,sans-serif;letter-spacing:-1px;color:#ffffff;text-decoration:none;">mitch.pro</a></td><td align="right" class="masthead-credit" style="font:500 13px/1.4 Arial,Helvetica,sans-serif;color:#cbd5e1;">Mitch &amp; Tyler</td></tr></table></td></tr>
<tr><td class="content" style="padding:32px 40px 18px;">{edition}<h1 class="headline" style="margin:0;font:750 42px/1.12 Arial,Helvetica,sans-serif;letter-spacing:-1.6px;color:{ink};">{title}</h1></td></tr>
<tr><td class="content" style="padding:0 40px 14px;font:16px/1.8 Arial,Helvetica,sans-serif;color:{muted};">{intro}</td></tr>
{hero}{sections}{button}
<tr><td class="content" style="padding:0 40px 32px;"><table role="presentation" cellpadding="0" cellspacing="0"><tr><td width="44" valign="middle"><table role="presentation" cellpadding="0" cellspacing="0"><tr><td width="32" height="32" align="center" bgcolor="{header}" style="background:{header};border-radius:50%;font:700 12px Arial,Helvetica,sans-serif;color:#ffffff;">M</td></tr></table></td><td width="44" valign="middle"><table role="presentation" cellpadding="0" cellspacing="0"><tr><td width="32" height="32" align="center" bgcolor="{accent}" style="background:{accent};border-radius:50%;font:700 12px Arial,Helvetica,sans-serif;color:{accent_ink};">T</td></tr></table></td><td style="padding-left:4px;"><p style="margin:0 0 3px;font:700 16px/1.4 Arial,Helvetica,sans-serif;color:{ink};">{signature}</p><p style="margin:0;font:12px/1.4 Arial,Helvetica,sans-serif;color:{muted};">mitch.pro</p></td></tr></table></td></tr>
<tr><td class="content" style="padding:22px 40px;border-top:1px solid {line};font:11px/1.9 Arial,Helvetica,sans-serif;color:{muted};">
You’re receiving a site update from mitch.pro.<br><a href="https://mitchdog.com" style="color:{muted};">Visit the site</a> &nbsp;·&nbsp; <a href="mailto:support@mitch.pro" style="color:{muted};">Contact us</a> &nbsp;·&nbsp; <a href="{unsubscribe}" style="color:{muted};">Unsubscribe</a><br>2014 Capitol Ave #100, Sacramento, CA 95811
</td></tr></table></td></tr></table></body></html>"##
    )
}
