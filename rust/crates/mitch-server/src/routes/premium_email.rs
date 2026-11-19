//! Premium members' free `@student.mitch.pro` address: the member-facing API.
//!
//! - `GET  /api/premium/email/status`: what the application page and the popup
//!   need. 401 signed out, 403 not Premium.
//! - `POST /api/premium/email/register`: `{subdomain, fullName, reasons}`.
//! - `POST /api/premium/email/dismiss`: close the popup for good.
//!
//! Staff review is `routes/admin/premium_email.rs`. Storage and rules are in
//! `mitch_lib::premium_email`; see `docs/premium-email.md`.

use crate::routes::me::{cookies_of, json_response, me_uid};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::{auth, premium_email as pe};
use serde_json::{json, Value};
use std::sync::Arc;

pub(crate) fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Option<Response> {
    match (method, path) {
        (&Method::GET, "/api/premium/email/status") => Some(status(state, headers)),
        (&Method::POST, "/api/premium/email/register") => {
            Some(register(state, headers, body_bytes))
        }
        (&Method::POST, "/api/premium/email/dismiss") => Some(dismiss(state, headers)),
        _ => None,
    }
}

/// The signed-in account's normalized email.
fn signed_in(state: &Arc<AppState>, headers: &HeaderMap) -> Option<String> {
    let uid = me_uid(&cookies_of(state, headers));
    if !auth::valid_id(&uid, &state.id_secret) {
        return None;
    }
    auth::email_from_sid(&state.store, &state.id_secret, &uid)
        .map(|email| auth::normalize_email(&email))
}

fn status(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let Some(norm) = signed_in(state, headers) else {
        return json_response(401, json!({ "error": "Not signed in." }));
    };
    if !auth::is_premium_email(&state.store, &norm) {
        return json_response(403, json!({ "hasPremium": false }));
    }
    let active = pe::active_for(&state.store, &norm);
    let username = mitch_lib::profiles::profile_get(&state.store, &norm)
        .get("username")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_default();
    json_response(
        200,
        json!({
            "hasPremium": true,
            "currentEmail": active.as_ref().map(|r| r.full_address()),
            "isPending": active.as_ref().is_some_and(|r| r.status == "pending"),
            "defaultSubdomain": pe::suggest_address(&username),
            "showPrompt": pe::should_prompt(&state.store, &norm),
        }),
    )
}

fn register(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(norm) = signed_in(state, headers) else {
        return json_response(401, json!({ "error": "Not signed in." }));
    };
    if !auth::is_premium_email(&state.store, &norm) {
        return json_response(403, json!({ "error": "Premium is required." }));
    }
    let body: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
    let text = |key: &str| {
        body.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let address = text("subdomain").to_lowercase();
    match pe::create(
        &state.store,
        &norm,
        &address,
        &text("fullName"),
        &text("reasons"),
    ) {
        Ok(_) => json_response(200, json!({ "success": true })),
        Err(pe::CreateError::Invalid(message)) => json_response(400, json!({ "error": message })),
        Err(pe::CreateError::AlreadyApplied) => json_response(
            409,
            json!({ "error": "You already have a request or an address." }),
        ),
        Err(pe::CreateError::AddressTaken) => {
            json_response(409, json!({ "error": "That address is taken." }))
        }
        Err(pe::CreateError::Storage) => json_response(
            500,
            json!({ "error": "Could not save your request. Try again." }),
        ),
    }
}

fn dismiss(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let Some(norm) = signed_in(state, headers) else {
        return json_response(401, json!({ "error": "Not signed in." }));
    };
    pe::dismiss_prompt(&state.store, &norm);
    json_response(200, json!({ "success": true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn test_state() -> (Arc<AppState>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "mitch-server-premium-email-test-{}",
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
        (Arc::new(AppState::new(cfg, store)), dir)
    }

    fn headers_for(state: &Arc<AppState>, email: &str) -> HeaderMap {
        let norm = auth::normalize_email(email);
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
        headers
    }

    async fn body_json(resp: Response) -> Value {
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn status_register_and_dismiss_follow_premium_and_prompt_rules() {
        let (state, dir) = test_state();
        let email = "test.student2@student.rjuhsd.us";
        let headers = headers_for(&state, email);

        assert_eq!(
            status(&state, &headers).status(),
            403,
            "free accounts are not eligible"
        );
        assert_eq!(register(&state, &headers, b"{}").status(), 403);

        // Make the account Premium the way a grant does.
        state
            .store
            .write_document(
                &dir.join("data/applications.json"),
                &json!([{"email": email, "type": "premium", "status": "approved", "grantPremium": true, "neverExpire": true}]),
            )
            .unwrap();
        assert!(auth::is_premium_email(&state.store, email));

        let s = body_json(status(&state, &headers)).await;
        assert_eq!(s["hasPremium"], true);
        assert_eq!(s["showPrompt"], true);
        assert_eq!(s["isPending"], false);

        let dismissed = dismiss(&state, &headers);
        assert_eq!(dismissed.status(), 200);
        assert_eq!(
            body_json(status(&state, &headers)).await["showPrompt"],
            false
        );

        let bad = register(
            &state,
            &headers,
            br#"{"subdomain":"ad","fullName":"A","reasons":"b"}"#,
        );
        assert_eq!(bad.status(), 400);
        let ok = register(
            &state,
            &headers,
            br#"{"subdomain":"member99","fullName":"Member","reasons":"I like it"}"#,
        );
        assert_eq!(ok.status(), 200);
        let s = body_json(status(&state, &headers)).await;
        assert_eq!(s["isPending"], true);
        assert_eq!(s["currentEmail"], "member99@student.mitch.pro");
        std::fs::remove_dir_all(dir).ok();
    }
}
