//! The main HTML injection pipeline — byte-faithful port of server.js's
//! injection block (~21176-21313) and serveStatic's HTML transform.

use crate::hosts::{is_host, request_host, SiteConfig};
use crate::inject::{
    inject_readability, is_embedded_game_runtime, recaptcha_loader_str, strip_broadcast,
};
use crate::state::AppState;
use std::sync::Arc;

/// The main injection pipeline (runs for .html / `/` pages).
/// `html` is the raw file contents; `path`/`html_base` come from the request.
/// Session-dependent pieces are stubbed unauthenticated (Step 6).
pub fn inject_page(
    state: &Arc<AppState>,
    headers: &axum::http::HeaderMap,
    path: &str,
    _html_base: &str,
    html: &mut String,
    is_sales_page: bool,
) -> String {
    let cfg: &SiteConfig = &state.cfg;
    let mut raw = std::mem::take(html);
    let is_onboarding = raw.contains("data-presentation=\"onboarding\"");

    let mut inject_str = String::new();
    let rc_key = std::env::var("RECAPTCHA_SITE_KEY")
        .unwrap_or_default()
        .trim()
        .to_string();
    let has_v2_script = raw.contains("recaptcha/api.js");
    let req_host = request_host(headers)
        .split(':')
        .next()
        .unwrap_or("")
        .to_lowercase();
    let primary_host = url::Url::parse(&cfg.primary)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_lowercase()))
        .unwrap_or_else(|| "mitch.pro".to_string());
    let is_primary_host = req_host.is_empty()
        || req_host == primary_host
        || req_host == "localhost"
        || req_host == "127.0.0.1";
    let recaptcha_host = std::env::var("RECAPTCHA_SCRIPT_HOST")
        .unwrap_or_else(|_| {
            if is_primary_host {
                "www.google.com".into()
            } else {
                "www.recaptcha.net".into()
            }
        })
        .trim()
        .to_string();
    let is_unsubscribe_page =
        path == "/unsubscribe" || path == "/unsubscribe/" || path == "/unsubscribe/index.html";
    let load_recaptcha = !rc_key.is_empty() && !has_v2_script && !is_unsubscribe_page;

    let is_standalone_game_portal =
        path == "/game-portal" || path == "/game-portal/" || path == "/game-portal/index.html";
    let is_embedded = is_standalone_game_portal
        || (path.starts_with("/games/") && path != "/games/" && path != "/games/index.html");

    let node_env_test = std::env::var("NODE_ENV").unwrap_or_default() == "test";
    let cookie_header = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let cookies = mitch_lib::auth::get_cookies_from_header_value(
        cookie_header,
        &state.store,
        &state.id_secret,
        node_env_test,
    );
    let page_sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("");
    let is_authenticated_html = !page_sid.is_empty()
        && mitch_lib::auth::valid_id(page_sid, &state.id_secret)
        && !state.is_revoked_id(page_sid)
        && state.check_password_cookie(headers, Some(page_sid));

    let is_rjuhsd = crate::hosts::is_rjuhsd_host(headers);
    let is_pickle = crate::hosts::is_pickle_host(headers);

    if !is_onboarding
        && !is_sales_page
        && !is_rjuhsd
        && !is_pickle
        && (!is_embedded || is_standalone_game_portal)
    {
        inject_str.push_str("<link rel=\"stylesheet\" href=\"/community-refresh.css?v=4\">\n");
        if !is_authenticated_html {
            inject_str.push_str("<script src=\"/guest-preview.js?v=1\" defer></script>\n");
        }
    }

    if is_authenticated_html && !is_embedded && !raw.contains("/broadcast.js") {
        inject_str.push_str("<script src=\"/broadcast.js?v=9\" defer></script>\n");
    } else if !is_authenticated_html && raw.contains("/broadcast.js") {
        raw = strip_broadcast(&raw);
    }

    // Enhancement CSS layers (async media=print swap).
    if !is_onboarding && !is_embedded && !raw.contains("/relaunch.css") {
        inject_str.push_str("<link rel=\"stylesheet\" href=\"/relaunch.css\" media=\"print\" onload=\"this.media='all'\"><noscript><link rel=\"stylesheet\" href=\"/relaunch.css\"></noscript>\n");
    }
    if !is_onboarding && !is_embedded && !raw.contains("/site-galaxy.css") {
        inject_str.push_str("<link rel=\"stylesheet\" href=\"/site-galaxy.css\" media=\"print\" onload=\"this.media='all'\"><noscript><link rel=\"stylesheet\" href=\"/site-galaxy.css\"></noscript>\n");
    }
    if !is_onboarding && !is_embedded && !raw.contains("/portal-redesign.css") {
        inject_str.push_str("<link rel=\"stylesheet\" href=\"/portal-redesign.css?v=16\" media=\"print\" onload=\"this.media='all'\"><noscript><link rel=\"stylesheet\" href=\"/portal-redesign.css?v=16\"></noscript>\n");
    }
    if !is_embedded && !raw.contains("/app-shell.js") {
        inject_str.push_str("<script src=\"/app-shell.js\" defer></script>\n");
    }
    if !is_embedded
        && (path == "/encrypt" || path == "/encrypt/" || path == "/encrypt/index.html")
        && !raw.contains("/encrypt-galaxy.css")
    {
        inject_str.push_str("<link rel=\"stylesheet\" href=\"/encrypt-galaxy.css\">\n");
    }
    if !is_embedded {
        if !raw.contains("fonts.googleapis.com") {
            inject_str.push_str("<link rel=\"preconnect\" href=\"https://fonts.googleapis.com\">\n<link rel=\"preconnect\" href=\"https://fonts.gstatic.com\" crossorigin>\n");
        }
        if !raw.contains("/popup.js") {
            inject_str.push_str("<script src=\"/popup.js?v=3\" defer></script>\n");
        }
        if !raw.contains("/pwa-install.js") {
            inject_str.push_str("<script src=\"/pwa-install.js\" defer></script>\n");
        }
        if !raw.contains("name=\"viewport\"") {
            inject_str.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1, viewport-fit=cover, interactive-widget=resizes-content\">\n");
        } else {
            let mut changed = false;
            if !raw.contains("viewport-fit") {
                raw = crate::inject::replace_first_viewport(&raw, ", viewport-fit=cover");
                changed = true;
            }
            if !raw.contains("interactive-widget") {
                raw = crate::inject::replace_first_viewport(
                    &raw,
                    ", interactive-widget=resizes-content",
                );
                changed = true;
            }
            let _ = changed;
        }
        if !raw.contains("rel=\"manifest\"") {
            inject_str.push_str("<link rel=\"manifest\" href=\"/manifest.json\">\n");
        }
        if !raw.contains("rel=\"apple-touch-icon\"") {
            inject_str.push_str("<link rel=\"apple-touch-icon\" sizes=\"180x180\" href=\"/apple-touch-icon.png\">\n");
        }
        if !raw.contains("name=\"theme-color\"") {
            inject_str.push_str("<meta name=\"theme-color\" content=\"#05070d\">\n");
        }
        if !raw.contains("name=\"mobile-web-app-capable\"") {
            inject_str.push_str("<meta name=\"mobile-web-app-capable\" content=\"yes\">\n");
        }
        if !raw.contains("name=\"apple-mobile-web-app-capable\"") {
            inject_str.push_str("<meta name=\"apple-mobile-web-app-capable\" content=\"yes\">\n");
        }
        if !raw.contains("name=\"apple-mobile-web-app-status-bar-style\"") {
            inject_str.push_str("<meta name=\"apple-mobile-web-app-status-bar-style\" content=\"black-translucent\">\n");
        }
        if !raw.contains("name=\"apple-mobile-web-app-title\"") {
            inject_str
                .push_str("<meta name=\"apple-mobile-web-app-title\" content=\"mitch.pro\">\n");
        }
    }

    let hide_vm = std::env::var("HIDE_VM_FEATURES").unwrap_or_default() == "1"
        || std::env::var("DISABLE_VM_FEATURES").unwrap_or_default() == "1"
        || std::env::var("DISABLE_VM_FEATURES").unwrap_or_default() == "true";
    if hide_vm && !is_embedded {
        inject_str.push_str("<style>a[href=\"/vms/\"], .site-advert-banner, #vm-workspace-panel, #cloud-vms { display: none !important; }</style>\n");
    }

    if load_recaptcha {
        inject_str.push_str("\n<!-- mitch.pro: reCAPTCHA Loader -->\n");
        inject_str.push_str(&recaptcha_loader_str(&recaptcha_host, &rc_key));
        inject_str
            .push_str("<style>.grecaptcha-badge { visibility: hidden !important; }</style>\n");
    }

    if !inject_str.is_empty() {
        if let Some(idx) = raw.find("</head>") {
            // JS: concat([raw.slice(0, idx), injectBuf, raw.slice(idx)]) — the
            // insertion is BEFORE the existing </head>, which is kept.
            raw = format!("{}{}{}", &raw[..idx], inject_str, &raw[idx..]);
        } else if let Some(idx) = raw.find("<body") {
            if let Some(end) = raw[idx..].find('>') {
                let end = idx + end + 1;
                raw = format!("{}{}{}", &raw[..end], inject_str, &raw[end..]);
            } else {
                raw = format!("{inject_str}{raw}");
            }
        } else {
            raw = format!("{inject_str}{raw}");
        }
    }

    // Agree footer: before the LAST </body>, with the encrypt-app exception.
    let req_host_full = request_host(headers);
    let req_host_name = req_host_full.split(':').next().unwrap_or("");
    let is_rjuhsd = is_host(req_host_name, "rjuhsd.school");
    let is_encrypt_app_page =
        path == "/encrypt" || path == "/encrypt/" || path == "/encrypt/index.html";
    if !is_onboarding && !raw.contains("_agree_footer") && !(is_encrypt_app_page && !is_rjuhsd) {
        let agree = "<div id=\"_agree_footer\" style=\"position:fixed;bottom:5px;left:0;right:0;text-align:center;pointer-events:none;z-index:2147483647;font-size:.65rem;color:rgba(255,255,255,.15);font-family:system-ui,sans-serif;letter-spacing:.01em;\">By using mitch.pro you agree to the <a href=\"/use-agreement.html\" style=\"color:rgba(255,255,255,.15);pointer-events:all;\" target=\"_blank\">use agreement</a> and <a href=\"/privacy.html\" style=\"color:rgba(255,255,255,.15);pointer-events:all;\" target=\"_blank\">privacy policy</a>.</div>";
        match raw.rfind("</body>") {
            Some(i) => raw = format!("{}{}{}", &raw[..i], agree, &raw[i..]),
            None => raw = format!("{raw}{agree}"),
        }
    }
    *html = raw.clone();
    raw
}

/// serveStatic's HTML transform: readability + broadcast handling.
pub fn serve_static_html(
    state: &Arc<AppState>,
    headers: &axum::http::HeaderMap,
    path: &str,
    html: String,
) -> String {
    let mut html = if html.contains("data-presentation=\"onboarding\"") {
        html
    } else {
        inject_readability(&html, path)
    };
    let is_authenticated = state.check_password_cookie(headers, None);
    let is_embedded = is_embedded_game_runtime(path);
    if is_authenticated && !is_embedded {
        html = crate::inject::inject_broadcast(&html);
    } else {
        html = strip_broadcast(&html);
    }
    html
}
