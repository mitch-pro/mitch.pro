//! HTML injection pipeline — byte-faithful ports of `injectSharedHead`,
//! `recaptchaLoaderStr`, `injectReadability`, `injectBroadcast`,
//! `stripBroadcast`, and the main page-injection block (server.js
//! ~20788-21313).

/// Inject before upstream scripts so presence retains the site's fetch transport.
#[allow(clippy::expect_used)]
pub fn inject_presence(html: &str, page: Option<&str>) -> String {
    if html.contains("/presence.js") {
        return html.to_string();
    }
    let context = page
        .map(|page| {
            format!(
                " data-presence-page=\"{}\"",
                page.replace('&', "&amp;")
                    .replace('"', "&quot;")
                    .replace('<', "&lt;")
            )
        })
        .unwrap_or_default();
    let tag = format!("<script src=\"/presence.js?v=1\"{context}></script>");
    static HEAD: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let head = HEAD.get_or_init(|| regex::Regex::new(r"(?i)<head\b[^>]*>").expect("static regex"));
    if let Some(found) = head.find(html) {
        format!("{}{}{}", &html[..found.end()], tag, &html[found.end()..])
    } else {
        format!("{tag}{html}")
    }
}

/// `injectSharedHead(html)`.
pub fn inject_shared_head(html: &str) -> String {
    let mut html = html.to_string();
    if !html.contains("/popup.js") {
        let block = "<link rel=\"preconnect\" href=\"https://fonts.googleapis.com\">\n<link rel=\"preconnect\" href=\"https://fonts.gstatic.com\" crossorigin>\n<script src=\"/popup.js?v=3\"></script>\n<script src=\"/pwa-install.js\" defer></script>\n<link rel=\"manifest\" href=\"/manifest.json\">\n<meta name=\"theme-color\" content=\"#05070d\">\n<meta name=\"mobile-web-app-capable\" content=\"yes\">\n<meta name=\"apple-mobile-web-app-capable\" content=\"yes\">\n<meta name=\"apple-mobile-web-app-status-bar-style\" content=\"black-translucent\">\n<link rel=\"apple-touch-icon\" sizes=\"180x180\" href=\"/apple-touch-icon.png\">\n</head>";
        html = html.replacen("</head>", block, 1);
    }
    // Independent of the popup.js guard above: a page that already
    // hardcodes popup.js in its own markup (e.g. the homepage) would
    // otherwise short-circuit this whole block and silently drop the
    // companion widget on exactly those pages.
    if !html.contains("/companion-widget.js") {
        let block = "<script src=\"/companion-widget.js?v=1\" defer></script>\n<link rel=\"stylesheet\" href=\"/companion-widget.css\">\n</head>";
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
    let tag = "<script src=\"/broadcast.js?v=11\" defer></script>";
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

pub fn inject_access_guard(html: &str) -> String {
    if html.contains("/ban-guard.js") {
        return html.to_string();
    }
    match html.rfind("</body>") {
        Some(index) => format!(
            "{}<script src=\"/ban-guard.js\" defer></script>{}",
            &html[..index],
            &html[index..]
        ),
        None => html.to_string(),
    }
}

pub fn inject_credit(html: &str, school: bool) -> String {
    let guarded = inject_access_guard(html);
    let html = guarded.as_str();
    if html.contains("id=\"site-credit\"") {
        return html.to_string();
    }
    let text = if school {
        "Made by Tyler and Mitch."
    } else {
        "Created by Mitch and Tyler"
    };
    let band = format!(
        r#"<style>
:root {{ --site-credit-height: calc(29px + env(safe-area-inset-bottom, 0px)); }}
#site-credit-space {{ clear:both; height:var(--site-credit-height); flex:0 0 var(--site-credit-height); grid-column:1 / -1; pointer-events:none; }}
#site-credit {{ box-sizing:border-box; position:fixed; inset:auto 0 0; z-index:100000; width:100%; height:var(--site-credit-height); margin:0; padding:6px 16px calc(6px + env(safe-area-inset-bottom, 0px)); border-top:1px solid #8883; background:#88888818; backdrop-filter:blur(8px); -webkit-backdrop-filter:blur(8px); color:inherit; text-align:center; font:400 11px/16px system-ui,sans-serif; pointer-events:none; }}
#site-credit span {{ opacity:.7; }}
#site-credit {{ display:grid; grid-template-columns:minmax(0,1fr) auto minmax(0,1fr); align-items:center; gap:8px; }}
#site-credit-copy {{ grid-column:2; white-space:nowrap; }}
#site-active {{ grid-column:3; justify-self:end; display:inline-flex; align-items:center; gap:5px; white-space:nowrap; font-variant-numeric:tabular-nums; }}
#site-active[hidden] {{ display:none; }}
#site-active::before {{ content:""; width:5px; height:5px; border-radius:50%; background:#78ad8b; flex:none; }}
@media (max-width:380px) {{ #site-credit {{ padding-inline:10px; gap:5px; font-size:10px; }} }}
body #mobile-dock {{ bottom:calc(var(--site-credit-height) + 10px); }}
body .mobile-nav-dock {{ bottom:var(--site-credit-height); padding-bottom:6px; }}
@media print {{ #site-credit {{ position:static; height:auto; padding:6px 16px; }} #site-credit-space {{ display:none; }} }}
</style><div id="site-credit-space" aria-hidden="true"></div><div id="site-credit"><span id="site-credit-copy">{text}</span><span id="site-active" hidden title="Members online now"></span></div><script src="/site-status.js" defer></script>"#
    );
    match html.rfind("</body>") {
        Some(i) => format!("{}{}{}", &html[..i], band, &html[i..]),
        None => html.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_precedes_upstream_transports_and_preserves_legacy_page_context() {
        let html = "<HTML><HEAD lang=\"en\"><script>window.fetch=proxyFetch</script></HEAD></HTML>";
        let injected = inject_presence(html, Some("/games/chess/?name=\"a\"&mode=1"));
        assert!(injected.find("/presence.js").unwrap() < injected.find("window.fetch").unwrap());
        assert!(
            injected.contains("data-presence-page=\"/games/chess/?name=&quot;a&quot;&amp;mode=1\"")
        );
        assert_eq!(inject_presence(&injected, None), injected);
        assert!(inject_presence("<body>VM desktop</body>", None)
            .starts_with("<script src=\"/presence.js"));
    }

    #[test]
    fn credits_are_idempotent_and_use_school_wording() {
        let html = "<html><body><main>Sign up</main></body></html>";
        let main = inject_credit(html, false);
        assert!(main.contains("Created by Mitch and Tyler"));
        assert_eq!(main, inject_credit(&main, false));
        let school = inject_credit(html, true);
        assert!(school.contains("Made by Tyler and Mitch."));
        assert!(!school.contains("Created by"));
        assert!(school.find("site-credit").unwrap() < school.find("</body>").unwrap());
    }

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
        let html = "a<script src=\"/broadcast.js?v=11\" defer></script>b<script SRC='/broadcast.js' ></script>c";
        assert_eq!(strip_broadcast(html), "abc");
    }

    #[test]
    fn readability_is_a_passthrough_now_that_its_merged_into_app_css() {
        let html = "<html><head></head></html>";
        assert_eq!(inject_readability(html, "/"), html);
        assert_eq!(inject_readability(html, "/games/chess/"), html);
    }
}
