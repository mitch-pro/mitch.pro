//! Economy & privileged fan-out — coin multiplier (10582), casino toggle/rig
//! (10593/10191), grant-premium (10062), gift-coins (10104), economy audit/
//! burn (10142-10172), broadcast (10174), send/unsend notification
//! (10812-10910), revoke-premium (10912), trigger-daily-summary (9876).
//! Shared helpers are reused by the moderator request engine in moderation.rs.

use super::{forbidden, unauthorized, AdminCtx, Resp};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body: &Value,
    ctx: &AdminCtx,
) -> Resp {
    // POST /api/admin/economy/multiplier (admins only).
    if path == "/api/admin/economy/multiplier" {
        if !ctx.is_admin(state) {
            return Some(forbidden());
        }
        let multiplier = body
            .get("multiplier")
            .and_then(|v| v.as_str())
            .and_then(parse_f64)
            .or_else(|| body.get("multiplier").and_then(|v| v.as_f64()))
            .unwrap_or(1.0);
        state.set_coin_multiplier(multiplier);
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &ctx.email(state),
            "set_multiplier",
            json!({ "multiplier": multiplier }),
        );
        return Some(json_response(
            200,
            json!({ "ok": true, "multiplier": multiplier }),
        ));
    }

    // POST /api/admin/casino/toggle (admins only).
    if path == "/api/admin/casino/toggle" {
        if !ctx.is_admin(state) {
            return Some(forbidden());
        }
        let enabled = !state
            .casino_enabled
            .load(std::sync::atomic::Ordering::Relaxed);
        state
            .casino_enabled
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &ctx.email(state),
            "toggle_casino",
            json!({ "enabled": enabled }),
        );
        return Some(json_response(
            200,
            json!({ "ok": true, "enabled": enabled }),
        ));
    }

    // POST /api/admin/casino/rig (admins only).
    if path == "/api/admin/casino/rig" && *method == Method::POST {
        if !ctx.is_admin(state) {
            return Some(forbidden());
        }
        let chance = body
            .get("chance")
            .and_then(|v| v.as_str())
            .and_then(parse_f64)
            .or_else(|| body.get("chance").and_then(|v| v.as_f64()));
        let Some(chance) = chance.filter(|c| c.is_finite() && (0.0..=100.0).contains(c)) else {
            return Some(json_response(400, json!({ "error": "invalid chance" })));
        };
        state
            .casino_rig_chance
            .store(chance.to_bits(), std::sync::atomic::Ordering::Relaxed);
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &ctx.email(state),
            "set_casino_rig",
            json!({ "chance": chance }),
        );
        return Some(json_response(200, json!({ "ok": true, "chance": chance })));
    }

    // POST /api/admin/trigger-daily-summary (admins; 401 on failure per JS).
    if path == "/api/admin/trigger-daily-summary" && *method == Method::POST {
        if state.rate_limit_check(&ctx.ip, "anon", path).is_some() {
            return Some(json_response(
                429,
                json!({ "error": "Too many requests, slow down" }),
            ));
        }
        if !valid_id(&ctx.sid, state) || !ctx.is_admin(state) {
            return Some(unauthorized());
        }
        let summary = build_daily_summary(state);
        crate::routes::push::ntfy_notify(&summary, "Daily Traffic Summary", "default");
        return Some(json_response(
            200,
            json!({ "success": true, "message": "Daily traffic summary notification triggered successfully." }),
        ));
    }

    // POST /api/admin/grant-premium (grant-admins only).
    if path == "/api/admin/grant-premium" && *method == Method::POST {
        if state.rate_limit_check(&ctx.ip, "anon", path).is_some() {
            return Some(json_response(
                429,
                json!({ "error": "Too many requests, slow down" }),
            ));
        }
        if !valid_id(&ctx.sid, state) {
            return Some(unauthorized());
        }
        if !ctx.can_grant_premium(state) {
            return Some(forbidden());
        }
        let admin_email = ctx.email(state);
        let target_raw = body
            .get("targetEmail")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase()
            .trim()
            .to_string();
        let target_email = mitch_lib::auth::normalize_email(&target_raw);
        let reason = body
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("free premium granted by admin")
            .trim()
            .chars()
            .take(200)
            .collect::<String>();
        if !valid_email(&target_raw) {
            return Some(json_response(
                400,
                json!({ "error": "valid target email required" }),
            ));
        }
        if is_premium_email(state, &target_email) {
            return Some(json_response(
                400,
                json!({ "error": "That user is already Premium." }),
            ));
        }
        if reason.is_empty() {
            return Some(json_response(400, json!({ "error": "reason required" })));
        }
        grant_premium_application(state, &target_raw, &admin_email, &reason);
        let notice_message = format!("{admin_email} granted you Premium. Reason: {reason}");
        mitch_lib::coins::add_admin_notification(
            &state.store,
            &state.cfg.data_dir,
            &target_raw,
            "Premium granted",
            &notice_message,
            &admin_email,
            "",
            "",
        );
        crate::routes::push::push_admin_notification(
            state,
            &target_raw,
            "Premium granted",
            &notice_message,
        );
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &admin_email,
            "grant_premium",
            json!({ "targetEmail": target_email, "reason": reason }),
        );
        return Some(json_response(
            200,
            json!({ "ok": true, "targetEmail": target_raw }),
        ));
    }

    // POST /api/admin/gift-coins (admins only).
    if path == "/api/admin/gift-coins" && *method == Method::POST {
        if state.rate_limit_check(&ctx.ip, "anon", path).is_some() {
            return Some(json_response(
                429,
                json!({ "error": "Too many requests, slow down" }),
            ));
        }
        if !valid_id(&ctx.sid, state) {
            return Some(unauthorized());
        }
        if !ctx.is_admin(state) {
            return Some(forbidden());
        }
        let admin_email = ctx.email(state);
        let target_raw = body
            .get("targetEmail")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase()
            .trim()
            .to_string();
        let target_email = mitch_lib::auth::normalize_email(&target_raw);
        let amount = body.get("amount").and_then(|v| v.as_f64());
        let reason = body
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("admin gift")
            .trim()
            .chars()
            .take(160)
            .collect::<String>();
        if !valid_email(&target_raw) {
            return Some(json_response(
                400,
                json!({ "error": "valid target email required" }),
            ));
        }
        let Some(amount) = amount.filter(|a| a.is_finite() && *a > 0.0) else {
            return Some(json_response(
                400,
                json!({ "error": "amount must be a positive number" }),
            ));
        };
        if amount > 1_000_000_000.0 {
            return Some(json_response(
                400,
                json!({ "error": "amount is too large" }),
            ));
        }
        mitch_lib::coins::add_coins(
            &state.store,
            &state.cfg.data_dir,
            &target_raw,
            amount,
            state.coin_multiplier(),
            "",
        );
        mitch_lib::coins::add_coin_gift_notice(
            &state.store,
            &state.cfg.data_dir,
            &target_raw,
            amount,
            &admin_email,
            &reason,
        );
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &admin_email,
            "gift_coins",
            json!({ "targetEmail": target_email, "targetRaw": target_raw, "amount": mitch_lib::jsval::num_value(amount), "reason": reason }),
        );
        return Some(json_response(
            200,
            json!({
                "ok": true,
                "targetEmail": target_raw,
                "amount": mitch_lib::jsval::num_value(amount),
                "newBalance": mitch_lib::jsval::num_value(mitch_lib::coins::get_coins(&state.store, &state.cfg.data_dir, &target_raw)),
            }),
        ));
    }

    // GET /api/admin/economy/audit.
    if path == "/api/admin/economy/audit" {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        let coins = mitch_lib::coins::load_coins(&state.store, &state.cfg.data_dir);
        let mut sorted: Vec<(String, f64)> = coins
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.as_f64().unwrap_or(0.0)))
                    .collect()
            })
            .unwrap_or_default();
        sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let total: f64 = sorted.iter().map(|e| e.1).sum();
        let top1_count = ((sorted.len() as f64 * 0.01).ceil() as usize).max(1);
        let top1_sum: f64 = sorted.iter().take(top1_count).map(|e| e.1).sum();
        return Some(json_response(
            200,
            json!({
                "total": total,
                "topPercent": if total > 0.0 { top1_sum / total * 100.0 } else { 0.0 },
                "richestUser": sorted.first().map(|e| e.0.clone()).unwrap_or_else(|| "none".into()),
            }),
        ));
    }

    // POST /api/admin/economy/burn (admins only).
    if path == "/api/admin/economy/burn" {
        if !ctx.is_admin(state) {
            return Some(forbidden());
        }
        let admin_email = ctx.email(state);
        let email = body.get("email").cloned().unwrap_or(Value::Null);
        let target = mitch_lib::auth::normalize_email(email.as_str().unwrap_or(""));
        let amount = body
            .get("amount")
            .and_then(|v| v.as_str())
            .and_then(parse_f64)
            .or_else(|| body.get("amount").and_then(|v| v.as_f64()))
            .unwrap_or(f64::NAN);
        mitch_lib::coins::add_coins(
            &state.store,
            &state.cfg.data_dir,
            &target,
            -amount,
            state.coin_multiplier(),
            "",
        );
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &admin_email,
            "burn_coins",
            json!({ "target": target, "amount": mitch_lib::jsval::num_value(amount) }),
        );
        return Some(json_response(200, json!({ "ok": true })));
    }

    // POST /api/admin/broadcast (any admin/moderator).
    if path == "/api/admin/broadcast" {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        let typ_str = body
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("normal");
        let is_jumpscare = typ_str == "jumpscare";
        let msg_str = body
            .get("msg")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if !is_jumpscare && msg_str.is_empty() {
            return Some(json_response(400, json!({ "error": "message required" })));
        }
        let msg_truncated = &msg_str[..msg_str.len().min(500)];
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let random_hex = mitch_lib::crypto::random_bytes_hex(6);
        let event = crate::state::AdminBroadcastEvent {
            broadcast_id: format!("{}-{random_hex}", mitch_lib::crypto::to_base36(now)),
            event_type: if is_jumpscare {
                "admin_jumpscare".to_string()
            } else {
                "admin_broadcast".to_string()
            },
            message: msg_truncated.to_string(),
            created_at: now as i64,
            expires_at: (now + 5 * 60 * 1000) as i64,
        };
        {
            let mut guard = state
                .latest_admin_broadcast
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *guard = Some(event.clone());
        }
        let payload_str = serde_json::to_string(&event).unwrap_or_default();
        crate::ws::broadcast(state, crate::ws::WsRecipients::All, payload_str);
        let recipients = state.ws_broadcasts.lock().map(|m| m.len()).unwrap_or(0);
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &ctx.email(state),
            if is_jumpscare {
                "jumpscare"
            } else {
                "broadcast"
            },
            json!({ "message": msg_truncated }),
        );
        return Some(json_response(
            200,
            json!({
                "ok": true,
                "recipients": recipients,
                "broadcastId": event.broadcast_id,
                "expiresAt": event.expires_at,
            }),
        ));
    }

    // POST /api/admin/send-notification (any admin/moderator).
    if path == "/api/admin/send-notification" && *method == Method::POST {
        if state.rate_limit_check(&ctx.ip, "anon", path).is_some() {
            return Some(json_response(
                429,
                json!({ "error": "Too many requests, slow down" }),
            ));
        }
        if !valid_id(&ctx.sid, state) {
            return Some(unauthorized());
        }
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        let admin_email = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &ctx.sid)
            .unwrap_or_else(|| "moderator".to_string());
        let all_users = body.get("allUsers").and_then(|v| v.as_bool()) == Some(true);
        let target_raw = body
            .get("targetEmail")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase()
            .trim()
            .to_string();
        let title = body
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Admin notification")
            .trim()
            .chars()
            .take(80)
            .collect::<String>();
        let message = body
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .chars()
            .take(1000)
            .collect::<String>();
        let batch_id = mitch_lib::crypto::random_bytes_hex(10);
        if !all_users && !valid_email(&target_raw) {
            return Some(json_response(
                400,
                json!({ "error": "valid target email required" }),
            ));
        }
        if message.is_empty() {
            return Some(json_response(400, json!({ "error": "message required" })));
        }
        let role = if ctx.is_admin(state) {
            "admin"
        } else {
            "moderator"
        };
        let title = if title.is_empty() {
            "Admin notification".to_string()
        } else {
            title
        };
        let user_agent = header_or_unknown(headers, "user-agent");
        if all_users {
            let targets = token_email_targets(state);
            for email in &targets {
                mitch_lib::coins::add_admin_notification(
                    &state.store,
                    &state.cfg.data_dir,
                    email,
                    &title,
                    &message,
                    &admin_email,
                    &batch_id,
                    "",
                );
                crate::routes::push::push_admin_notification(state, email, &title, &message);
            }
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                &admin_email,
                "send_notification_all",
                json!({
                    "count": targets.len(),
                    "title": title,
                    "batchId": batch_id,
                    "ip": ctx.ip,
                    "userAgent": user_agent,
                    "role": role,
                }),
            );
            return Some(json_response(
                200,
                json!({ "ok": true, "allUsers": true, "count": targets.len(), "batchId": batch_id }),
            ));
        }
        let notice = mitch_lib::coins::add_admin_notification(
            &state.store,
            &state.cfg.data_dir,
            &target_raw,
            &title,
            &message,
            &admin_email,
            &batch_id,
            "",
        );
        crate::routes::push::push_admin_notification(state, &target_raw, &title, &message);
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &admin_email,
            "send_notification",
            json!({
                "targetEmail": target_raw,
                "title": title,
                "notificationId": notice.as_ref().and_then(|n| n.get("id")).and_then(|v| v.as_str()).unwrap_or(""),
                "batchId": batch_id,
                "ip": ctx.ip,
                "userAgent": user_agent,
                "role": role,
            }),
        );
        return Some(json_response(
            200,
            json!({
                "ok": true,
                "targetEmail": target_raw,
                "notificationId": notice.as_ref().and_then(|n| n.get("id")).and_then(|v| v.as_str()).unwrap_or(""),
                "batchId": batch_id,
            }),
        ));
    }

    // POST /api/admin/unsend-notification.
    if path == "/api/admin/unsend-notification" && *method == Method::POST {
        if state.rate_limit_check(&ctx.ip, "anon", path).is_some() {
            return Some(json_response(
                429,
                json!({ "error": "Too many requests, slow down" }),
            ));
        }
        if !valid_id(&ctx.sid, state) {
            return Some(unauthorized());
        }
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        let id = body
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let batch_id = body
            .get("batchId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if id.is_empty() && batch_id.is_empty() {
            return Some(json_response(
                400,
                json!({ "error": "notification id or batch id required" }),
            ));
        }
        let admin_email = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &ctx.sid)
            .unwrap_or_else(|| "moderator".to_string());
        let role = if ctx.is_admin(state) {
            "admin"
        } else {
            "moderator"
        };
        let removed = unsend_notification(state, &id, &batch_id);
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &admin_email,
            "unsend_notification",
            json!({
                "id": id,
                "batchId": batch_id,
                "removed": removed,
                "ip": ctx.ip,
                "userAgent": header_or_unknown(headers, "user-agent"),
                "role": role,
            }),
        );
        return Some(json_response(
            200,
            json!({ "ok": true, "removed": removed }),
        ));
    }

    // POST /api/admin/revoke-premium (grant-admins only).
    if path == "/api/admin/revoke-premium" && *method == Method::POST {
        if state.rate_limit_check(&ctx.ip, "anon", path).is_some() {
            return Some(json_response(
                429,
                json!({ "error": "Too many requests, slow down" }),
            ));
        }
        if !valid_id(&ctx.sid, state) {
            return Some(unauthorized());
        }
        if !ctx.can_grant_premium(state) {
            return Some(forbidden());
        }
        let admin_email = ctx.email(state);
        let email_raw = body
            .get("email")
            .or_else(|| body.get("targetEmail"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase()
            .trim()
            .to_string();
        let reason = body
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("premium revoked by admin")
            .trim()
            .chars()
            .take(200)
            .collect::<String>();
        if !valid_email(&email_raw) {
            return Some(json_response(
                400,
                json!({ "error": "valid email required" }),
            ));
        }
        let norm = mitch_lib::auth::normalize_email(&email_raw);
        if !revoke_premium_in_applications(state, &norm, &admin_email, &json!(reason)) {
            return Some(json_response(
                404,
                json!({ "error": "active premium user not found" }),
            ));
        }
        // Clean up user stats premium email fields.
        let stats_file = state.cfg.data_dir.join("user_stats.json");
        let mut stats = state.store.read_document(&stats_file, json!({}));
        if let Some(entry) = stats
            .as_object_mut()
            .and_then(|m| m.get_mut(norm.as_str()))
            .and_then(|v| v.as_object_mut())
        {
            for key in [
                "premium_email",
                "premium_email_request",
                "premium_email_fullname",
                "premium_email_reasons",
                "premium_email_requested_at",
                "premium_lost_at",
                "email_revoke_notified",
            ] {
                entry.remove(key);
            }
        }
        let _ = state.store.write_document(&stats_file, &stats);
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &admin_email,
            "revoke_premium",
            json!({ "targetEmail": email_raw, "reason": reason }),
        );
        return Some(json_response(
            200,
            json!({ "ok": true, "targetEmail": email_raw }),
        ));
    }

    None
}

fn json_response(code: u16, obj: Value) -> Response {
    crate::errors::json_resp(code, obj)
}

pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn parse_f64(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

fn valid_email(email: &str) -> bool {
    let mut parts = email.split('@');
    let local = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("");
    !local.is_empty() && !rest.is_empty() && rest.contains('.') && parts.next().is_none()
}

fn header_or_unknown(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string()
}

fn valid_id(sid: &str, state: &Arc<AppState>) -> bool {
    !sid.is_empty() && mitch_lib::auth::valid_id(sid, &state.id_secret)
}

/// `isPremiumEmail(email)` — mitch-lib ladder (admin/moderator/approved app).
pub fn is_premium_email(state: &Arc<AppState>, email: &str) -> bool {
    mitch_lib::auth::is_premium_email(&state.store, email)
}

/// Emails of every non-revoked token (the allUsers fan-out target set).
fn token_email_targets(state: &Arc<AppState>) -> std::collections::BTreeSet<String> {
    let tokens = state
        .store
        .read_document(&state.cfg.base_dir.join("data/tokens.json"), json!({}));
    let revoked = state
        .store
        .read_document(&state.cfg.base_dir.join("data/revoked.json"), json!({}));
    let mut targets = std::collections::BTreeSet::new();
    if let Some(map) = tokens.as_object() {
        for (tok, data) in map {
            let email = data
                .get("email")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase()
                .trim()
                .to_string();
            if email.is_empty()
                || revoked
                    .as_object()
                    .map(|r| r.contains_key(tok))
                    .unwrap_or(false)
            {
                continue;
            }
            targets.insert(email);
        }
    }
    targets
}

/// `grant-premium` application insert (10081-10093) — also used by the
/// moderator-request executor. Both JS call sites follow the insert with
/// `sendPremiumEmailOffer(targetRaw)` (server.js:13079, 7120), so the offer
/// is folded in here.
pub fn grant_premium_application(
    state: &Arc<AppState>,
    target_raw: &str,
    admin_email: &str,
    reason: &str,
) {
    let target_email = mitch_lib::auth::normalize_email(target_raw);
    let file = state.cfg.data_dir.join("applications.json");
    let apps = state
        .store
        .read_document(&file, json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut next = vec![json!({
        "name": target_email.split('@').next().unwrap_or(""),
        "email": target_raw,
        "type": "premium",
        "status": "approved",
        "grantPremium": true,
        "why": format!("Free premium granted by {admin_email}: {reason}"),
        "submitted_at": now_millis(),
        "approved_at": now_millis(),
        "approved_by": admin_email,
    })];
    next.extend(apps);
    let _ = state.store.write_document(&file, &json!(next));
    super::legacy::send_premium_email_offer(state, target_raw);
}

/// Marks approved premium applications for one email as revoked; returns
/// whether anything changed.
pub fn revoke_premium_in_applications(
    state: &Arc<AppState>,
    norm: &str,
    admin_email: &str,
    reason: &Value,
) -> bool {
    let file = state.cfg.data_dir.join("applications.json");
    let mut apps = state
        .store
        .read_document(&file, json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut changed = false;
    for app in apps.iter_mut() {
        if mitch_lib::auth::normalize_email(app.get("email").and_then(|v| v.as_str()).unwrap_or(""))
            != norm
        {
            continue;
        }
        if app.get("status").and_then(|v| v.as_str()) != Some("approved") {
            continue;
        }
        let is_premium = app.get("type").and_then(|v| v.as_str()) == Some("premium")
            || app.get("grantPremium").and_then(|v| v.as_bool()) == Some(true);
        if !is_premium {
            continue;
        }
        app["status"] = json!("revoked");
        app["revoked_at"] = json!(now_millis());
        app["revoked_by"] = json!(admin_email);
        app["why_revoked"] = reason.clone();
        changed = true;
    }
    if changed {
        let _ = state.store.write_document(&file, &json!(apps));
    }
    changed
}

/// `sendAdminNotice` for the moderator-request engine (6808-6834).
pub fn send_admin_notice(
    state: &Arc<AppState>,
    payload: &Value,
    actor: &str,
    requester_email: &str,
) -> Result<Value, (u16, String)> {
    let cleaned = mitch_lib::admin::clean_moderator_action_payload("send_notification", payload)
        .map_err(|e| (e.status, e.message))?;
    let all_users = cleaned.get("allUsers").and_then(|v| v.as_bool()) == Some(true);
    let title = cleaned
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let title = if title.is_empty() {
        "Admin notification"
    } else {
        title
    };
    let message = cleaned
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if message.trim().is_empty() {
        return Err((400, "message required".into()));
    }
    let batch_id = mitch_lib::crypto::random_bytes_hex(10);
    if all_users {
        let targets = token_email_targets(state);
        for email in &targets {
            mitch_lib::coins::add_admin_notification(
                &state.store,
                &state.cfg.data_dir,
                email,
                title,
                &message,
                actor,
                &batch_id,
                "",
            );
            crate::routes::push::push_admin_notification(state, email, title, &message);
        }
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            actor,
            "send_notification_all",
            json!({ "count": targets.len(), "title": title, "batchId": batch_id, "requestedBy": requester_email }),
        );
        return Ok(
            json!({ "ok": true, "allUsers": true, "count": targets.len(), "batchId": batch_id }),
        );
    }
    let target = cleaned
        .get("targetEmail")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if target.is_empty() {
        return Err((400, "valid target email required".into()));
    }
    let notice = mitch_lib::coins::add_admin_notification(
        &state.store,
        &state.cfg.data_dir,
        target,
        title,
        &message,
        actor,
        &batch_id,
        "",
    );
    crate::routes::push::push_admin_notification(state, target, title, &message);
    mitch_lib::admin::log_admin_action(
        &state.store,
        &state.cfg.data_dir,
        actor,
        "send_notification",
        json!({
            "targetEmail": target,
            "title": title,
            "notificationId": notice.as_ref().and_then(|n| n.get("id")).and_then(|v| v.as_str()).unwrap_or(""),
            "batchId": batch_id,
            "requestedBy": requester_email,
        }),
    );
    Ok(json!({
        "ok": true,
        "targetEmail": target,
        "notificationId": notice.as_ref().and_then(|n| n.get("id")).and_then(|v| v.as_str()).unwrap_or(""),
        "batchId": batch_id,
    }))
}

/// `removeAdminNotice` for the moderator-request engine (6788-6806).
pub fn remove_admin_notice(
    state: &Arc<AppState>,
    payload: &Value,
    actor: &str,
    requester_email: &str,
) -> Result<Value, (u16, String)> {
    let cleaned = mitch_lib::admin::clean_moderator_action_payload("unsend_notification", payload)
        .map_err(|e| (e.status, e.message))?;
    let id = cleaned
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let batch_id = cleaned
        .get("batchId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if id.is_empty() && batch_id.is_empty() {
        return Err((400, "notification id or batch id required".into()));
    }
    let removed = unsend_notification(state, &id, &batch_id);
    mitch_lib::admin::log_admin_action(
        &state.store,
        &state.cfg.data_dir,
        actor,
        "unsend_notification",
        json!({ "id": id, "batchId": batch_id, "removed": removed, "requestedBy": requester_email }),
    );
    Ok(json!({ "ok": true, "removed": removed }))
}

pub fn moderator_gift_coins(
    state: &Arc<AppState>,
    payload: &Value,
    actor: &str,
    requester_email: &str,
) -> Result<Value, (u16, String)> {
    let amount = payload
        .get("amount")
        .and_then(|v| v.as_f64())
        .unwrap_or(f64::NAN);
    if !amount.is_finite() || amount <= 0.0 {
        return Err((400, "amount must be a positive number".into()));
    }
    if amount > 1_000_000_000.0 {
        return Err((400, "amount is too large".into()));
    }
    let target = payload
        .get("targetEmail")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    mitch_lib::coins::add_coins(
        &state.store,
        &state.cfg.data_dir,
        &target,
        amount,
        state.coin_multiplier(),
        "",
    );
    mitch_lib::coins::add_coin_gift_notice(
        &state.store,
        &state.cfg.data_dir,
        &target,
        amount,
        actor,
        payload
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("admin gift"),
    );
    mitch_lib::admin::log_admin_action(
        &state.store,
        &state.cfg.data_dir,
        actor,
        "gift_coins",
        json!({
            "targetEmail": target,
            "amount": mitch_lib::jsval::num_value(amount),
            "reason": payload.get("reason").cloned().unwrap_or(Value::Null),
            "requestedBy": requester_email,
        }),
    );
    Ok(json!({
        "ok": true,
        "targetEmail": target,
        "amount": mitch_lib::jsval::num_value(amount),
        "newBalance": mitch_lib::jsval::num_value(mitch_lib::coins::get_coins(&state.store, &state.cfg.data_dir, &target)),
    }))
}

pub fn moderator_burn_coins(
    state: &Arc<AppState>,
    payload: &Value,
    actor: &str,
    requester_email: &str,
) -> Result<Value, (u16, String)> {
    let amount = payload
        .get("amount")
        .and_then(|v| v.as_f64())
        .unwrap_or(f64::NAN);
    if !amount.is_finite() || amount <= 0.0 {
        return Err((400, "amount must be a positive number".into()));
    }
    let target = payload
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    mitch_lib::coins::add_coins(
        &state.store,
        &state.cfg.data_dir,
        &target,
        -amount,
        state.coin_multiplier(),
        "",
    );
    mitch_lib::admin::log_admin_action(
        &state.store,
        &state.cfg.data_dir,
        actor,
        "burn_coins",
        json!({ "target": target, "amount": mitch_lib::jsval::num_value(amount), "requestedBy": requester_email }),
    );
    Ok(json!({
        "ok": true,
        "targetEmail": target,
        "amount": mitch_lib::jsval::num_value(amount),
        "newBalance": mitch_lib::jsval::num_value(mitch_lib::coins::get_coins(&state.store, &state.cfg.data_dir, &target)),
    }))
}

/// The unsend scan shared by the endpoint and remove_admin_notice.
fn unsend_notification(state: &Arc<AppState>, id: &str, batch_id: &str) -> i64 {
    let file = state.cfg.data_dir.join("coin_gifts.json");
    let mut gifts = state.store.read_document(&file, json!({}));
    let mut removed = 0i64;
    if let Some(map) = gifts.as_object_mut() {
        for (_email, notices) in map.iter_mut() {
            let Some(arr) = notices.as_array().cloned() else {
                continue;
            };
            let kept: Vec<Value> = arr
                .into_iter()
                .filter(|notice| {
                    let is_notice =
                        notice.get("kind").and_then(|v| v.as_str()) == Some("admin_notice");
                    let matches = is_notice
                        && ((!id.is_empty()
                            && notice.get("id").and_then(|v| v.as_str()).unwrap_or("") == id)
                            || (!batch_id.is_empty()
                                && notice.get("batchId").and_then(|v| v.as_str()).unwrap_or("")
                                    == batch_id));
                    if matches {
                        removed += 1;
                    }
                    !matches
                })
                .collect();
            *notices = json!(kept);
        }
    }
    let _ = state.store.write_document(&file, &gifts);
    removed
}

/// `sendDailySummaryNotification` body (server.js:3966-4014) — builds the
/// text; the ntfy send happens in push.rs. Also shared with the batch 3
/// daily-summary scheduler (workers_site.rs).
pub(crate) fn build_daily_summary(state: &Arc<AppState>) -> String {
    let logs = state
        .store
        .read_document(&state.cfg.data_dir.join("sessions.json"), json!([]));
    let start_of_day = local_start_of_day_ms();
    let mut registered: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut guest_count = 0usize;
    for entry in logs.as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        let ts_raw = entry
            .get("timestamp")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let Some(entry_time) = parse_timestamp(ts_raw) else {
            continue;
        };
        if entry_time < start_of_day {
            continue;
        }
        let id = entry.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let mut email = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, id);
        if email.is_none() {
            email = mitch_lib::auth::names_get_email(&state.store, id);
        }
        match email {
            Some(e) if !e.is_empty() => {
                registered.insert(e);
            }
            _ => guest_count += 1,
        }
    }
    let registered_count = registered.len();
    let total = registered_count + guest_count;
    let mut summary = String::new();
    summary.push_str("Daily Site Summary\n");
    summary.push_str("------------------\n");
    summary.push_str(&format!("Total Unique Visitors: {total}\n"));
    summary.push_str(&format!("- Members: {registered_count}\n"));
    summary.push_str(&format!("- Guests: {guest_count}\n\n"));
    if registered_count > 0 {
        summary.push_str("Members active today:\n");
        summary.push_str(
            &registered
                .iter()
                .map(|e| format!("• {e}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    } else {
        summary.push_str("No members visited today.");
    }
    summary
}

/// `new Date(y, m, d).getTime()` — local midnight today (server TZ).
fn local_start_of_day_ms() -> i64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // Local offset: read /etc/localtime offset via the `date` command is
    // heavy; derive from the TZ env when set, else UTC. The JS runs on the
    // VPS with its local timezone; parity here is best-effort.
    let offset = local_tz_offset_secs();
    let local = secs + offset;
    let day = local.div_euclid(86_400) * 86_400;
    (day - offset) * 1000
}

pub(crate) fn local_tz_offset_secs() -> i64 {
    // Parse `TZ` env or fall back to the offset embedded in `date +%z`.
    if let Ok(out) = std::process::Command::new("date").arg("+%z").output() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        // "+0530" / "-0800"
        let sign = if s.starts_with('-') { -1 } else { 1 };
        let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() == 4 {
            let hours: i64 = digits[..2].parse().unwrap_or(0);
            let mins: i64 = digits[2..].parse().unwrap_or(0);
            return sign * (hours * 3600 + mins * 60);
        }
    }
    0
}

/// `new Date(str).getTime()` for the session-log timestamp formats
/// (public for the admin data dtype `users`).
/// (ISO-ish "YYYY-MM-DD HH:MM:SS(.mmm)" or ISO 8601). Returns None when
/// unparseable (JS `NaN` → skipped by the >= startOfDay comparison).
pub fn parse_timestamp(s: &str) -> Option<i64> {
    let s = s.trim();
    // ISO 8601 with T separator.
    if let Some((date, time)) = s.split_once('T') {
        let (hms, _frac) = time.split_once('.').unwrap_or((time, ""));
        return parse_date_hms(date, hms, 0);
    }
    if let Some((date, time)) = s.split_once(' ') {
        let (hms, frac_ms) = match time.split_once('.') {
            Some((h, f)) => (h, frac_ms_of(f)),
            None => (time, 0),
        };
        return parse_date_hms(date, hms, frac_ms);
    }
    None
}

fn frac_ms_of(frac: &str) -> i64 {
    let digits: String = frac
        .chars()
        .filter(|c| c.is_ascii_digit())
        .take(3)
        .collect();
    if digits.is_empty() {
        return 0;
    }
    let mut ms: i64 = digits.parse().unwrap_or(0);
    for _ in digits.len()..3 {
        ms *= 10;
    }
    ms
}

fn parse_date_hms(date: &str, hms: &str, frac_ms: i64) -> Option<i64> {
    let mut d = date.split('-');
    let year: i64 = d.next()?.parse().ok()?;
    let month: i64 = d.next()?.parse().ok()?;
    let day: i64 = d.next()?.parse().ok()?;
    let mut t = hms.split(':');
    let hour: i64 = t.next().unwrap_or("0").parse().unwrap_or(0);
    let min: i64 = t.next().unwrap_or("0").parse().unwrap_or(0);
    let sec: i64 = t
        .next()
        .unwrap_or("0")
        .split_whitespace()
        .next()
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);
    Some(civil_to_epoch_ms(year, month, day, hour, min, sec) + frac_ms)
}

/// Days-from-civil (Howard Hinnant), UTC.
fn civil_to_epoch_ms(y: i64, m: i64, d: i64, hh: i64, mm: i64, ss: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m - 3).rem_euclid(12);
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400_000 + hh * 3_600_000 + mm * 60_000 + ss * 1000
}
