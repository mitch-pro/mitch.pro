//! `GET /api/download-information` — the signed-in account's data as a JSON
//! download. Built by `mitch_lib::account_deletion::export_account` from the
//! same table and document lists deletion uses, so what you can download is
//! what deleting removes. Password hashes, session tokens and push keys are
//! never included.

use super::{cookies_of, json_response, me_uid};
use crate::state::AppState;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::Response;
use mitch_lib::account_deletion::{self, Identity};
use mitch_lib::auth;
use serde_json::json;
use std::sync::Arc;

pub(crate) fn download_information(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let cookies = cookies_of(state, headers);
    let uid = me_uid(&cookies);
    if !auth::valid_id(&uid, &state.id_secret) {
        return json_response(
            401,
            json!({ "error": "Sign in to download your information." }),
        );
    }
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &uid) else {
        return json_response(
            401,
            json!({ "error": "Sign in to download your information." }),
        );
    };
    let norm = auth::normalize_email(&email);
    let aliases = account_deletion::session_ids_for(&state.store, &norm)
        .into_iter()
        .chain(crate::routes::matrix::matrix_user_ids_for_account(
            state, &norm,
        ));
    let identity = Identity::new(&norm, aliases);
    let data = account_deletion::export_account(&state.store, &identity);
    let body = json!({
        "account": email,
        "exportedAt": mitch_lib::school::now_millis(),
        "data": data,
    });
    let mut resp = json_response(200, body);
    resp.headers_mut().insert(
        axum::http::header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"mitch-pro-information.json\""),
    );
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    resp
}
