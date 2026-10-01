//! HTML injection pipeline — byte-faithful ports of `injectSharedHead`,
//! `recaptchaLoaderStr`, `injectReadability`, `injectBroadcast`,
//! `stripBroadcast`, and the main page-injection block (server.js
//! ~20788-21313).

/// `injectSharedHead(html)`.
pub fn inject_shared_head(html: &str) -> String {
    let mut html = html.to_string();
    if !html.contains("/popup.js") {
        let block = "<link rel=\"preconnect\" href=\"https://fonts.googleapis.com\">\n<link rel=\"preconnect\" href=\"https://fonts.gstatic.com\" crossorigin>\n<script src=\"/popup.js?v=3\"></script>\n<script src=\"/pwa-install.js\" defer></script>\n<link rel=\"manifest\" href=\"/manifest.json\">\n<meta name=\"theme-color\" content=\"#05070d\">\n<meta name=\"mobile-web-app-capable\" content=\"yes\">\n<meta name=\"apple-mobile-web-app-capable\" content=\"yes\">\n<meta name=\"apple-mobile-web-app-status-bar-style\" content=\"black-translucent\">\n<link rel=\"apple-touch-icon\" sizes=\"180x180\" href=\"/apple-touch-icon.png\">\n</head>";
        html = html.replacen("</head>", block, 1);
    }
    if html.contains("name=\"viewport\"") {
        if !html.contains("viewport-fit") {
            html = replace_first_viewport(&html, ", viewport-fit=cover");
        }
        if !html.contains("interactive-widget") {
            html = replace_first_viewport(&html, ", interactive-widget=resizes-content");
        }
    }
    html
}

/// JS regex: `/(<meta[^>]*name="viewport"[^>]*content=")([^"]*)("[^>]*>)/i`
/// with the captured content extended. Replaces the FIRST match.
pub fn replace_first_viewport(html: &str, addition: &str) -> String {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    #[allow(clippy::expect_used)]
    let re = RE.get_or_init(|| {
        regex::Regex::new("(?i)(<meta[^>]*name=\"viewport\"[^>]*content=\")([^\"]*)(\"[^>]*>)")
            .expect("static regex")
    });
    re.replace(html, |caps: &regex::Captures| {
        format!("{}{}{}{}", &caps[1], &caps[2], addition, &caps[3])
    })
    .into_owned()
}

/// `recaptchaLoaderStr(recaptchaHost, rcKey)` — exact JS string.
pub fn recaptcha_loader_str(recaptcha_host: &str, rc_key: &str) -> String {
    format!(
        "<script>\n  window.getCaptchaToken = function(action) {{\n    return new Promise(function(resolve) {{\n      function load() {{\n        if (window._mitchRcLoading) return;\n        window._mitchRcLoading = true;\n        var s = document.createElement('script');\n        s.src = 'https://{recaptcha_host}/recaptcha/api.js?render={rc_key}';\n        s.async = true;\n        document.head.appendChild(s);\n      }}\n      let attempts = 0;\n      function checkAndExecute() {{\n        if (window.grecaptcha && window.grecaptcha.ready) {{\n          grecaptcha.ready(function() {{\n            grecaptcha.execute('{rc_key}', {{action: action || 'page_view'}}).then(function(token) {{\n              resolve(token);\n            }}).catch(function() {{\n              resolve(null);\n            }});\n          }});\n        }} else {{\n          load();\n          attempts++;\n          if (attempts < 100) {{\n            setTimeout(checkAndExecute, 100);\n          }} else {{\n            resolve(null);\n          }}\n        }}\n      }}\n      checkAndExecute();\n    }});\n  }};\n</script>\n"
    )
}

/// readability.css's rules (line-height, min tap targets, focus rings) are
/// now part of app.css's consolidated stylesheet, so this no longer injects
/// a separate <link> — kept as a passthrough rather than removed outright
/// since both call sites (the /team route, and every static HTML serve)
/// still call it unconditionally.
pub fn inject_readability(html: &str, _url_path: &str) -> String {
    html.to_string()
}

/// `injectBroadcast(html)`.
pub fn inject_broadcast(html: &str) -> String {
    if html.contains("/broadcast.js") {
        return html.to_string();
    }
    let tag = "<script src=\"/broadcast.js?v=4\" defer></script>";
    match html.rfind("</body>") {
        Some(i) => format!("{}{}{}", &html[..i], tag, &html[i..]),
        None => format!("{html}{tag}"),
    }
}

/// `stripBroadcast(html)` — regex from the JS, case-insensitive, global.
pub fn strip_broadcast(html: &str) -> String {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    #[allow(clippy::expect_used)]
    let re = RE.get_or_init(|| {
        regex::Regex::new(
            r#"(?i)<script\b[^>]*\bsrc=["']/broadcast\.js(?:\?[^"']*)?["'][^>]*>\s*</script>"#,
        )
        .expect("static regex")
    });
    re.replace_all(html, "").into_owned()
}

/// True when a path is an embedded game runtime (broadcast/injections skip).
pub fn is_embedded_game_runtime(path: &str) -> bool {
    path.starts_with("/games/") && path != "/games/" && path != "/games/index.html"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_head_injects_before_head_close_once() {
        let html = "<html><head><title>x</title></head><body></body></html>";
        let out = inject_shared_head(html);
        assert!(out.contains("<script src=\"/popup.js?v=3\"></script>"));
        assert!(!out.contains("</head>\n<meta"));
        assert!(out.matches("popup.js").count() == 1);
        // Idempotent: second call is a no-op.
        assert_eq!(out, inject_shared_head(&out));
    }

    #[test]
    fn viewport_additions_match_js_order() {
        let html = "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">";
        let out = inject_shared_head(html);
        assert!(out.contains("width=device-width, initial-scale=1, viewport-fit=cover, interactive-widget=resizes-content"), "{out}");
        // The meta tag must stay intact (closing quote + > preserved).
        assert!(out.ends_with("\">"), "closing group preserved: {out}");
        assert!(
            out.matches('<').count() == 1,
            "no duplicated meta tag: {out}"
        );
    }

    #[test]
    fn broadcast_strip_matches_js() {
        let html = "a<script src=\"/broadcast.js?v=4\" defer></script>b<script SRC='/broadcast.js' ></script>c";
        assert_eq!(strip_broadcast(html), "abc");
    }

    #[test]
    fn readability_is_a_passthrough_now_that_its_merged_into_app_css() {
        let html = "<html><head></head></html>";
        assert_eq!(inject_readability(html, "/"), html);
        assert_eq!(inject_readability(html, "/games/chess/"), html);
    }
}
