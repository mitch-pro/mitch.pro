//! `/api/admin/*` route group (plan Step 8) — port of server.js's admin
//! surface. Split per concern, mirroring server.js's sections:
//! - `dashboard.rs`: read-only dashboards (resources, logs, feeds, RTP,
//!   advanced-data, maintenance/shop status)
//! - `moderation.rs`: moderators, panels, requests, bans, reports, blog,
//!   content tools
//! - `economy.rs`: coins, premium grants, notifications, broadcast, casino
//!   state
//! - `legacy.rs`: passphrase-key auth endpoints (js/data/ssh-key/team-token/
//!   canvas/reset-ratelimit) + passphrase management
//! - `vm.rs`: Proxmox VM approval flows
//!
//! Global gate (server.js:7945-7973): every `/api/admin/*` path except
//! `passphrase-status` requires a valid sid, `isAnyAdminId`, and — for full
//! admins — the `X-Admin-Passphrase` header. `/api/admin/data` and
//! `/api/admin/js` still pass the gate and then additionally require the
//! admin key (`checkAdminPw`).

use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use serde_json::{json, Value};
use std::sync::Arc;

pub mod dashboard;
pub mod data;
pub mod economy;
pub mod legacy;
pub mod moderation;
pub mod vm;

pub type Resp = Option<Response>;

/// Parsed per-request auth context shared by the submodules.
pub struct AdminCtx {
    pub cookies: mitch_lib::auth::Cookies,
    pub sid: String,
    pub ip: String,
}

impl AdminCtx {
    /// `emailFromSid(sid) || 'admin'`.
    pub fn email(&self, state: &AppState) -> String {
        mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &self.sid)
            .unwrap_or_else(|| "admin".to_string())
    }

    /// `isAdminId(sid)`.
    pub fn is_admin(&self, state: &AppState) -> bool {
        let node_env_test = std::env::var("NODE_ENV").unwrap_or_default() == "test";
        mitch_lib::auth::is_admin_id(&state.store, &state.id_secret, &self.sid, node_env_test)
    }

    /// `isAnyAdminId(sid)`.
    pub fn is_any_admin(&self, state: &AppState) -> bool {
        mitch_lib::auth::is_any_admin_id(&state.store, &state.id_secret, &self.sid, false)
    }

    /// `canGrantPremiumId(sid)`.
    pub fn can_grant_premium(&self, state: &AppState) -> bool {
        mitch_lib::admin::can_grant_premium_id(&state.store, &state.id_secret, &self.sid)
    }

    /// `isOwnerId(sid)`.
    pub fn is_owner(&self, state: &AppState) -> bool {
        mitch_lib::auth::is_owner_id(&state.store, &state.id_secret, &self.sid)
    }
}

fn json_response(code: u16, obj: Value) -> Response {
    crate::errors::json_resp(code, obj)
}

pub fn unauthorized() -> Response {
    json_response(401, json!({ "error": "unauthorized" }))
}

pub fn forbidden() -> Response {
    json_response(403, json!({ "error": "forbidden" }))
}

/// `getCookies(req)` + `sid = studentId || id` + `getRealIp(req)`.
pub fn admin_ctx(state: &AppState, headers: &HeaderMap) -> AdminCtx {
    let node_env_test = std::env::var("NODE_ENV").unwrap_or_default() == "test";
    let cookie_header = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let cookies = mitch_lib::auth::get_cookies_from_header_value(
        cookie_header,
        &state.store,
        &state.id_secret,
        node_env_test,
    );
    let mut sid = cookies.get("studentId").unwrap_or("").to_string();
    if sid.is_empty() {
        sid = cookies.get("id").unwrap_or("").to_string();
    }
    let ip = crate::handler::get_real_ip(headers, None);
    AdminCtx { cookies, sid, ip }
}

/// `devTestRequestAllowed(req)` — dev-test access + localhost/private host.
pub fn dev_test_request_allowed(headers: &HeaderMap) -> bool {
    if !mitch_lib::auth::dev_test_access_enabled() {
        return false;
    }
    let host = crate::hosts::request_host(headers).to_lowercase();
    let host = host.split(':').next().unwrap_or("");
    host == "localhost" || host == "127.0.0.1" || host == "::1"
}

/// The global admin gate — server.js:7945-7973. `Some` rejects.
pub fn admin_gate(state: &Arc<AppState>, headers: &HeaderMap) -> Resp {
    let ctx = admin_ctx(state, headers);
    if !mitch_lib::auth::valid_id(&ctx.sid, &state.id_secret) {
        return Some(unauthorized());
    }
    if !ctx.is_any_admin(state) {
        return Some(forbidden());
    }
    // Passphrase enforcement applies to full administrators only; moderators
    // skip it. Dev-superuser sessions on localhost bypass.
    let is_dev_superuser = ctx
        .cookies
        .get("_authSession")
        .and_then(|v| serde_json::from_str::<Value>(v).ok())
        .and_then(|s| s.get("devSuperuser").and_then(|v| v.as_bool()).map(|_| ()))
        .is_some()
        && dev_test_request_allowed(headers);
    if ctx.is_admin(state) && !is_dev_superuser {
        let email = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &ctx.sid)
            .unwrap_or_else(|| "admin".to_string());
        let norm = mitch_lib::auth::normalize_email(&email);
        let data = mitch_lib::admin::load_admin_passphrase(&state.store, &state.cfg.data_dir);
        let entry = data.get(norm.as_str()).cloned().unwrap_or(json!({}));
        if entry
            .get("hash")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .is_empty()
        {
            return Some(json_response(
                403,
                json!({ "error": "passphrase_not_configured" }),
            ));
        }
        let pass = headers
            .get("X-Admin-Passphrase")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .trim()
            .to_string();
        if pass.is_empty()
            || !mitch_lib::admin::verify_admin_passphrase_raw(
                &state.store,
                &state.id_secret,
                &state.cfg.data_dir,
                &ctx.sid,
                &pass,
            )
        {
            return Some(json_response(403, json!({ "error": "invalid_passphrase" })));
        }
    }
    None
}

/// API route dispatch — called from handler.rs after the global gate.
/// Returns `Some(Response)` for a matched route, `None` for fallthrough.
pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    search: &str,
    body: &Value,
) -> Resp {
    let ctx = admin_ctx(state, headers);

    // ── Legacy pw-key tools (js/ssh-key/team-token/canvas/ratelimit) ──
    if let Some(resp) = legacy::handle(state, method, path, headers, search, body, &ctx).await {
        return Some(resp);
    }

    // ── Legacy pw-key data transport (/api/admin/data dtype dispatch) ──
    if let Some(resp) = data::handle(state, method, path, body, &ctx).await {
        return Some(resp);
    }

    // ── Moderation profile lookup (async: aggregates live Matrix room state) ──
    if path == "/api/admin/moderation/lookup" && *method == Method::GET {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        return Some(moderation::lookup_profile(state, search).await);
    }

    // ── Conduit health/diagnostics widget (async: pings Conduit) ──
    if path == "/api/admin/matrix/health" && *method == Method::GET {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        return Some(crate::routes::matrix::admin_conduit_health(state).await);
    }

    // ── Moderation + staff tools ──
    if let Some(resp) = moderation::handle(state, method, path, headers, body, &ctx) {
        return Some(resp);
    }

    // ── Economy/coins/premium/casino ──
    if let Some(resp) = economy::handle(state, method, path, headers, body, &ctx) {
        return Some(resp);
    }

    // ── VM management (Step 8 wiring; Proxmox exec in Step 13) ──
    if let Some(resp) = vm::handle(state, method, path, headers, search, body, &ctx).await {
        return Some(resp);
    }

    // ── Dashboards & status ──
    if let Some(resp) = dashboard::handle(state, method, path, headers, search, &ctx) {
        return Some(resp);
    }

    None
}
