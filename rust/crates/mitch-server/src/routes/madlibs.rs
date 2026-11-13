//! `/api/madlibs` — CORS-friendly random Mad Libs API.
//! Serves fill-in-the-blank story templates formatted as:
//! `{ "title": String, "text": Vec<String>, "blanks": Vec<String> }`
//! with universal CORS headers (`Access-Control-Allow-Origin: *`)
//! for cross-origin use (e.g. Pyodide in web browsers on any domain).

use crate::state::AppState;
use axum::http::{header, Method, Response, StatusCode};
use serde_json::{json, Value};

/// Handle `/api/madlibs`, `/api/madlibs/random`, `/api/madlibs/list`, etc.
pub fn handle(
    state: &AppState,
    method: &Method,
    path: &str,
    search: &str,
) -> Response<axum::body::Body> {
    // 1. CORS Preflight
    if method == Method::OPTIONS {
        return Response::builder()
            .status(StatusCode::NO_CONTENT)
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(header::ACCESS_CONTROL_ALLOW_METHODS, "GET, OPTIONS, HEAD")
            .header(header::ACCESS_CONTROL_ALLOW_HEADERS, "*")
            .header(header::ACCESS_CONTROL_MAX_AGE, "86400")
            .body(axum::body::Body::empty())
            .unwrap_or_default();
    }

    if method != Method::GET && method != Method::HEAD {
        return Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
            .body(axum::body::Body::from(
                json!({ "error": "method not allowed" }).to_string(),
            ))
            .unwrap_or_default();
    }

    let templates = load_templates(state);
    if templates.is_empty() {
        return Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
            .body(axum::body::Body::from(
                json!({ "error": "no templates available" }).to_string(),
            ))
            .unwrap_or_default();
    }

    let qs: std::collections::HashMap<String, String> = form_urlencoded::parse(search.as_bytes())
        .into_owned()
        .collect();

    // /api/madlibs/list or ?list=1 returns all available titles
    if path == "/api/madlibs/list"
        || qs
            .get("list")
            .map(|v| v == "1" || v == "true")
            .unwrap_or(false)
    {
        let titles: Vec<&str> = templates
            .iter()
            .filter_map(|t| t.get("title").and_then(|v| v.as_str()))
            .collect();
        return cors_json(
            StatusCode::OK,
            json!({ "count": titles.len(), "titles": titles }),
        );
    }

    let requested_title = qs
        .get("title")
        .or_else(|| qs.get("name"))
        .cloned()
        .or_else(|| {
            path.strip_prefix("/api/madlibs/story/")
                .map(|p| p.replace("%20", " "))
        });

    let chosen = if let Some(t) = requested_title {
        let low = t.to_lowercase().trim().to_string();
        templates
            .iter()
            .find(|item| {
                item.get("title")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_lowercase().trim() == low)
                    .unwrap_or(false)
            })
            .unwrap_or_else(|| pick_random(&templates))
    } else {
        pick_random(&templates)
    };

    cors_json(StatusCode::OK, chosen.clone())
}

fn cors_json(status: StatusCode, value: Value) -> Response<axum::body::Body> {
    Response::builder()
        .status(status)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCESS_CONTROL_ALLOW_METHODS, "GET, OPTIONS, HEAD")
        .header(header::ACCESS_CONTROL_ALLOW_HEADERS, "*")
        .header(header::CACHE_CONTROL, "no-cache, no-store, must-revalidate")
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .body(axum::body::Body::from(value.to_string()))
        .unwrap_or_default()
}

fn pick_random(templates: &[Value]) -> &Value {
    use rand::Rng;
    let idx = rand::rng().random_range(0..templates.len());
    &templates[idx]
}

fn load_templates(state: &AppState) -> Vec<Value> {
    mitch_lib::site_content::madlibs(&state.store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn test_state() -> (Arc<AppState>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "mitch-server-madlibs-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap_or_default();
        let cfg = crate::hosts::SiteConfig::load();
        let cfg = crate::hosts::SiteConfig {
            data_dir: dir.join("data"),
            ..cfg
        };
        let store = Arc::new(
            mitch_lib::data::DataStore::open(&dir, &dir.join("data"))
                .unwrap_or_else(|e| panic!("store: {e}")),
        );
        (Arc::new(AppState::new(cfg, Arc::clone(&store))), dir)
    }

    #[test]
    fn test_cors_preflight_options() {
        let (state, dir) = test_state();
        let resp = handle(&state, &Method::OPTIONS, "/api/madlibs", "");
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            resp.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            "*"
        );
        assert_eq!(
            resp.headers()
                .get(header::ACCESS_CONTROL_ALLOW_METHODS)
                .unwrap(),
            "GET, OPTIONS, HEAD"
        );
        assert_eq!(
            resp.headers()
                .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
                .unwrap(),
            "*"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn test_random_madlibs_get() {
        let (state, dir) = test_state();
        let resp = handle(&state, &Method::GET, "/api/madlibs", "");
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            "*"
        );

        // Body must have title, text, and blanks matching length
        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX);
        let val: Value = serde_json::from_slice(
            &tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(body_bytes)
                .unwrap(),
        )
        .unwrap();
        assert!(val.get("title").and_then(|v| v.as_str()).is_some());
        let text = val.get("text").and_then(|v| v.as_array()).unwrap();
        let blanks = val.get("blanks").and_then(|v| v.as_array()).unwrap();
        assert_eq!(text.len(), blanks.len() + 1);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn test_madlibs_by_title() {
        let (state, dir) = test_state();
        let resp = handle(
            &state,
            &Method::GET,
            "/api/madlibs",
            "title=How%20Pizza%20Was%20Invented",
        );
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX);
        let val: Value = serde_json::from_slice(
            &tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(body_bytes)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            val.get("title").and_then(|v| v.as_str()),
            Some("How Pizza Was Invented")
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn test_madlibs_list() {
        let (state, dir) = test_state();
        let resp = handle(&state, &Method::GET, "/api/madlibs/list", "");
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX);
        let val: Value = serde_json::from_slice(
            &tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(body_bytes)
                .unwrap(),
        )
        .unwrap();
        let count = val.get("count").and_then(|v| v.as_u64()).unwrap();
        assert!(count >= 80, "Expected at least 80 templates, got {count}");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn test_method_not_allowed() {
        let (state, dir) = test_state();
        let resp = handle(&state, &Method::POST, "/api/madlibs", "");
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(
            resp.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            "*"
        );

        let _ = std::fs::remove_dir_all(dir);
    }
}
