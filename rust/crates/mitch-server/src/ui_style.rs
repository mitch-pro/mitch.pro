//! Site styles (mitch.pro or Tylerium): which one a page renders in, and the
//! account API that saves the choice. See `docs/ui-styles.md`.
//!
//! - `GET  /api/me/ui-style`: `{style, chosen}` for the signed-in account.
//! - `POST /api/me/ui-style`: `{style: "mitch" | "tylerium"}`.
//!
//! Rendering rules ([`for_page`]): the account's saved choice applies to
//! signed-in pages on the main site. rjuhsd.school, the pickle host, and the
//! public pages in [`MITCH_ONLY_SECTIONS`] (landing, sign-in, legal) always
//! render in mitch.pro. Accounts that never chose render in mitch.pro and get
//! the chooser (`/ui-style.js` with `data-ask="1"`). Tylerium pages are marked
//! with `html[data-ui="tylerium"]`, and the pipeline links its stylesheets,
//! the cloud background, and the Tylerium variants of a few pages from
//! `webserver/tylerium/` ([`overlay_path`]).

use crate::routes::me::{cookies_of, json_response, me_uid};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::auth;
use mitch_lib::ui_style::{self, UiStyle};
use serde_json::{json, Value};
use std::sync::Arc;

/// First path segments of pages that always use mitch.pro, whatever the
/// account chose: the signed-out landing page, sign-in and recovery, legal
/// pages, and the school site's own section.
pub const MITCH_ONLY_SECTIONS: &[&str] = &[
    "index-sales",
    "index-sales.html",
    "enroll",
    "claim",
    "password",
    "magic-link",
    "appeal",
    "support",
    "unsubscribe",
    "privacy",
    "use-agreement",
    "cookies",
    "copyright",
    "rjuhsd",
];

/// How one page renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PageStyle {
    pub style: UiStyle,
    /// Signed in and never chose: show the chooser on this page.
    pub ask: bool,
}

impl PageStyle {
    pub fn is_tylerium(self) -> bool {
        self.style == UiStyle::Tylerium
    }
}

/// Whether `path` is a page that always renders in mitch.pro.
pub fn is_mitch_only_path(path: &str) -> bool {
    let first = path.trim_start_matches('/').split('/').next().unwrap_or("");
    MITCH_ONLY_SECTIONS.contains(&first)
}

/// The normalized email of a fully signed-in request (valid, unrevoked sid
/// with a matching password cookie), the same test the HTML pipeline uses.
fn signed_in_email(state: &Arc<AppState>, headers: &HeaderMap) -> Option<String> {
    let sid = me_uid(&cookies_of(state, headers));
    if sid.is_empty()
        || !auth::valid_id(&sid, &state.id_secret)
        || state.is_revoked_id(&sid)
        || !state.check_password_cookie(headers, Some(&sid))
    {
        return None;
    }
    auth::email_from_sid(&state.store, &state.id_secret, &sid)
        .map(|email| auth::normalize_email(&email))
        .filter(|email| !email.is_empty())
}

/// The style `path` renders in for this request.
pub fn for_page(state: &Arc<AppState>, headers: &HeaderMap, path: &str) -> PageStyle {
    if crate::hosts::is_rjuhsd_host(headers)
        || crate::hosts::is_pickle_host(headers)
        || is_mitch_only_path(path)
    {
        return PageStyle::default();
    }
    let Some(email) = signed_in_email(state, headers) else {
        return PageStyle::default();
    };
    match ui_style::get(&state.store, &email) {
        Some(style) => PageStyle { style, ask: false },
        None => PageStyle {
            style: UiStyle::Mitch,
            ask: true,
        },
    }
}

/// The Tylerium variant of a page file (`webserver/tylerium/<rel>`), if there
/// is one. `rel` is the webroot-relative file the request resolved to, e.g.
/// `index.html` or `leaderboard/index.html`.
pub fn overlay_path(webroot: &std::path::Path, rel: &str) -> Option<std::path::PathBuf> {
    let rel = rel.trim_start_matches('/');
    if rel.is_empty() || rel.starts_with("tylerium/") {
        return None;
    }
    crate::static_files::safe_webroot_path(webroot, &format!("tylerium/{rel}"))
        .filter(|path| path.is_file())
}

/// Adds `data-ui="tylerium"` to the page's `<html>` tag. Pages without one
/// are returned unchanged.
pub fn mark_html(raw: &str) -> String {
    let mut from = 0;
    while let Some(found) = raw[from..].find("<html") {
        let at = from + found;
        let after = at + "<html".len();
        match raw[after..].chars().next() {
            Some(c) if c == '>' || c.is_whitespace() => {
                return format!("{} data-ui=\"tylerium\"{}", &raw[..after], &raw[after..]);
            }
            _ => from = after,
        }
    }
    raw.to_string()
}

pub(crate) fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Option<Response> {
    if path != "/api/me/ui-style" {
        return None;
    }
    let Some(email) = signed_in_email(state, headers) else {
        return Some(json_response(401, json!({ "error": "Not signed in." })));
    };
    match *method {
        Method::GET => {
            let saved = ui_style::get(&state.store, &email);
            Some(json_response(
                200,
                json!({
                    "style": saved.unwrap_or_default().as_str(),
                    "chosen": saved.is_some(),
                }),
            ))
        }
        Method::POST => {
            let body: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
            let Some(style) = body
                .get("style")
                .and_then(Value::as_str)
                .and_then(UiStyle::parse)
            else {
                return Some(json_response(
                    400,
                    json!({ "error": "Choose mitch or tylerium." }),
                ));
            };
            if !ui_style::set(&state.store, &email, style) {
                return Some(json_response(
                    500,
                    json!({ "error": "Could not save your choice. Try again." }),
                ));
            }
            Some(json_response(
                200,
                json!({ "style": style.as_str(), "chosen": true }),
            ))
        }
        _ => Some(json_response(
            405,
            json!({ "error": "Method not allowed." }),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn test_state() -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!(
            "mitch-server-ui-style-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap_or_default();
        let cfg = crate::hosts::SiteConfig {
            data_dir: dir.join("data"),
            ..crate::hosts::SiteConfig::load()
        };
        let store = Arc::new(
            mitch_lib::data::DataStore::open(&dir, &dir.join("data"))
                .unwrap_or_else(|e| panic!("store: {e}")),
        );
        Arc::new(AppState::new(cfg, store))
    }

    fn signed_in(state: &Arc<AppState>, email: &str, host: &str) -> HeaderMap {
        let norm = auth::normalize_email(email);
        mitch_lib::passwords::password_set(&state.store, &norm, "argon2id$hash");
        let sess = auth::create_auth_session(&state.store, &state.id_secret, &norm, email, false);
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_str(&format!(
                "mitch_session={}; studentId={}",
                sess.token, sess.sid
            ))
            .unwrap(),
        );
        headers.insert(
            axum::http::header::HOST,
            HeaderValue::from_str(host).unwrap(),
        );
        headers
    }

    async fn body_json(resp: Response) -> Value {
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn choice_is_asked_once_then_applies_to_signed_in_pages() {
        let state = test_state();
        let email = "test.student1@student.rjuhsd.us";
        let headers = signed_in(&state, email, "mitch.pro");

        assert_eq!(
            for_page(&state, &headers, "/"),
            PageStyle {
                style: UiStyle::Mitch,
                ask: true
            }
        );
        let status = handle(&state, &Method::GET, "/api/me/ui-style", &headers, b"").unwrap();
        assert_eq!(
            body_json(status).await,
            json!({"style": "mitch", "chosen": false})
        );

        let bad = handle(
            &state,
            &Method::POST,
            "/api/me/ui-style",
            &headers,
            br#"{"style":"neon"}"#,
        )
        .unwrap();
        assert_eq!(bad.status(), 400);

        let saved = handle(
            &state,
            &Method::POST,
            "/api/me/ui-style",
            &headers,
            br#"{"style":"tylerium"}"#,
        )
        .unwrap();
        assert_eq!(saved.status(), 200);
        assert_eq!(
            for_page(&state, &headers, "/game-portal/"),
            PageStyle {
                style: UiStyle::Tylerium,
                ask: false
            }
        );

        // Public pages and the school site stay mitch.pro and never ask.
        for path in ["/privacy/", "/index-sales.html", "/enroll/", "/rjuhsd/"] {
            assert_eq!(for_page(&state, &headers, path), PageStyle::default());
        }
        let school = signed_in(&state, email, "rjuhsd.school");
        assert_eq!(for_page(&state, &school, "/bell/"), PageStyle::default());
    }

    #[tokio::test]
    async fn signed_out_requests_get_mitch_and_no_api() {
        let state = test_state();
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::HOST,
            HeaderValue::from_static("mitch.pro"),
        );
        assert_eq!(for_page(&state, &headers, "/"), PageStyle::default());
        let resp = handle(&state, &Method::GET, "/api/me/ui-style", &headers, b"").unwrap();
        assert_eq!(resp.status(), 401);
        assert!(handle(&state, &Method::GET, "/api/me/other", &headers, b"").is_none());
    }

    #[test]
    fn pipeline_applies_the_page_style() {
        let state = test_state();
        let headers = HeaderMap::new();
        let page =
            "<!doctype html><html lang=\"en\"><head><title>x</title></head><body></body></html>";
        let render = |path: &str, style: PageStyle| {
            let mut html = page.to_string();
            crate::pipeline::inject_page(&state, &headers, path, path, &mut html, style)
        };
        let tylerium = PageStyle {
            style: UiStyle::Tylerium,
            ask: false,
        };

        let out = render("/vms/", tylerium);
        assert!(out.contains("<html data-ui=\"tylerium\" lang=\"en\">"));
        assert!(out.contains("/tylerium/tokens.css"));
        assert!(out.contains("/tylerium/tylerium.css"));
        assert!(out.contains("/background.js"));
        assert!(!out.contains("/ui-style.js"));

        // The games portal draws its own chrome: tokens and background only.
        let portal = render("/game-portal/", tylerium);
        assert!(portal.contains("/tylerium/tokens.css"));
        assert!(!portal.contains("/tylerium/tylerium.css"));
        assert!(portal.contains("/background.js"));

        let mitch = render("/vms/", PageStyle::default());
        assert!(!mitch.contains("data-ui="));
        assert!(!mitch.contains("/tylerium/"));
        assert!(!mitch.contains("/background.js"));
        assert!(!mitch.contains("/ui-style.js"));

        let ask = render(
            "/",
            PageStyle {
                style: UiStyle::Mitch,
                ask: true,
            },
        );
        assert!(ask.contains("<script src=\"/ui-style.js?v=1\" data-ask=\"1\" defer></script>"));
    }

    #[test]
    fn tylerium_overlays_exist_for_home_and_rankings_only() {
        let webroot = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../webserver")
            .canonicalize()
            .unwrap();
        assert!(overlay_path(&webroot, "index.html").is_some());
        assert!(overlay_path(&webroot, "leaderboard/index.html").is_some());
        assert!(overlay_path(&webroot, "vms/index.html").is_none());
        assert!(overlay_path(&webroot, "tylerium/index.html").is_none());
        assert!(overlay_path(&webroot, "../etc/passwd").is_none());
    }

    #[test]
    fn marks_the_html_tag_only() {
        assert_eq!(
            mark_html("<!doctype html>\n<html lang=\"en\"><head>"),
            "<!doctype html>\n<html data-ui=\"tylerium\" lang=\"en\"><head>"
        );
        assert_eq!(
            mark_html("<html><body>"),
            "<html data-ui=\"tylerium\"><body>"
        );
        assert_eq!(mark_html("<htmlish><p>"), "<htmlish><p>");
    }

    #[test]
    fn mitch_only_sections_match_by_first_segment() {
        assert!(is_mitch_only_path("/privacy/"));
        assert!(is_mitch_only_path("/privacy/index.html"));
        assert!(is_mitch_only_path("/index-sales.html"));
        assert!(!is_mitch_only_path("/"));
        assert!(!is_mitch_only_path("/vms/"));
        assert!(!is_mitch_only_path("/privacy-center/"));
    }
}
