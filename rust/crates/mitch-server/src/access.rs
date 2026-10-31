//! Ban enforcement before redirects, public routes, proxies, and upgrades.
use crate::state::AppState;
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::Response;
use serde_json::{json, Value};
use std::sync::Arc;

pub const BAN_COOKIE: &str = "mitch_ban";

fn appeal_or_asset(path: &str) -> bool {
    matches!(
        path,
        "/appeal"
            | "/appeal/"
            | "/appeal.html"
            | "/appeal/index.html"
            | "/api/appeal"
            | "/ban-guard.js"
            | "/site-status.js"
            | "/sw.js"
            | "/favicon.ico"
            | "/icon-192.png"
            | "/apple-touch-icon.png"
    )
}

pub fn account_response(
    state: &AppState,
    headers: &HeaderMap,
    path: &str,
    email: &str,
    ban: &Value,
) -> Response {
    let api = path.starts_with("/api/")
        || path.starts_with("/_matrix/")
        || path.starts_with("/livekit/")
        || path.ends_with("/ws")
        || path == "/ws";
    let mut response = if api {
        crate::errors::json_resp(
            403,
            json!({
                "success":false, "banned":true, "error":"account banned",
                "errcode":"M_FORBIDDEN", "reason":ban.get("reason"),
                "bannedAt":ban.get("banned_at"), "expiresAt":ban.get("expires_at")
            }),
        )
    } else {
        crate::errors::banned_response(ban, crate::hosts::is_rjuhsd_host(headers))
    };
    if !email.is_empty() {
        let marker = mitch_lib::bans::signed_ban_marker(email, &state.id_secret);
        let secure = std::env::var("NODE_ENV").as_deref() == Ok("production");
        let cookie =
            mitch_lib::auth::set_cookie_header(BAN_COOKIE, &marker, "", secure, 31_536_000, true);
        if let Ok(value) = HeaderValue::from_str(&cookie) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    response
}

pub async fn enforce_ban(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    path: &str,
    search: &str,
) -> Option<Response> {
    if appeal_or_asset(path) {
        return None;
    }
    let ip = crate::handler::get_real_ip(headers, None);
    if let Some(ban) = mitch_lib::auth::banned_info_for_ip(&state.store, &ip) {
        return Some(account_response(state, headers, path, "", &ban));
    }
    let cookies = crate::routes::me::cookies_of(state, headers);
    let mut emails = Vec::new();
    if let Some(email) = cookies
        .get(BAN_COOKIE)
        .and_then(|value| mitch_lib::bans::verify_ban_marker(value, &state.id_secret))
    {
        emails.push(email);
    }
    for sid in [
        cookies.get("studentId").unwrap_or(""),
        cookies.get("id").unwrap_or(""),
        &cookies.raw_student_id,
        &cookies.raw_id,
    ] {
        if mitch_lib::auth::valid_id(sid, &state.id_secret) {
            if let Some(email) = mitch_lib::auth::names_get_email(&state.store, sid)
                .or_else(|| mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, sid))
            {
                emails.push(email);
            }
        }
    }
    for email in emails {
        if let Some(ban) = mitch_lib::auth::banned_info_for_email(&state.store, &email) {
            return Some(account_response(state, headers, path, &email, &ban));
        }
    }
    if path.starts_with("/livekit") {
        if let Some(email) = crate::routes::livekit::verified_token_email(state, search) {
            if let Some(ban) = mitch_lib::auth::banned_info_for_email(&state.store, &email) {
                return Some(account_response(state, headers, path, &email, &ban));
            }
        }
    }
    if path.starts_with("/_matrix/") || path.starts_with("/api/matrix/") {
        let mut tokens: Vec<String> = headers
            .get_all(header::AUTHORIZATION)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter_map(|value| {
                value
                    .split_once(' ')
                    .filter(|(kind, _)| kind.eq_ignore_ascii_case("bearer"))
                    .map(|(_, token)| token.trim().to_string())
            })
            .collect();
        tokens.extend(
            form_urlencoded::parse(search.as_bytes())
                .filter(|(key, _)| key == "access_token")
                .map(|(_, token)| token.into_owned()),
        );
        tokens.sort();
        tokens.dedup();
        if tokens.len() > 4 {
            return Some(crate::errors::json_resp(
                400,
                json!({"error":"Too many access tokens."}),
            ));
        }
        for token in tokens {
            let mut token_headers = headers.clone();
            token_headers.remove(header::AUTHORIZATION);
            if let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}")) {
                token_headers.insert(header::AUTHORIZATION, value);
                if let Some(account) =
                    crate::routes::matrix::resolve_matrix_account(state, &token_headers, None).await
                {
                    if let Some(ban) =
                        mitch_lib::auth::banned_info_for_email(&state.store, &account.norm_email)
                    {
                        return Some(account_response(
                            state,
                            headers,
                            path,
                            &account.norm_email,
                            &ban,
                        ));
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{Method, Uri};
    fn fixture() -> (Arc<AppState>, std::path::PathBuf, String) {
        let dir = std::env::temp_dir().join(format!(
            "mitch-access-test-{}",
            mitch_lib::crypto::random_bytes_hex(12)
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap();
        let cfg = crate::hosts::SiteConfig {
            base_dir: dir.clone(),
            data_dir: dir.join("data"),
            webroot: dir.join("webserver"),
            ..crate::hosts::SiteConfig::load()
        };
        let store = Arc::new(mitch_lib::data::DataStore::open(&dir, &cfg.data_dir).unwrap());
        let state = Arc::new(AppState::new(cfg, store));
        let session = mitch_lib::auth::create_auth_session(
            &state.store,
            &state.id_secret,
            "banned@example.com",
            "banned@example.com",
            "fixture",
            "127.0.0.1",
            false,
        );
        mitch_lib::bans::blacklist_insert(
            &state.store,
            "banned@example.com",
            "Repeated spam",
            "admin",
        )
        .unwrap();
        (state, dir, session.token)
    }
    #[tokio::test]
    async fn bans_precede_every_page_api_proxy_and_host_redirect() {
        let (state, dir, token) = fixture();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("mitch_session={token}")).unwrap(),
        );
        for host in ["mitchdog.com", "mitch.pro", "rjuhsd.school"] {
            headers.insert(header::HOST, HeaderValue::from_str(host).unwrap());
            for path in [
                "/",
                "/enroll/",
                "/games/foo/index.html",
                "/game-portal/",
                "/coins/",
                "/preferences/",
                "/admin/",
                "/api/presence/count",
                "/api/madlibs/random",
                "/api/pass",
                "/matrix/config.json",
                "/_matrix/client/v3/sync",
                "/livekit/get_token",
                "/proxy/luma/test.html",
                "/verify-open.json",
                "/unsubscribe/token",
                "/ws",
            ] {
                for method in [Method::GET, Method::POST, Method::HEAD] {
                    let response = crate::handler::handle(
                        state.clone(),
                        method,
                        &path.parse::<Uri>().unwrap(),
                        &headers,
                        b"{}",
                        None,
                    )
                    .await;
                    assert_eq!(response.status(), 403, "{host}{path}");
                    assert!(response
                        .headers()
                        .get(header::CACHE_CONTROL)
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .contains("no-store"));
                }
            }
        }
        for path in [
            "/appeal/",
            "/appeal.html",
            "/api/appeal",
            "/sw.js",
            "/ban-guard.js",
        ] {
            assert!(enforce_ban(&state, &headers, path, "").await.is_none());
        }
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn removing_login_cookie_or_changing_generation_does_not_clear_ban() {
        let (state, dir, token) = fixture();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("mitch_session={token}")).unwrap(),
        );
        let response = enforce_ban(&state, &headers, "/", "").await.unwrap();
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Lax"));
        let marker = cookie.split(';').next().unwrap().to_string();
        headers.insert(header::COOKIE, HeaderValue::from_str(&marker).unwrap());
        mitch_lib::auth::rotate_session_generation(&state.store, "banned@example.com");
        assert_eq!(
            enforce_ban(&state, &headers, "/games/foo/index.html", "")
                .await
                .unwrap()
                .status(),
            403
        );
        mitch_lib::bans::blacklist_insert_until(
            &state.store,
            "banned@example.com",
            "Expired",
            "admin",
            Some(1),
        )
        .unwrap();
        assert!(enforce_ban(&state, &headers, "/games/foo/index.html", "")
            .await
            .is_none());
        mitch_lib::bans::blacklist_insert(&state.store, "banned@example.com", "Reban", "admin")
            .unwrap();
        mitch_lib::bans::blacklist_remove(&state.store, "banned@example.com", "banned@example.com")
            .unwrap();
        assert!(enforce_ban(&state, &headers, "/", "").await.is_none());
        assert!(
            enforce_ban(&state, &HeaderMap::new(), "/games/foo/index.html", "")
                .await
                .is_none()
        );
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
