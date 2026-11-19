//! Staff review of Premium email requests (`/staff-dashboard/premium-email/`).
//!
//! - `GET  /api/admin/premium-email`: the queue, pending first.
//! - `POST /api/admin/premium-email/decide`: `{id, action: "approve"|"deny", note?}`.
//!
//! Approving only records the decision and emails the member: the mailbox itself
//! is created on the mail host by the admin before approving. Full admins only.
//! Member side: `routes/premium_email.rs`; rules in `docs/premium-email.md`.

use super::{forbidden, json_response, AdminCtx, Resp};
use crate::state::AppState;
use axum::http::Method;
use mitch_lib::premium_email as pe;
use serde_json::{json, Value};
use std::sync::Arc;

fn request_json(r: &pe::Request) -> Value {
    json!({
        "id": r.id,
        "email": r.email,
        "address": r.full_address(),
        "fullName": r.full_name,
        "reasons": r.reasons,
        "status": r.status,
        "createdAt": r.created_at,
        "decidedBy": r.decided_by,
        "decidedAt": r.decided_at,
        "note": r.note,
    })
}

pub fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    body: &Value,
    ctx: &AdminCtx,
) -> Resp {
    match (method, path) {
        (&Method::GET, "/api/admin/premium-email") => {
            if !ctx.is_admin(state) {
                return Some(forbidden());
            }
            let requests: Vec<Value> = pe::list(&state.store, 200)
                .iter()
                .map(request_json)
                .collect();
            Some(json_response(
                200,
                json!({ "requests": requests, "pending": pe::pending_count(&state.store) }),
            ))
        }
        (&Method::POST, "/api/admin/premium-email/decide") => {
            if !ctx.is_admin(state) {
                return Some(forbidden());
            }
            Some(decide(state, body, ctx))
        }
        _ => None,
    }
}

fn decide(state: &Arc<AppState>, body: &Value, ctx: &AdminCtx) -> axum::response::Response {
    let id = body.get("id").and_then(Value::as_i64).unwrap_or(0);
    let approve = match body.get("action").and_then(Value::as_str) {
        Some("approve") => true,
        Some("deny") => false,
        _ => return json_response(400, json!({ "error": "Choose approve or deny." })),
    };
    let note = body
        .get("note")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let by = ctx.email(state);
    let Some(request) = pe::decide(&state.store, id, approve, &by, note) else {
        return json_response(
            404,
            json!({ "error": "That request isn't pending any more." }),
        );
    };
    notify_member(state, &request);
    json_response(
        200,
        json!({ "ok": true, "request": request_json(&request) }),
    )
}

/// Emails the member the decision. The address is the account's own, resolved
/// the same way as other account mail.
fn notify_member(state: &Arc<AppState>, request: &pe::Request) {
    let to = mitch_lib::profile::canonical_delivery_email(
        &state.store,
        &state.cfg.data_dir,
        &state.id_secret,
        &request.email,
    );
    let (subject, message) = if request.status == "approved" {
        (
            "Your @student.mitch.pro address is ready".to_string(),
            format!(
                "Your address {} is set up. Check your email app, or sign in to the mail server with it.",
                request.full_address()
            ),
        )
    } else {
        let reason = if request.note.is_empty() {
            String::new()
        } else {
            format!(" Reason: {}", request.note)
        };
        (
            "Your email address request".to_string(),
            format!(
                "We couldn't set up {} for you.{reason} You can send a new request with a different address.",
                request.full_address()
            ),
        )
    };
    let base = crate::workers_email::site_url(state, &to);
    let html = super::legacy::premium_alert_html(
        state,
        &to,
        &subject,
        &message,
        &format!("{base}/premium-email"),
        "Open",
    );
    crate::routes::push::send_email_bg(state, &to, &subject, &html);
}
