use super::{forbidden, json_response, AdminCtx, Resp};
use crate::state::AppState;
use axum::http::Method;
use mitch_lib::chat_cleanup;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    body: &Value,
    ctx: &AdminCtx,
) -> Resp {
    if path != "/api/admin/chat/history-reset" {
        return None;
    }
    let actor = ctx.email(state);
    if !ctx.is_owner(state) && !mitch_lib::auth::is_co_owner_email(&state.store, &actor) {
        return Some(forbidden());
    }
    if *method == Method::POST {
        let request_id = body["requestId"].as_str().unwrap_or("");
        if body["confirmation"] != "CLEAR ALL CHAT HISTORY"
            || !(16..=128).contains(&request_id.len())
            || !request_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Some(json_response(
                400,
                json!({"error":"Type CLEAR ALL CHAT HISTORY to confirm."}),
            ));
        }
        let cutoff = mitch_lib::school::now_millis();
        if let Err(error) = chat_cleanup::reset_all(&state.store, &actor, request_id, cutoff) {
            tracing::error!("Chat history reset failed: {error}");
            return Some(json_response(
                500,
                json!({"error":"Chat history was not cleared. Try again."}),
            ));
        }
        let cutoff = chat_cleanup::history_reset_cutoff(&state.store);
        state
            .e2e_messages
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values_mut()
            .for_each(|messages| messages.retain(|message| message.timestamp > cutoff));
        let _ = chat_cleanup::remove_files(&state.store);
        mitch_lib::admin::log_admin_action(
            &state.store,
            state.data_dir(),
            &actor,
            "clear_all_chat_history",
            json!({"cutoff":cutoff,"requestId":request_id}),
        );
    } else if *method != Method::GET {
        return Some(json_response(405, json!({"error":"Method not allowed."})));
    }
    let jobs = chat_cleanup::statuses(&state.store);
    let pending = jobs
        .as_object()
        .map(|map| map.values().filter(|job| job["complete"] != true).count())
        .unwrap_or(0);
    let errors = jobs
        .as_object()
        .map(|map| {
            map.values()
                .filter(|job| !job["error"].as_str().unwrap_or("").is_empty())
                .count()
        })
        .unwrap_or(0);
    let files = chat_cleanup::pending_file_count(&state.store);
    Some(json_response(
        200,
        json!({"supportsHistoryReset":true,"lastReset":chat_cleanup::history_reset_status(&state.store),"pendingMatrixAccounts":pending,"accountsNeedingRetry":errors,"pendingFiles":files}),
    ))
}
