//! `GET /api/staff-hub/status` — tells the homepage footer and `/staff/`
//! itself whether the viewer is admin, moderator, or tester, so a link to
//! the staff directory page can be shown/hidden and the page itself can
//! gate its own content. New subsystem, no `server.js` equivalent.
//!
//! Deliberately its own tiny endpoint rather than adding fields to
//! `/api/me` (a `server.js`-parity-tracked response shape) or depending on
//! `/api/tester/status` (referenced by `webserver/tester/index.html` but
//! not actually implemented anywhere in this server yet — a pre-existing
//! gap, not something this change takes on fixing).

use crate::routes::me::{cookies_of, json_response, me_uid};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::auth;
use serde_json::json;
use std::sync::Arc;

pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
) -> Option<Response> {
    if path != "/api/staff-hub/status" || *method != Method::GET {
        return None;
    }
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    let node_env_test = std::env::var("NODE_ENV").unwrap_or_default() == "test";
    let is_admin = auth::is_admin_id(&state.store, &state.id_secret, &sid, node_env_test);
    let is_moderator = auth::is_moderator_id(&state.store, &state.id_secret, &sid);
    let email = auth::email_from_sid(&state.store, &state.id_secret, &sid);
    let is_tester = email
        .as_deref()
        .is_some_and(|e| auth::is_tester_email(&state.store, e));
    Some(json_response(
        200,
        json!({
            "isAdmin": is_admin,
            "isModerator": is_moderator,
            "isTester": is_tester,
            "isStaff": is_admin || is_moderator || is_tester,
        }),
    ))
}
