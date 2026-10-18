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
                "<p style=\"margin:0 0 18px;\">{}</p>",
                escape(p).replace('\n', "<br>")
            )
        })
        .collect()
}

pub(super) fn render(draft: &Draft, unsubscribe: &str) -> String {
    let (outer, card, ink, muted, line) = if draft.theme == "paper" {
        ("#e9e7e1", "#fffdf8", "#202720", "#656d64", "#dedfd5")
    } else {
        ("#080d13", "#131d27", "#f3f2e9", "#a8b4be", "#303d47")
    };
    let accent = &draft.accent;
    let subject = escape(&draft.subject);
    let title = escape(&draft.title);
    let preheader = escape(&draft.preheader);
    let intro = paragraphs(&draft.intro);
    let signature = escape(&draft.signature);
    let date = chrono::Utc::now().format("%B %Y").to_string();
    let hero = if draft.hero_url.is_empty() {
        String::new()
    } else {
        format!("<tr><td style=\"padding:0 40px 32px;\" class=\"content\"><img src=\"{}\" width=\"580\" alt=\"\" style=\"display:block;width:100%;max-width:580px;height:auto;border-radius:12px;\"></td></tr>", escape(&draft.hero_url))
    };
    let sections: String = draft.sections.iter().enumerate().filter(|(_,s)| !s.title.trim().is_empty() || !s.body.trim().is_empty()).map(|(i,s)| {
        format!(r#"<tr><td class="content" style="padding:0 40px;"><table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="border-top:1px solid {line};"><tr><td width="38" valign="top" style="padding-top:27px;color:{accent};font:600 12px Arial,sans-serif;">{:02}</td><td style="padding:24px 0 10px;"><h2 style="margin:0 0 12px;font:600 23px/1.25 Georgia,serif;letter-spacing:-.5px;color:{ink};">{}</h2><div style="font:15px/1.75 Arial,sans-serif;color:{muted};">{}</div></td></tr></table></td></tr>"#, i+1,escape(&s.title),paragraphs(&s.body))
    }).collect();
    let button = if draft.button_label.trim().is_empty() || draft.button_url.is_empty() {
        String::new()
    } else {
        let r = u8::from_str_radix(&accent[1..3], 16).unwrap_or(0) as u32;
        let g = u8::from_str_radix(&accent[3..5], 16).unwrap_or(0) as u32;
        let b = u8::from_str_radix(&accent[5..7], 16).unwrap_or(0) as u32;
        let button_ink = if r * 299 + g * 587 + b * 114 > 140_000 {
            "#111b17"
        } else {
            "#ffffff"
        };
        format!(
            r#"<tr><td class="content" style="padding:8px 40px 36px;"><table role="presentation" cellpadding="0" cellspacing="0"><tr><td bgcolor="{accent}" style="background:{accent};border-radius:8px;"><a href="{}" style="display:inline-block;padding:16px 24px;color:{button_ink};font:700 14px Arial,sans-serif;text-decoration:none;">{}</a></td></tr></table></td></tr>"#,
            escape(&draft.button_url),
            escape(&draft.button_label)
        )
    };
    let unsubscribe = escape(unsubscribe);
    format!(
        r##"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{subject}</title>
<style>@media(max-width:600px){{.outer{{padding:12px 8px!important}}.content{{padding-left:24px!important;padding-right:24px!important}}.headline{{font-size:38px!important}}}}</style></head>
<body style="margin:0;background:{outer};color:{ink};-webkit-text-size-adjust:100%;">
<div style="display:none;font-size:1px;line-height:1px;max-height:0;max-width:0;opacity:0;overflow:hidden;mso-hide:all;">{preheader}</div>
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" bgcolor="{outer}"><tr><td align="center" class="outer" style="padding:36px 16px;">
<table role="presentation" width="660" cellpadding="0" cellspacing="0" bgcolor="{card}" style="width:100%;max-width:660px;background:{card};border:1px solid {line};border-radius:16px;overflow:hidden;">
<tr><td style="height:5px;background:{accent};font-size:0;line-height:0;">&nbsp;</td></tr>
<tr><td class="content" style="padding:32px 40px 28px;"><table role="presentation" width="100%" cellpadding="0" cellspacing="0"><tr><td><a href="https://mitchdog.com" style="font:800 23px Arial,sans-serif;letter-spacing:-1px;color:{ink};text-decoration:none;">mitch<span style="color:{accent};">.</span>pro</a></td><td align="right" style="font:12px Arial,sans-serif;color:{muted};">{date}</td></tr></table></td></tr>
<tr><td class="content" style="padding:0 40px 18px;"><h1 class="headline" style="margin:0;font:500 52px/1.08 Georgia,serif;letter-spacing:-2px;color:{ink};">{title}</h1></td></tr>
<tr><td class="content" style="padding:4px 40px 18px;font:16px/1.8 Arial,sans-serif;color:{muted};">{intro}</td></tr>
{hero}{sections}{button}
<tr><td class="content" style="padding:0 40px 34px;font:italic 21px Georgia,serif;color:{ink};">{signature}</td></tr>
<tr><td class="content" style="padding:25px 40px;border-top:1px solid {line};font:12px/1.8 Arial,sans-serif;color:{muted};">
<strong style="color:{ink};">mitch.pro</strong> &nbsp;·&nbsp; <a href="https://mitchdog.com" style="color:{muted};">mitchdog.com</a><br>
You’re receiving a site update from mitch.pro. <a href="{unsubscribe}" style="color:{muted};">Unsubscribe</a><br>
<a href="mailto:support@mitch.pro" style="color:{muted};">support@mitch.pro</a><br>2014 Capitol Ave #100, Sacramento, CA 95811
</td></tr></table></td></tr></table></body></html>"##
    )
}
