//! Staff side of the feedback inbox (`/api/admin/feedback`). Submissions come
//! from `POST /api/suggest` (routes/auth.rs); the storage and rules are in
//! `mitch_lib::feedback`, the page is `/moderator/feedback/`, and the overview
//! is `docs/feedback.md`.
//!
//! Moderators and admins can read the inbox and mark entries handled. Only full
//! admins can delete. Admins also pass the usual passphrase gate.

use std::sync::Arc;

use axum::http::Method;
use serde_json::{json, Value};

use super::{forbidden, json_response, AdminCtx, Resp};
use crate::state::AppState;
use mitch_lib::feedback as db;

const BASE: &str = "/api/admin/feedback";

fn query_param(search: &str, key: &str) -> String {
    url::form_urlencoded::parse(search.trim_start_matches('?').as_bytes())
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

fn entry_json(e: &db::Entry) -> Value {
    json!({
        "id": e.id,
        "type": e.kind,
        "text": e.text,
        "account": e.account,
        "createdAt": e.created_at,
        "status": e.status,
        "handledBy": e.handled_by,
        "handledAt": e.handled_at,
    })
}

pub fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    search: &str,
    body: &Value,
    ctx: &AdminCtx,
) -> Resp {
    let rest = path.strip_prefix(BASE)?;
    if !matches!(rest, "" | "/status" | "/delete") {
        return None;
    }
    if !ctx.is_any_admin(state) {
        return Some(forbidden());
    }
    let id = body.get("id").and_then(Value::as_i64).unwrap_or(-1);
    Some(match (method.as_str(), rest) {
        ("GET", "") => {
            // The old suggestions.json is folded in the first time anyone looks.
            let legacy = state
                .store
                .read_document(&state.cfg.data_dir.join("suggestions.json"), json!([]));
            db::import_legacy(&state.store, &legacy);
            let (new, handled) = db::counts(&state.store);
            let entries: Vec<Value> = db::list(&state.store, &query_param(search, "status"))
                .iter()
                .map(entry_json)
                .collect();
            json_response(
                200,
                json!({ "entries": entries, "counts": { "new": new, "handled": handled } }),
            )
        }
        ("POST", "/status") => {
            let status = body.get("status").and_then(Value::as_str).unwrap_or("");
            let email = ctx.email(state);
            if db::set_status(
                &state.store,
                id,
                status,
                &email,
                mitch_lib::school::now_millis(),
            ) {
                json_response(200, json!({ "ok": true }))
            } else {
                json_response(404, json!({ "error": "That entry no longer exists." }))
            }
        }
        ("POST", "/delete") => {
            if !ctx.is_admin(state) {
                return Some(forbidden());
            }
            if db::delete(&state.store, id) {
                mitch_lib::admin::log_admin_action(
                    &state.store,
                    &state.cfg.data_dir,
                    &ctx.email(state),
                    "delete_feedback",
                    json!({ "id": id }),
                );
                json_response(200, json!({ "ok": true }))
            } else {
                json_response(404, json!({ "error": "That entry no longer exists." }))
            }
        }
        _ => json_response(405, json!({ "error": "method not allowed" })),
    })
}
