//! Staff side of the support portal (`/api/admin/support/*`). The public side
//! is `routes/support.rs`; the rules are in `docs/support.md`.
//!
//! Who can do what:
//! - Moderators and above: read the queue, chat, set status, flag a ticket as
//!   "account compromised", and *request* an email change for the account a
//!   ticket is linked to.
//! - Admins and above: approve or deny those requests, but never their own
//!   (enforced in `mitch_lib::support::decide_email_request`).
//! - Staff accounts, banned accounts and addresses already in use can never be
//!   the subject of a request (`check_email_change_allowed`).
//!
//! Approving does not change anything by itself: it emails a code to the NEW
//! address and the ticket's owner must enter it on their ticket page, which
//! proves they control that address. Staff never see the code.

use std::sync::Arc;

use axum::http::Method;
use serde_json::{json, Value};

use super::{forbidden, json_response, AdminCtx, Resp};
use crate::state::AppState;
use mitch_lib::support as db;

const BASE: &str = "/api/admin/support/";

fn query_param(search: &str, key: &str) -> String {
    url::form_urlencoded::parse(search.trim_start_matches('?').as_bytes())
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

fn text(body: &Value, key: &str, max: usize) -> String {
    body.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .chars()
        .take(max)
        .collect()
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
    if !ctx.is_any_admin(state) {
        return Some(forbidden());
    }
    let staff = mitch_lib::auth::normalize_email(&ctx.email(state));
    let get = *method == Method::GET;
    let post = *method == Method::POST;
    Some(match rest {
        "tickets" if get => list(state, &query_param(search, "status")),
        "ticket" if get => detail(state, &query_param(search, "id")),
        "reply" if post => reply(state, body, &staff),
        "status" if post => set_status(state, body),
        "flag" if post => flag(state, body, &staff),
        "assign" if post => assign(state, body, &staff),
        "email-request" if post => request_email_change(state, body, &staff),
        "email-requests" if get => {
            if !ctx.is_admin(state) {
                return Some(forbidden());
            }
            pending_requests(state)
        }
        "email-request/decide" if post => {
            if !ctx.is_admin(state) {
                return Some(forbidden());
            }
            decide(state, body, &staff)
        }
        _ => return None,
    })
}

fn err(code: u16, message: &str) -> axum::response::Response {
    json_response(code, json!({ "error": message }))
}

fn ticket_json(state: &AppState, t: &db::Ticket) -> Value {
    let pending = db::email_requests_for_ticket(&state.store, &t.id)
        .into_iter()
        .any(|r| r.status == "pending");
    json!({
        "id": t.id,
        "status": t.status,
        "category": t.category,
        "subject": t.subject,
        "claimedAccount": t.claimed_account,
        "linked": !t.account_email.is_empty(),
        "contactEmail": t.contact_email,
        "compromised": t.compromised,
        "assignedTo": t.assigned_to,
        "createdAt": t.created_at,
        "updatedAt": t.updated_at,
        "emailRequestPending": pending,
    })
}

fn list(state: &Arc<AppState>, status: &str) -> axum::response::Response {
    let filter = if db::STATUSES.contains(&status) {
        Some(status)
    } else {
        None
    };
    let tickets: Vec<Value> = db::list_tickets(&state.store, filter, 200)
        .iter()
        .map(|t| ticket_json(state, t))
        .collect();
    json_response(200, json!({ "ok": true, "tickets": tickets }))
}

/// What staff may know about the account a ticket is linked to: no full email,
/// and whether an email change is even possible.
fn account_json(state: &Arc<AppState>, t: &db::Ticket) -> Value {
    if t.account_email.is_empty() {
        return Value::Null;
    }
    let profile = mitch_lib::profiles::profile_get(&state.store, &t.account_email);
    let change = crate::routes::me::account::check_email_change_allowed(state, &t.account_email);
    json!({
        "username": profile.get("username").cloned().unwrap_or(Value::Null),
        "displayName": profile.get("displayName").cloned().unwrap_or(Value::Null),
        "emailMasked": mitch_lib::admin::mask_email(&t.account_email),
        "emailChangeBlockedReason": change.err().map(|e| e.message()),
    })
}

fn detail(state: &Arc<AppState>, id: &str) -> axum::response::Response {
    let Some(t) = db::ticket(&state.store, id) else {
        return err(404, "Ticket not found.");
    };
    let messages: Vec<Value> = db::messages(&state.store, &t.id)
        .into_iter()
        .map(|m| {
            let who = if m.staff_email.is_empty() {
                Value::Null
            } else {
                let p = mitch_lib::profiles::profile_get(&state.store, &m.staff_email);
                p.get("username").cloned().unwrap_or(json!("staff"))
            };
            json!({ "id": m.id, "from": m.sender, "staff": who, "body": m.body, "at": m.created_at })
        })
        .collect();
    let requests: Vec<Value> = db::email_requests_for_ticket(&state.store, &t.id)
        .iter()
        .map(|r| request_json(state, r))
        .collect();
    json_response(
        200,
        json!({
            "ok": true,
            "ticket": ticket_json(state, &t),
            "account": account_json(state, &t),
            "messages": messages,
            "emailRequests": requests,
        }),
    )
}

fn request_json(state: &AppState, r: &db::EmailRequest) -> Value {
    let name = |email: &str| -> Value {
        if email.is_empty() {
            return Value::Null;
        }
        mitch_lib::profiles::profile_get(&state.store, email)
            .get("username")
            .cloned()
            .unwrap_or(json!(email))
    };
    json!({
        "id": r.id,
        "ticketId": r.ticket_id,
        "oldEmail": mitch_lib::admin::mask_email(&r.old_email),
        "newEmail": mitch_lib::admin::mask_email(&r.new_email),
        "reason": r.reason,
        "requestedBy": name(&r.requested_by),
        "requestedAt": r.requested_at,
        "status": r.status,
        "decidedBy": name(&r.decided_by),
        "decidedAt": r.decided_at,
        "decisionNote": r.decision_note,
    })
}

fn existing_ticket(state: &AppState, body: &Value) -> Option<db::Ticket> {
    db::ticket(&state.store, &text(body, "id", 64))
}

fn reply(state: &Arc<AppState>, body: &Value, staff: &str) -> axum::response::Response {
    let Some(t) = existing_ticket(state, body) else {
        return err(404, "Ticket not found.");
    };
    let message = text(body, "body", 4000);
    if message.is_empty() {
        return err(400, "Write a message first.");
    }
    db::add_message(&state.store, &t.id, "staff", staff, &message);
    if t.status == "open" {
        db::set_status(&state.store, &t.id, "pending");
    }
    if t.assigned_to.is_empty() {
        db::set_assigned(&state.store, &t.id, staff);
    }
    json_response(200, json!({ "ok": true }))
}

fn set_status(state: &Arc<AppState>, body: &Value) -> axum::response::Response {
    let Some(t) = existing_ticket(state, body) else {
        return err(404, "Ticket not found.");
    };
    if !db::set_status(&state.store, &t.id, &text(body, "status", 20)) {
        return err(400, "Unknown status.");
    }
    json_response(200, json!({ "ok": true }))
}

fn flag(state: &Arc<AppState>, body: &Value, staff: &str) -> axum::response::Response {
    let Some(t) = existing_ticket(state, body) else {
        return err(404, "Ticket not found.");
    };
    let on = body.get("compromised").and_then(Value::as_bool) == Some(true);
    db::set_compromised(&state.store, &t.id, on);
    if on != t.compromised {
        db::add_message(
            &state.store,
            &t.id,
            "system",
            "",
            if on {
                "Support marked this account as possibly compromised."
            } else {
                "Support cleared the compromised flag."
            },
        );
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            staff,
            if on {
                "support_flag_compromised"
            } else {
                "support_unflag_compromised"
            },
            json!({ "ticket": t.id }),
        );
    }
    json_response(200, json!({ "ok": true }))
}

fn assign(state: &Arc<AppState>, body: &Value, staff: &str) -> axum::response::Response {
    let Some(t) = existing_ticket(state, body) else {
        return err(404, "Ticket not found.");
    };
    let take = body.get("assign").and_then(Value::as_bool) != Some(false);
    db::set_assigned(&state.store, &t.id, if take { staff } else { "" });
    json_response(200, json!({ "ok": true }))
}

fn request_email_change(
    state: &Arc<AppState>,
    body: &Value,
    staff: &str,
) -> axum::response::Response {
    let Some(t) = existing_ticket(state, body) else {
        return err(404, "Ticket not found.");
    };
    if t.account_email.is_empty() {
        return err(400, "This ticket isn't linked to an account.");
    }
    let new_email = mitch_lib::auth::normalize_email(&text(body, "newEmail", 254));
    if !new_email.contains('@') || new_email.contains(char::is_whitespace) {
        return err(400, "Enter a valid new email address.");
    }
    if new_email == t.account_email {
        return err(400, "That is already the account's email.");
    }
    if mitch_lib::passwords::password_exists(&state.store, &new_email) {
        return err(400, "That email already has an account.");
    }
    if let Err(blocked) =
        crate::routes::me::account::check_email_change_allowed(state, &t.account_email)
    {
        return err(403, blocked.message());
    }
    let reason = text(body, "reason", 500);
    if reason.is_empty() {
        return err(400, "Say why: how you confirmed this is the account owner.");
    }
    match db::create_email_request(&state.store, &t.id, &new_email, &reason, staff) {
        Ok(id) => {
            db::add_message(
                &state.store,
                &t.id,
                "system",
                "",
                "An email change was requested. An admin will review it.",
            );
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                staff,
                "support_email_change_requested",
                json!({ "ticket": t.id, "request": id }),
            );
            json_response(200, json!({ "ok": true, "id": id }))
        }
        Err(e) => err(400, e.message()),
    }
}

fn pending_requests(state: &Arc<AppState>) -> axum::response::Response {
    let rows: Vec<Value> = db::email_requests_with_status(&state.store, "pending")
        .iter()
        .map(|r| {
            let mut j = request_json(state, r);
            if let Some(t) = db::ticket(&state.store, &r.ticket_id) {
                j["ticket"] = ticket_json(state, &t);
            }
            j
        })
        .collect();
    json_response(200, json!({ "ok": true, "requests": rows }))
}

fn decide(state: &Arc<AppState>, body: &Value, staff: &str) -> axum::response::Response {
    let id = body.get("requestId").and_then(Value::as_i64).unwrap_or(0);
    let approve = body.get("approve").and_then(Value::as_bool) == Some(true);
    let note = text(body, "note", 500);
    let Some(req) = db::email_request(&state.store, id) else {
        return err(404, "Request not found.");
    };
    if req.status != "pending" {
        return err(400, db::RequestError::NotPending.message());
    }
    // Conditions can change while a request waits; re-check before approving.
    if approve {
        if mitch_lib::passwords::password_exists(&state.store, &req.new_email) {
            return err(400, "That email now has an account.");
        }
        if let Err(blocked) =
            crate::routes::me::account::check_email_change_allowed(state, &req.old_email)
        {
            return err(403, blocked.message());
        }
    } else if note.is_empty() {
        return err(400, "Add a note explaining the denial.");
    }
    let code = format!("{:06}", rand::random::<u32>() % 1_000_000);
    let code_hash = mitch_lib::crypto::sha256_hex(format!("{}:{}", req.ticket_id, code).as_bytes());
    if let Err(e) = db::decide_email_request(&state.store, id, approve, staff, &note, &code_hash) {
        return err(
            if e == db::RequestError::OwnRequest {
                403
            } else {
                400
            },
            e.message(),
        );
    }
    if approve {
        let html = crate::routes::me::security::make_verification_code_html(
            state,
            "Confirm your new email",
            &code,
            30.0,
            &req.new_email,
        );
        crate::routes::push::send_email_bg(
            state,
            &req.new_email,
            "Confirm your new mitch.pro email",
            &html,
        );
        db::add_message(
            &state.store,
            &req.ticket_id,
            "system",
            "",
            &format!(
                "An admin approved your email change. We sent a code to {}. Enter it below within 30 minutes to finish.",
                mitch_lib::admin::mask_email(&req.new_email)
            ),
        );
    } else {
        db::add_message(
            &state.store,
            &req.ticket_id,
            "system",
            "",
            "Your email change request wasn't approved. Support will follow up here.",
        );
    }
    mitch_lib::admin::log_admin_action(
        &state.store,
        &state.cfg.data_dir,
        staff,
        if approve {
            "support_email_change_approved"
        } else {
            "support_email_change_denied"
        },
        json!({ "ticket": req.ticket_id, "request": id }),
    );
    json_response(200, json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    fn test_state() -> (Arc<AppState>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "mitch-server-admin-support-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap();
        let cfg = crate::hosts::SiteConfig {
            data_dir: dir.join("data"),
            ..crate::hosts::SiteConfig::load()
        };
        let store = Arc::new(mitch_lib::data::DataStore::open(&dir, &dir.join("data")).unwrap());
        (Arc::new(AppState::new(cfg, store)), dir)
    }

    async fn status_of(resp: axum::response::Response) -> (u16, Value) {
        let status = resp.status().as_u16();
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(json!({})))
    }

    fn linked_ticket(state: &AppState, account: &str) -> String {
        db::create_ticket(
            &state.store,
            &db::NewTicket {
                token_hash: "h",
                ip_hash: "ip",
                category: "email",
                subject: "s",
                claimed_account: "user",
                account_email: account,
                contact_email: "",
            },
        )
        .unwrap()
    }

    #[tokio::test]
    async fn requests_follow_the_rules_and_nobody_approves_their_own() {
        let (state, dir) = test_state();
        state
            .store
            .write_document(
                &dir.join("data/admins.json"),
                &json!({ "owners": [], "admins": ["admin@x.test"] }),
            )
            .unwrap();
        mitch_lib::passwords::password_set(&state.store, "user@x.test", "h");
        mitch_lib::passwords::password_set(&state.store, "taken@x.test", "h");
        mitch_lib::passwords::password_set(&state.store, "admin@x.test", "h");
        let id = linked_ticket(&state, "user@x.test");
        let body = |new: &str, reason: &str| json!({ "id": id, "newEmail": new, "reason": reason });

        // A reason is required, the new address must be free and valid.
        assert_eq!(
            status_of(request_email_change(
                &state,
                &body("n@x.test", ""),
                "mod@x.test"
            ))
            .await
            .0,
            400
        );
        assert_eq!(
            status_of(request_email_change(
                &state,
                &body("taken@x.test", "r"),
                "mod@x.test"
            ))
            .await
            .0,
            400
        );
        assert_eq!(
            status_of(request_email_change(
                &state,
                &body("nope", "r"),
                "mod@x.test"
            ))
            .await
            .0,
            400
        );
        // Staff accounts can't be the subject.
        let staff_ticket = linked_ticket(&state, "admin@x.test");
        let staff_body = json!({ "id": staff_ticket, "newEmail": "n@x.test", "reason": "r" });
        assert_eq!(
            status_of(request_email_change(&state, &staff_body, "mod@x.test"))
                .await
                .0,
            403
        );

        let (code, v) = status_of(request_email_change(
            &state,
            &body("n@x.test", "asked 3 questions"),
            "admin@x.test",
        ))
        .await;
        assert_eq!(code, 200);
        let request_id = v["id"].as_i64().unwrap();
        // A second open request is refused.
        assert_eq!(
            status_of(request_email_change(
                &state,
                &body("m@x.test", "r"),
                "mod@x.test"
            ))
            .await
            .0,
            400
        );
        // The requester can't decide it (even an admin), another admin can.
        let decide_body = json!({ "requestId": request_id, "approve": false, "note": "no" });
        assert_eq!(
            status_of(decide(&state, &decide_body, "ADMIN@x.test"))
                .await
                .0,
            403
        );
        assert_eq!(
            status_of(decide(
                &state,
                &json!({ "requestId": request_id, "approve": false }),
                "admin2@x.test"
            ))
            .await
            .0,
            400,
            "a denial needs a note"
        );
        assert_eq!(
            status_of(decide(&state, &decide_body, "admin2@x.test"))
                .await
                .0,
            200
        );
        assert_eq!(
            status_of(decide(&state, &decide_body, "admin2@x.test"))
                .await
                .0,
            400,
            "already decided"
        );
        std::fs::remove_dir_all(dir).ok();
    }
}
