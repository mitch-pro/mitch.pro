//! Backend of the unified staff dashboard (`/staff-dashboard/`): one summary of
//! everything waiting on staff, and the Matrix report queue. Overview and rules
//! are in `docs/staff-dashboard.md`.
//!
//! - `GET /api/admin/staff-dashboard/summary` (moderators and above): counts plus
//!   the unresolved items of every queue. Admin-only queues (moderator requests,
//!   email-change requests) are left out for moderators, not just hidden.
//! - `GET /api/admin/staff-dashboard/matrix-reports?filter=open|closed|all`: the
//!   Matrix report list. Open reports that lack the sender or text are looked up
//!   in Matrix first (best effort).
//! - `POST /api/admin/staff-dashboard/matrix-reports/status {id, status, note}`:
//!   resolve, dismiss or reopen a report. Acting on the reported message or its
//!   author uses the existing `/api/matrix/moderation/*` endpoints.
//!
//! - `GET .../logs/actions`, `.../logs/coins` and `.../logs/other` (full admins):
//!   searchable activity and coin logs, and the notification, ban and anti-cheat
//!   lists for the Logs page. These replace `/api/admin/advanced-data` there, which
//!   only returns the newest 250 rows and writes a "view_admin_tools" entry on
//!   every call.
//!
//! Reads only; the other queues are changed through their own endpoints, so their
//! rules (admin-only deletes, passphrase gate) stay in one place.

use std::sync::Arc;

use axum::http::Method;
use serde_json::{json, Value};

use super::{forbidden, json_response, AdminCtx, Resp};
use crate::routes::matrix::{
    enrich_matrix_report, matrix_report_json, migrate_legacy_matrix_reports,
};
use crate::state::AppState;
use mitch_lib::{feedback, matrix_reports as reports, support, teachers};

const BASE: &str = "/api/admin/staff-dashboard/";
/// Most items of one queue the summary carries; the count is always the full one.
const QUEUE_LIMIT: usize = 50;
/// Most reports looked up in Matrix per list call.
const ENRICH_LIMIT: usize = 15;

fn query_param(search: &str, key: &str) -> String {
    url::form_urlencoded::parse(search.trim_start_matches('?').as_bytes())
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

pub async fn handle(
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
    Some(match (method.as_str(), rest) {
        ("GET", "summary") => summary(state, ctx),
        ("GET", "matrix-reports") => matrix_reports(state, &query_param(search, "filter")).await,
        ("POST", "matrix-reports/status") => set_report_status(state, body, ctx),
        ("GET", "logs/actions" | "logs/coins" | "logs/other") => {
            if !ctx.is_admin(state) {
                return Some(forbidden());
            }
            match rest {
                "logs/actions" => search_actions(state, search, ctx),
                "logs/coins" => search_coins(state, search, ctx),
                _ => other_logs(state),
            }
        }
        _ => return None,
    })
}

/// Epoch-millisecond query parameter, if present and numeric.
fn ms_param(search: &str, key: &str) -> Option<i64> {
    query_param(search, key).trim().parse().ok()
}

/// Rows per search, clamped so one request stays small.
fn limit_param(search: &str) -> usize {
    query_param(search, "limit")
        .trim()
        .parse()
        .unwrap_or(100)
        .clamp(1, 500)
}

/// Writes "<staff> looked up <person> in <log>" to the admin activity log, so
/// looking into someone's history is itself visible. Repeats of the same lookup
/// by the same staff member within ten minutes (paging, re-sorting) are skipped.
fn record_lookup(state: &Arc<AppState>, ctx: &AdminCtx, log: &str, person: &str) {
    let person = person.trim().to_lowercase();
    if person.is_empty() {
        return;
    }
    let actor = ctx.email(state);
    let now = mitch_lib::school::now_millis();
    let recent = mitch_lib::admin::admin_actions_with_prefix(&state.store, "log_lookup", 50)
        .into_iter()
        .any(|r| {
            r["actor"].as_str() == Some(actor.as_str())
                && r["details"]["log"].as_str() == Some(log)
                && r["details"]["person"].as_str() == Some(person.as_str())
                && now - r["ts"].as_i64().unwrap_or(0) < 10 * 60_000
        });
    if !recent {
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &actor,
            "log_lookup",
            json!({ "log": log, "person": person }),
        );
    }
}

/// `GET .../logs/actions?person=&action=&from=&to=&limit=` (`from`/`to` are epoch ms).
fn search_actions(state: &Arc<AppState>, search: &str, ctx: &AdminCtx) -> axum::response::Response {
    let (person, action) = (query_param(search, "person"), query_param(search, "action"));
    record_lookup(state, ctx, "activity", &person);
    let q = mitch_lib::admin::ActionQuery {
        person: &person,
        action: &action,
        from: ms_param(search, "from"),
        to: ms_param(search, "to"),
    };
    let (entries, total) =
        mitch_lib::admin::search_admin_actions(&state.store, &q, limit_param(search) as i64);
    json_response(
        200,
        json!({
            "ok": true,
            "entries": entries,
            "total": total,
            "actions": mitch_lib::admin::admin_action_names(&state.store),
        }),
    )
}

/// `GET .../logs/coins?person=&reason=&from=&to=&min=&direction=&limit=`.
fn search_coins(state: &Arc<AppState>, search: &str, ctx: &AdminCtx) -> axum::response::Response {
    let (person, reason, direction) = (
        query_param(search, "person"),
        query_param(search, "reason"),
        query_param(search, "direction"),
    );
    let q = mitch_lib::coins::CoinQuery {
        person: &person,
        reason: &reason,
        from: ms_param(search, "from"),
        to: ms_param(search, "to"),
        min_abs: query_param(search, "min").trim().parse().unwrap_or(0.0),
        direction: &direction,
    };
    record_lookup(state, ctx, "coins", &person);
    let (entries, total) =
        mitch_lib::coins::search_coin_log(&state.cfg.data_dir, &q, limit_param(search));
    json_response(
        200,
        json!({ "ok": true, "entries": entries, "total": total }),
    )
}

/// `GET .../logs/other`: sent notifications, banned accounts and anti-cheat flags.
fn other_logs(state: &Arc<AppState>) -> axum::response::Response {
    let store = &state.store;
    let mut bans = mitch_lib::bans::blacklist_all(store);
    bans.sort_by_key(|b| std::cmp::Reverse(b.get("bannedAt").and_then(Value::as_i64).unwrap_or(0)));
    let (cheats, _) = mitch_lib::admin::cheat_logs_recent(store, 100);
    json_response(
        200,
        json!({
            "ok": true,
            "sentNotifications": mitch_lib::admin::sent_notifications(store).into_iter().take(100).collect::<Vec<_>>(),
            "bannedAccounts": bans.into_iter().take(100).collect::<Vec<_>>(),
            "cheatLogs": cheats,
        }),
    )
}

/// True for a report/ticket status that still needs staff.
fn is_open_status(status: &str) -> bool {
    !matches!(
        status.to_ascii_lowercase().as_str(),
        "resolved" | "dismissed" | "handled" | "approved" | "rejected" | "denied"
    )
}

fn is_matrix_entry(r: &Value) -> bool {
    r.get("matrixRoomId").is_some()
        || r.get("id")
            .and_then(Value::as_str)
            .is_some_and(|s| s.starts_with("matrix-"))
}

fn summary(state: &Arc<AppState>, ctx: &AdminCtx) -> axum::response::Response {
    let is_admin = ctx.is_admin(state);
    let store = &state.store;
    let data_dir = &state.cfg.data_dir;

    migrate_legacy_matrix_reports(state);
    let matrix: Vec<Value> = reports::list(store, "open", QUEUE_LIMIT as i64)
        .iter()
        .map(matrix_report_json)
        .collect();
    let matrix_count = reports::open_count(store);

    // Reports from direct messages and the old chat; oldest first.
    let chat_all: Vec<Value> = store
        .read_document(&data_dir.join("chat_reports.json"), json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let chat_open: Vec<&Value> = chat_all
        .iter()
        .filter(|r| {
            !is_matrix_entry(r)
                && is_open_status(r.get("status").and_then(Value::as_str).unwrap_or(""))
        })
        .collect();
    let chat: Vec<Value> = chat_open
        .iter()
        .take(QUEUE_LIMIT)
        .map(|r| {
            json!({
                "ts": r.get("ts").and_then(Value::as_i64).unwrap_or(0),
                "reportedBy": r.get("reportedBy").or_else(|| r.get("reporter")).and_then(Value::as_str).unwrap_or(""),
                "reason": r.get("reason").and_then(Value::as_str).unwrap_or("Chat report"),
                "context": r.get("context").cloned().unwrap_or(json!([])),
            })
        })
        .collect();

    let profile: Vec<Value> = mitch_lib::admin::public_profile_reports(store, data_dir, 1000)
        .into_iter()
        .filter(|r| is_open_status(r.get("status").and_then(Value::as_str).unwrap_or("")))
        .collect();

    let outside = mitch_lib::matrix::outside_approvals_pending(store);

    // The old suggestions file is folded in the first time anyone looks.
    let legacy = store.read_document(&data_dir.join("suggestions.json"), json!([]));
    feedback::import_legacy(store, &legacy);
    let (feedback_new, _) = feedback::counts(store);
    let feedback_items: Vec<Value> = feedback::list(store, "new")
        .iter()
        .rev()
        .take(QUEUE_LIMIT)
        .map(|e| {
            json!({ "id": e.id, "type": e.kind, "text": e.text, "account": e.account, "createdAt": e.created_at })
        })
        .collect();

    let (notes_pending, _, _) = teachers::note_counts(store);
    let notes: Vec<Value> = teachers::list_notes(store, "pending")
        .iter()
        .take(QUEUE_LIMIT)
        .map(|n| {
            json!({
                "id": n.id,
                "teacher": n.teacher_name,
                "school": crate::routes::teachers::school_name(&n.school).unwrap_or(&n.school),
                "text": n.text,
                "createdAt": n.day * 86_400_000,
            })
        })
        .collect();

    // Tickets waiting on staff, oldest activity first.
    let mut tickets: Vec<support::Ticket> = support::list_tickets(store, Some("open"), 200);
    tickets.sort_by_key(|t| t.updated_at);
    let ticket_items: Vec<Value> = tickets
        .iter()
        .take(QUEUE_LIMIT)
        .map(|t| {
            json!({
                "id": t.id,
                "category": t.category,
                "subject": t.subject,
                "claimedAccount": t.claimed_account,
                "compromised": t.compromised,
                "assignedTo": t.assigned_to,
                "createdAt": t.created_at,
                "updatedAt": t.updated_at,
            })
        })
        .collect();

    let (email_requests, moderator_requests): (Vec<Value>, Vec<Value>) = if is_admin {
        (
            support::email_requests_with_status(store, "pending")
                .iter()
                .map(|r| {
                    json!({
                        "id": r.id,
                        "ticketId": r.ticket_id,
                        "reason": r.reason,
                        "requestedBy": r.requested_by,
                        "requestedAt": r.requested_at,
                    })
                })
                .collect(),
            mitch_lib::admin::load_moderator_requests(store, data_dir)
                .iter()
                .map(mitch_lib::admin::public_moderator_request)
                .filter(|r| r.get("status").and_then(Value::as_str) == Some("pending"))
                .collect(),
        )
    } else {
        (Vec::new(), Vec::new())
    };

    let counts = json!({
        "matrixReports": matrix_count,
        "chatReports": chat_open.len(),
        "profileReports": profile.len(),
        "outsideAccounts": outside.len(),
        "supportTickets": tickets.len(),
        "emailRequests": email_requests.len(),
        "feedback": feedback_new,
        "teacherNotes": notes_pending,
        "moderatorRequests": moderator_requests.len(),
    });
    let total: i64 = counts
        .as_object()
        .map(|o| o.values().filter_map(Value::as_i64).sum())
        .unwrap_or(0);
    json_response(
        200,
        json!({
            "ok": true,
            "viewer": {
                "isAdmin": is_admin,
                "isOwner": ctx.is_owner(state),
            },
            "total": total,
            "counts": counts,
            "queues": {
                "matrixReports": matrix,
                "chatReports": chat,
                "profileReports": profile.into_iter().take(QUEUE_LIMIT).collect::<Vec<_>>(),
                "outsideAccounts": outside.into_iter().take(QUEUE_LIMIT).collect::<Vec<_>>(),
                "supportTickets": ticket_items,
                "emailRequests": email_requests,
                "feedback": feedback_items,
                "teacherNotes": notes,
                "moderatorRequests": moderator_requests,
            },
        }),
    )
}

async fn matrix_reports(state: &Arc<AppState>, filter: &str) -> axum::response::Response {
    migrate_legacy_matrix_reports(state);
    let filter = match filter {
        "closed" | "all" => filter,
        _ => "open",
    };
    let mut list = reports::list(&state.store, filter, reports::LIST_LIMIT);
    for r in list
        .iter_mut()
        .filter(|r| !r.event_id.is_empty() && (r.sender.is_empty() || r.body.is_empty()))
        .take(ENRICH_LIMIT)
    {
        enrich_matrix_report(
            &state.id_secret,
            &state.store,
            &r.id,
            &r.room_id,
            &r.event_id,
        )
        .await;
        if let Some(fresh) = reports::get(&state.store, &r.id) {
            *r = fresh;
        }
    }
    json_response(
        200,
        json!({
            "ok": true,
            "reports": list.iter().map(matrix_report_json).collect::<Vec<_>>(),
            "open": reports::open_count(&state.store),
        }),
    )
}

fn set_report_status(
    state: &Arc<AppState>,
    body: &Value,
    ctx: &AdminCtx,
) -> axum::response::Response {
    let id = body.get("id").and_then(Value::as_str).unwrap_or("");
    let status = body.get("status").and_then(Value::as_str).unwrap_or("");
    let note = body.get("note").and_then(Value::as_str).unwrap_or("");
    let actor = ctx.email(state);
    if reports::set_status(
        &state.store,
        id,
        status,
        &actor,
        note,
        mitch_lib::school::now_millis(),
    ) {
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &actor,
            "matrix_report_status",
            json!({ "id": id, "status": status }),
        );
        json_response(
            200,
            json!({ "ok": true, "open": reports::open_count(&state.store) }),
        )
    } else {
        json_response(404, json!({ "error": "That report no longer exists." }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_status_treats_missing_and_review_states_as_open() {
        assert!(is_open_status(""));
        assert!(is_open_status("Needs review"));
        assert!(!is_open_status("Resolved"));
        assert!(!is_open_status("dismissed"));
    }

    #[test]
    fn matrix_entries_are_recognised_by_room_or_id() {
        assert!(is_matrix_entry(&json!({ "matrixRoomId": "!r:x" })));
        assert!(is_matrix_entry(&json!({ "id": "matrix-$e" })));
        assert!(!is_matrix_entry(&json!({ "id": "dm-1" })));
    }
}
