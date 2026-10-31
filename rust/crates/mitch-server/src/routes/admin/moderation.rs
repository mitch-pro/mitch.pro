//! Moderation & staff tools — moderators list/CRUD (10206-10240), panel
//! config (10235/10471), moderator action requests (9581-9649 + the
//! create/execute engine 6727-7006), shadow-ban/restricted-mode (10603),
//! ban/unban account (10626/10681), chat-reports resolve (10734), profile
//! reports resolve (8874), blog deletions/contributors (8824-8872), content
//! mirror/featured (10770-10810), prox legacy 410 (10730).

#![allow(clippy::expect_used)] // infallible static regexes
use super::{forbidden, AdminCtx, Resp};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use serde_json::{json, Value};
use std::sync::Arc;

fn spawn_staff_sync(
    secret: Vec<u8>,
    store: Arc<mitch_lib::data::DataStore>,
    data_dir: std::path::PathBuf,
) {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            crate::routes::matrix::sync_staff_power_levels_to_all_official_rooms(
                &secret, &store, &data_dir,
            )
            .await;
        });
    }
}

/// Fire-and-forget Matrix cleanup for a freshly banned account: ban from all
/// rooms (drops them from member lists) and redact their messages.
fn spawn_matrix_purge(
    secret: Vec<u8>,
    store: Arc<mitch_lib::data::DataStore>,
    data_dir: std::path::PathBuf,
    matrix_user_id: String,
    reason: String,
) {
    if std::env::var("NODE_ENV").unwrap_or_default() == "test" {
        return;
    }
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            crate::routes::matrix::purge_banned_matrix_user_all_rooms(
                &secret,
                &store,
                &data_dir,
                &matrix_user_id,
                &reason,
            )
            .await;
        });
    }
}

fn spawn_matrix_unban(
    secret: Vec<u8>,
    store: Arc<mitch_lib::data::DataStore>,
    data_dir: std::path::PathBuf,
    matrix_user_id: String,
) {
    if std::env::var("NODE_ENV").unwrap_or_default() == "test" {
        return;
    }
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            crate::routes::matrix::unban_matrix_user_all_rooms(
                &secret,
                &store,
                &data_dir,
                &matrix_user_id,
            )
            .await;
        });
    }
}

pub fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body: &Value,
    ctx: &AdminCtx,
) -> Resp {
    if path == "/api/admin/bans" && *method == Method::GET {
        if !ctx.is_admin(state) {
            return Some(forbidden());
        }
        let mut seen = std::collections::HashSet::new();
        let bans: Vec<Value> = mitch_lib::bans::blacklist_all(&state.store)
            .into_iter()
            .filter(|ban| {
                seen.insert(mitch_lib::auth::normalize_email(
                    ban["email"].as_str().unwrap_or(""),
                ))
            })
            .collect();
        let vm_bans = state.vm_bans.read().unwrap_or_else(|e| e.into_inner());
        let byo_bans = state.byo_os_bans.read().unwrap_or_else(|e| e.into_inner());
        let restrictions:Vec<Value>=vm_bans.union(&byo_bans).map(|email|json!({"email":email,"vm":vm_bans.contains(email),"byoOs":byo_bans.contains(email)})).collect();
        return Some(json_response(
            200,
            json!({ "bans": bans, "restrictions":restrictions, "supportsTimedBans":true,"supportsChatCleanup":true, "chatCleanup":mitch_lib::chat_cleanup::statuses(&state.store) }),
        ));
    }
    // GET /api/admin/matrix/outside-accounts — outside (non-student) accounts
    // waiting for permission to chat in Matrix. Moderators and up.
    if path == "/api/admin/matrix/outside-accounts" && *method == Method::GET {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        return Some(json_response(
            200,
            json!({ "pending": mitch_lib::matrix::outside_approvals_pending(&state.store) }),
        ));
    }

    // POST /api/admin/matrix/outside-accounts {email, approved} — approve or
    // deny an outside account's Matrix chat access. Moderators and up.
    if path == "/api/admin/matrix/outside-accounts" && *method == Method::POST {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        let raw_target = body["email"]
            .as_str()
            .or_else(|| body["identifier"].as_str())
            .unwrap_or("")
            .trim();
        let target = if raw_target.starts_with('@') && raw_target.contains(':') {
            raw_target.to_ascii_lowercase()
        } else {
            mitch_lib::auth::normalize_email(raw_target)
        };
        let is_valid_matrix_id =
            target.starts_with('@') && target.contains(':') && target.len() > 3;
        let is_valid_ext_email =
            valid_email(&target) && !mitch_lib::matrix::is_student_email(&target);

        if !is_valid_matrix_id && !is_valid_ext_email {
            return Some(json_response(
                400,
                json!({ "error": "an outside account email or Matrix user ID is required" }),
            ));
        }
        let approved = body["approved"].as_bool().unwrap_or(false);
        let actor = ctx.email(state);
        if !mitch_lib::matrix::outside_approval_decide(&state.store, &target, approved, &actor) {
            return Some(json_response(
                500,
                json!({ "error": "Could not save the decision." }),
            ));
        }
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &actor,
            if approved {
                "matrix_outside_approve"
            } else {
                "matrix_outside_deny"
            },
            json!({ "email": target }),
        );
        if approved {
            let state = state.clone();
            let target = target.clone();
            tokio::spawn(async move {
                crate::routes::matrix::join_approved_outside_account(&state, &target).await;
            });
        }
        return Some(json_response(
            200,
            json!({ "ok": true, "email": target, "approved": approved }),
        ));
    }

    // POST /api/admin/matrix/purge-user {target} — re-run the Matrix cleanup
    // (room bans, message and join/ban-notice redaction) for an account that is
    // already banned. `target` is an email or a Matrix ID like @name:mitch.pro.
    // Moderators and up.
    if path == "/api/admin/matrix/purge-user" && *method == Method::POST {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        let target = body["target"].as_str().unwrap_or("").trim().to_string();
        if target.is_empty() {
            return Some(json_response(400, json!({ "error": "target is required" })));
        }
        let resolved = super::resolve_admin_target(state, &target);
        if mitch_lib::auth::is_admin_email(
            &state.store,
            &mitch_lib::auth::normalize_email(&resolved),
        ) {
            return Some(forbidden());
        }
        spawn_matrix_purge(
            state.id_secret.clone(),
            state.store.clone(),
            state.cfg.data_dir.clone(),
            resolved,
            "Inappropriate content".to_string(),
        );
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &ctx.email(state),
            "matrix_purge_user",
            json!({ "target": target }),
        );
        return Some(json_response(202, json!({ "ok": true, "started": true })));
    }

    // POST /api/admin/moderators (admins only).
    if path == "/api/admin/moderators" && *method == Method::POST {
        if !ctx.is_admin(state) {
            return Some(forbidden());
        }
        return Some(set_moderator(state, body, ctx));
    }

    // GET /api/admin/moderators.
    if path == "/api/admin/moderators" && *method == Method::GET {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        return Some(json_response(
            200,
            json!({ "moderators": mitch_lib::auth::moderator_emails(&state.store) }),
        ));
    }

    // POST /api/admin/admins (owners only).
    if path == "/api/admin/admins" && *method == Method::POST {
        if !ctx.is_owner(state) {
            return Some(forbidden());
        }
        return Some(set_admin(state, body, ctx));
    }

    // GET /api/admin/admins (owners only).
    if path == "/api/admin/admins" && *method == Method::GET {
        if !ctx.is_owner(state) {
            return Some(forbidden());
        }
        return Some(json_response(
            200,
            json!({ "admins": mitch_lib::auth::admin_member_emails(&state.store) }),
        ));
    }

    // POST /api/admin/co-owners (owners only).
    if path == "/api/admin/co-owners" && *method == Method::POST {
        if !ctx.is_owner(state) {
            return Some(forbidden());
        }
        return Some(set_co_owner(state, body, ctx));
    }

    // GET /api/admin/co-owners (owners only).
    if path == "/api/admin/co-owners" && *method == Method::GET {
        if !ctx.is_owner(state) {
            return Some(forbidden());
        }
        return Some(json_response(
            200,
            json!({ "coOwners": mitch_lib::auth::co_owner_member_emails(&state.store) }),
        ));
    }

    // POST /api/admin/testers (admins only).
    if path == "/api/admin/testers" && *method == Method::POST {
        if !ctx.is_admin(state) {
            return Some(forbidden());
        }
        return Some(set_tester(state, body, ctx));
    }

    // GET /api/admin/testers (admins only).
    if path == "/api/admin/testers" && *method == Method::GET {
        if !ctx.is_admin(state) {
            return Some(forbidden());
        }
        return Some(json_response(
            200,
            json!({ "testers": tester_emails(state) }),
        ));
    }

    // POST /api/admin/audit/revert (admins and owners).
    if path == "/api/admin/audit/revert" && *method == Method::POST {
        if !ctx.is_admin(state) {
            return Some(forbidden());
        }
        return Some(revert_admin_action(state, body, ctx, headers));
    }

    // GET /api/admin/moderator-panel.
    if path == "/api/admin/moderator-panel" && *method == Method::GET {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        return Some(json_response(
            200,
            mitch_lib::admin::moderator_panel_config(&state.store, &state.cfg.data_dir),
        ));
    }

    // POST /api/admin/moderator-panel (admins only).
    if path == "/api/admin/moderator-panel" && *method == Method::POST {
        if !ctx.is_admin(state) {
            return Some(forbidden());
        }
        let links = mitch_lib::admin::sanitize_moderator_panel_links(
            body.get("links").unwrap_or(&json!([])),
        );
        let links_json = json!(links);
        let _ = state.store.write_document(
            &state.cfg.data_dir.join("moderator_panel.json"),
            &json!({ "links": links }),
        );
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &ctx.email(state),
            "update_moderator_panel",
            json!({ "linkCount": links.len() }),
        );
        return Some(json_response(
            200,
            json!({ "ok": true, "links": links_json }),
        ));
    }

    // GET /api/admin/moderator-requests (admins see all; moderators own only).
    if path == "/api/admin/moderator-requests" && *method == Method::GET {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        let email = mitch_lib::auth::normalize_email(&ctx.email(state));
        let requests: Vec<Value> =
            mitch_lib::admin::load_moderator_requests(&state.store, &state.cfg.data_dir)
                .into_iter()
                .filter(|req| {
                    ctx.is_admin(state)
                        || mitch_lib::auth::normalize_email(
                            req.get("requestedByEmail")
                                .and_then(|v| v.as_str())
                                .unwrap_or(""),
                        ) == email
                })
                .take(250)
                .map(|req| mitch_lib::admin::public_moderator_request(&req))
                .collect();
        return Some(json_response(200, json!({ "requests": requests })));
    }

    // POST /api/admin/moderator-requests/resolve (admins only).
    if path == "/api/admin/moderator-requests/resolve" && *method == Method::POST {
        if !ctx.is_admin(state) {
            return Some(forbidden());
        }
        return Some(resolve_moderator_request(state, body, ctx));
    }

    // POST /api/admin/shadow-ban & /api/admin/restricted-mode (toggles).
    if (path == "/api/admin/shadow-ban" || path == "/api/admin/restricted-mode")
        && *method == Method::POST
    {
        return Some(json_response(
            410,
            json!({"error":"Shadow bans have been retired. Use account bans with a visible reason and duration."}),
        ));
    }

    // POST /api/admin/vm-ban (toggle) — bars a user from ALL VM features,
    // templates and BYO-OS both.
    if path == "/api/admin/vm-ban" && *method == Method::POST {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        return Some(toggle_ban_set(
            state,
            body,
            ctx,
            headers,
            &state.vm_bans,
            "data/vm_bans.json",
            "vm_ban",
        ));
    }

    // POST /api/admin/byo-os-ban (toggle) — bars a user from the BYO-OS
    // custom-ISO feature specifically; normal template VMs still work.
    if path == "/api/admin/byo-os-ban" && *method == Method::POST {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        return Some(toggle_ban_set(
            state,
            body,
            ctx,
            headers,
            &state.byo_os_bans,
            "data/byo_os_bans.json",
            "byo_os_ban",
        ));
    }

    // POST /api/admin/ban-account.
    if path == "/api/admin/ban-account" && *method == Method::POST {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        return Some(ban_account(state, body, ctx, headers));
    }

    // POST /api/admin/unban-account.
    if path == "/api/admin/unban-account" && *method == Method::POST {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        return Some(unban_account(state, body, ctx, headers));
    }

    // Legacy mitch.prox admin tools removed.
    if path == "/api/admin/prox/sessions"
        || path == "/api/admin/prox/block"
        || path == "/api/admin/prox/unblock"
    {
        return Some(json_response(
            410,
            json!({ "error": "gone", "message": "Arbitrary proxy management has been removed." }),
        ));
    }

    // POST /api/admin/chat-reports/resolve.
    if path == "/api/admin/chat-reports/resolve" {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        return Some(chat_report_resolve(state, body, ctx));
    }

    // POST /api/admin/content/mirror.
    if path == "/api/admin/content/mirror" {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        let new_url = body.get("url").cloned().unwrap_or(Value::Null);
        let sites_path = state.cfg.base_dir.join("data/sites");
        let Ok(contents) = std::fs::read_to_string(&sites_path) else {
            return Some(json_response(500, json!({ "error": "internal_error" })));
        };
        static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let re = RE.get_or_init(|| {
            regex::Regex::new(r"url https://docs\.google\.com/document/d/[^\s]+ Mitch\.pro Mirrors")
                .unwrap_or_else(|e| {
                    tracing::error!("mirror regex failed: {e}");
                    regex::Regex::new("$^").unwrap_or_else(|_| {
                        regex::Regex::new("$^").unwrap_or_else(|e| {
                            tracing::error!("fallback regex failed: {e}");
                            regex::Regex::new("$^").unwrap_or_else(|_| unreachable!())
                        })
                    })
                })
        });
        let new_line = format!("url {new_url} Mitch.pro Mirrors");
        let updated = re.replace(&contents, new_line.as_str()).to_string();
        let _ = std::fs::write(&sites_path, &updated);
        let actor = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &ctx.sid)
            .unwrap_or_else(|| "moderator".to_string());
        let role = if ctx.is_admin(state) {
            "admin"
        } else {
            "moderator"
        };
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &actor,
            "update_mirror",
            json!({
                "url": new_url,
                "ip": ctx.ip,
                "userAgent": user_agent(headers),
                "role": role,
            }),
        );
        return Some(json_response(200, json!({ "ok": true })));
    }

    // POST /api/admin/content/featured.
    if path == "/api/admin/content/featured" {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        let href = body.get("href").cloned().unwrap_or(Value::Null);
        *state
            .featured_game_href
            .write()
            .unwrap_or_else(|e| e.into_inner()) = href.as_str().unwrap_or("").to_string();
        let _ = state
            .store
            .write_document(&state.cfg.data_dir.join("featured_game.json"), &href);
        let actor = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &ctx.sid)
            .unwrap_or_else(|| "moderator".to_string());
        let role = if ctx.is_admin(state) {
            "admin"
        } else {
            "moderator"
        };
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &actor,
            "set_featured",
            json!({
                "href": href,
                "ip": ctx.ip,
                "userAgent": user_agent(headers),
                "role": role,
            }),
        );
        return Some(json_response(200, json!({ "ok": true })));
    }

    // POST /api/admin/profile-reports/resolve.
    if path == "/api/admin/profile-reports/resolve" && *method == Method::POST {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        return Some(profile_report_resolve(state, body, ctx));
    }

    // GET /api/admin/blog-deletions.
    if path == "/api/admin/blog-deletions" && *method == Method::GET {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        let logs = state
            .store
            .read_document(&state.cfg.data_dir.join("blog_delete_log.json"), json!([]));
        let logs: Vec<Value> = logs
            .as_array()
            .map(|a| {
                a.iter()
                    .take(100)
                    .map(|entry| {
                        json!({
                            "id": entry.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                            "ts": entry.get("ts").and_then(|v| v.as_i64()).unwrap_or(0),
                            "postId": entry.get("postId").and_then(|v| v.as_str()).unwrap_or(""),
                            "slug": entry.get("slug").and_then(|v| v.as_str()).unwrap_or(""),
                            "title": entry.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                            "status": entry.get("status").and_then(|v| v.as_str()).unwrap_or(""),
                            "authorEmail": mitch_lib::admin::mask_email(entry.get("authorEmail").and_then(|v| v.as_str()).unwrap_or("")),
                            "authorName": entry.get("authorName").and_then(|v| v.as_str()).unwrap_or(""),
                            "deletedBy": mitch_lib::admin::mask_email(entry.get("deletedBy").and_then(|v| v.as_str()).unwrap_or("")),
                            "deletedByRole": entry.get("deletedByRole").and_then(|v| v.as_str()).unwrap_or("user"),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        return Some(json_response(200, json!({ "logs": logs })));
    }

    // GET /api/admin/blog-contributors (admins only).
    if path == "/api/admin/blog-contributors" && *method == Method::GET {
        if !ctx.is_admin(state) {
            return Some(forbidden());
        }
        return Some(json_response(
            200,
            json!({ "contributors": mitch_lib::admin::blog_contributor_emails(&state.store, &state.cfg.data_dir) }),
        ));
    }

    // POST /api/admin/blog-contributors (admins only; /api/blog/write bucket).
    if path == "/api/admin/blog-contributors" && *method == Method::POST {
        if state
            .rate_limit_check(&ctx.ip, "anon", "/api/blog/write")
            .is_some()
        {
            return Some(json_response(
                429,
                json!({ "error": "Too many requests, slow down" }),
            ));
        }
        if !ctx.is_admin(state) {
            return Some(forbidden());
        }
        let admin_email = ctx.email(state);
        let target_raw = body
            .get("email")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let target = super::resolve_admin_target(state, target_raw);
        if target.is_empty() || !target.contains('@') {
            return Some(json_response(
                400,
                json!({ "error": "valid email required" }),
            ));
        }
        let active = body.get("active").and_then(|v| v.as_bool()) != Some(false);
        let mut contributors =
            mitch_lib::admin::blog_contributor_emails(&state.store, &state.cfg.data_dir);
        if active {
            if !contributors
                .iter()
                .any(|email| mitch_lib::auth::normalize_email(email) == target)
            {
                contributors.push(target.clone());
            }
        } else {
            contributors.retain(|email| mitch_lib::auth::normalize_email(email) != target);
        }
        contributors.sort();
        let _ = state.store.write_document(
            &state.cfg.data_dir.join("blog_contributors.json"),
            &json!(contributors),
        );
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &admin_email,
            if active {
                "add_blog_contributor"
            } else {
                "remove_blog_contributor"
            },
            json!({ "target": target }),
        );
        return Some(json_response(
            200,
            json!({ "ok": true, "contributors": contributors }),
        ));
    }

    None
}

fn json_response(code: u16, obj: Value) -> Response {
    crate::errors::json_resp(code, obj)
}

/// `POST /api/admin/moderators` — add/remove from the bare moderators.json
/// array (the raw email is stored, normalization is only for comparison).
fn set_moderator(state: &Arc<AppState>, body: &Value, ctx: &AdminCtx) -> Response {
    let admin_email = ctx.email(state);
    let target_raw = body
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let target_raw = super::resolve_admin_target(state, &target_raw);
    let target = mitch_lib::auth::normalize_email(&target_raw);
    let active = body.get("active").and_then(|v| v.as_bool()) == Some(true);
    let mut mods = mitch_lib::auth::moderator_emails(&state.store);
    if active {
        if !mods
            .iter()
            .any(|m| mitch_lib::auth::normalize_email(m) == target)
        {
            mods.push(target_raw.clone());
        }
    } else {
        mods.retain(|m| mitch_lib::auth::normalize_email(m) != target);
    }
    let _ = state.store.write_document(
        &state.cfg.base_dir.join("data/moderators.json"),
        &json!(mods),
    );
    mitch_lib::admin::log_admin_action(
        &state.store,
        &state.cfg.data_dir,
        &admin_email,
        if active {
            "add_moderator"
        } else {
            "remove_moderator"
        },
        json!({ "target": target_raw }),
    );
    spawn_staff_sync(
        state.id_secret.clone(),
        state.store.clone(),
        state.cfg.data_dir.clone(),
    );
    json_response(200, json!({ "ok": true }))
}

/// Raw `data/testers.json` array - the admin panel's own tester list, read
/// the same way `mitch_lib::auth::is_tester_email` reads it, just without
/// collapsing to a single email's membership check.
fn tester_emails(state: &Arc<AppState>) -> Vec<String> {
    let raw = state
        .store
        .read_document(&state.cfg.base_dir.join("data/testers.json"), json!([]));
    raw.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// `POST /api/admin/testers` - add/remove from `data/testers.json`. Mirrors
/// set_moderator(); this backend route never existed before (the frontend's
/// Beta Testers panel called it and just got a 404), only the read side
/// (is_tester_email) was ever ported.
fn set_tester(state: &Arc<AppState>, body: &Value, ctx: &AdminCtx) -> Response {
    let admin_email = ctx.email(state);
    let target_raw = body
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let target_raw = super::resolve_admin_target(state, &target_raw);
    let target = mitch_lib::auth::normalize_email(&target_raw);
    if target.is_empty() || !target.contains('@') {
        return json_response(400, json!({ "error": "valid email required" }));
    }
    let active = body.get("active").and_then(|v| v.as_bool()) == Some(true);
    let mut testers = tester_emails(state);
    if active {
        if !testers
            .iter()
            .any(|m| mitch_lib::auth::normalize_email(m) == target)
        {
            testers.push(target_raw.clone());
        }
    } else {
        testers.retain(|m| mitch_lib::auth::normalize_email(m) != target);
    }
    let _ = state.store.write_document(
        &state.cfg.base_dir.join("data/testers.json"),
        &json!(testers),
    );
    // Adding a tester starts their sandbox; removing one returns them to their
    // real balance and premium state.
    if active {
        mitch_lib::tester::ensure(&state.store, &target);
    } else {
        mitch_lib::tester::remove(&state.store, &target);
    }
    mitch_lib::admin::log_admin_action(
        &state.store,
        &state.cfg.data_dir,
        &admin_email,
        if active {
            "add_tester"
        } else {
            "remove_tester"
        },
        json!({ "target": target_raw }),
    );
    json_response(200, json!({ "ok": true, "testers": testers }))
}

/// `POST /api/admin/admins` (owners only). Updates `data/admins.json["admins"]`.
fn set_admin(state: &Arc<AppState>, body: &Value, ctx: &AdminCtx) -> Response {
    let admin_email = ctx.email(state);
    let target_raw = body
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if target_raw.is_empty() {
        return json_response(400, json!({ "error": "email required" }));
    }
    let target_raw = super::resolve_admin_target(state, &target_raw);
    let target = mitch_lib::auth::normalize_email(&target_raw);
    let active = body.get("active").and_then(|v| v.as_bool()) == Some(true);
    if !active && mitch_lib::auth::is_owner_email(&state.store, &target) {
        return json_response(403, json!({ "error": "Cannot delete or demote the owner" }));
    }
    let admins_file = state.cfg.base_dir.join("data/admins.json");
    let mut doc = state.store.read_document(&admins_file, json!({}));
    let mut admins: Vec<String> = doc
        .get("admins")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    if active {
        if !admins
            .iter()
            .any(|m| mitch_lib::auth::normalize_email(m) == target)
        {
            admins.push(target_raw.clone());
        }
    } else {
        admins.retain(|m| mitch_lib::auth::normalize_email(m) != target);
    }

    if let Some(obj) = doc.as_object_mut() {
        obj.insert("admins".to_string(), json!(admins));
    } else {
        doc = json!({ "admins": admins });
    }

    let _ = state.store.write_document(&admins_file, &doc);
    mitch_lib::admin::log_admin_action(
        &state.store,
        &state.cfg.data_dir,
        &admin_email,
        if active { "add_admin" } else { "remove_admin" },
        json!({ "target": target_raw }),
    );
    spawn_staff_sync(
        state.id_secret.clone(),
        state.store.clone(),
        state.cfg.data_dir.clone(),
    );
    json_response(200, json!({ "ok": true, "admins": admins }))
}

/// `POST /api/admin/co-owners` (owners only). Updates `data/admins.json["coOwners"]`.
fn set_co_owner(state: &Arc<AppState>, body: &Value, ctx: &AdminCtx) -> Response {
    let admin_email = ctx.email(state);
    let target_raw = body
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if target_raw.is_empty() {
        return json_response(400, json!({ "error": "email required" }));
    }
    let target_raw = super::resolve_admin_target(state, &target_raw);
    let target = mitch_lib::auth::normalize_email(&target_raw);
    if mitch_lib::auth::is_owner_email(&state.store, &target) {
        return json_response(
            403,
            json!({ "error": "Owner cannot be modified as a developer" }),
        );
    }
    let active = body.get("active").and_then(|v| v.as_bool()) == Some(true);
    let admins_file = state.cfg.base_dir.join("data/admins.json");
    let mut doc = state.store.read_document(&admins_file, json!({}));
    let mut co_owners: Vec<String> = doc
        .get("coOwners")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    if active {
        if !co_owners
            .iter()
            .any(|m| mitch_lib::auth::normalize_email(m) == target)
        {
            co_owners.push(target_raw.clone());
        }
    } else {
        co_owners.retain(|m| mitch_lib::auth::normalize_email(m) != target);
    }

    if let Some(obj) = doc.as_object_mut() {
        obj.insert("coOwners".to_string(), json!(co_owners));
    } else {
        doc = json!({ "coOwners": co_owners });
    }

    let _ = state.store.write_document(&admins_file, &doc);
    mitch_lib::admin::log_admin_action(
        &state.store,
        &state.cfg.data_dir,
        &admin_email,
        if active {
            "add_co_owner"
        } else {
            "remove_co_owner"
        },
        json!({ "target": target_raw }),
    );
    spawn_staff_sync(
        state.id_secret.clone(),
        state.store.clone(),
        state.cfg.data_dir.clone(),
    );
    json_response(200, json!({ "ok": true, "coOwners": co_owners }))
}

/// `POST /api/admin/audit/revert` — reverts a logged admin action from admin_actions.json.
fn revert_admin_action(
    state: &Arc<AppState>,
    body: &Value,
    ctx: &AdminCtx,
    headers: &HeaderMap,
) -> Response {
    let admin_email = ctx.email(state);
    let action_id = body.get("id").and_then(|v| v.as_str()).unwrap_or("").trim();
    if action_id.is_empty() {
        return json_response(400, json!({ "error": "Action id required" }));
    }

    let actions_file = state.cfg.data_dir.join("admin_actions.json");
    let actions = state
        .store
        .read_document(&actions_file, json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();

    let target_action = actions
        .iter()
        .find(|a| a.get("id").and_then(|v| v.as_str()).unwrap_or("") == action_id);

    let Some(action_obj) = target_action else {
        return json_response(404, json!({ "error": "Action not found in audit log" }));
    };

    let action_name = action_obj
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let details = action_obj.get("details").unwrap_or(&Value::Null);

    let revert_result: Result<String, String> = match action_name {
        "ban_account" => {
            let email = details
                .get("targetEmail")
                .or_else(|| details.get("email"))
                .or_else(|| details.get("target"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if email.is_empty() {
                Err("Missing target email in action log".into())
            } else {
                let resp = unban_account(state, &json!({ "email": email }), ctx, headers);
                if resp.status().is_success() {
                    Ok(format!("Unbanned {}", email))
                } else {
                    Err("Failed to unban account".into())
                }
            }
        }
        "unban_account" => {
            let email = details
                .get("targetEmail")
                .or_else(|| details.get("email"))
                .or_else(|| details.get("target"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if email.is_empty() {
                Err("Missing target email in action log".into())
            } else {
                let resp = ban_account(
                    state,
                    &json!({ "email": email, "reason": "Reverted unban action" }),
                    ctx,
                    headers,
                );
                if resp.status().is_success() {
                    Ok(format!("Banned {}", email))
                } else {
                    Err("Failed to re-ban account".into())
                }
            }
        }
        "add_admin" => {
            if !ctx.is_owner(state) {
                return forbidden();
            }
            let target = details.get("target").and_then(|v| v.as_str()).unwrap_or("");
            if target.is_empty() {
                Err("Missing target in action log".into())
            } else {
                let resp = set_admin(state, &json!({ "email": target, "active": false }), ctx);
                if resp.status().is_success() {
                    Ok(format!("Removed admin {}", target))
                } else {
                    Err("Failed to remove admin".into())
                }
            }
        }
        "remove_admin" => {
            if !ctx.is_owner(state) {
                return forbidden();
            }
            let target = details.get("target").and_then(|v| v.as_str()).unwrap_or("");
            if target.is_empty() {
                Err("Missing target in action log".into())
            } else {
                let resp = set_admin(state, &json!({ "email": target, "active": true }), ctx);
                if resp.status().is_success() {
                    Ok(format!("Restored admin {}", target))
                } else {
                    Err("Failed to restore admin".into())
                }
            }
        }
        "add_co_owner" => {
            if !ctx.is_owner(state) {
                return forbidden();
            }
            let target = details.get("target").and_then(|v| v.as_str()).unwrap_or("");
            if target.is_empty() {
                Err("Missing target in action log".into())
            } else {
                let resp = set_co_owner(state, &json!({ "email": target, "active": false }), ctx);
                if resp.status().is_success() {
                    Ok(format!("Removed developer {}", target))
                } else {
                    Err("Failed to remove developer".into())
                }
            }
        }
        "remove_co_owner" => {
            if !ctx.is_owner(state) {
                return forbidden();
            }
            let target = details.get("target").and_then(|v| v.as_str()).unwrap_or("");
            if target.is_empty() {
                Err("Missing target in action log".into())
            } else {
                let resp = set_co_owner(state, &json!({ "email": target, "active": true }), ctx);
                if resp.status().is_success() {
                    Ok(format!("Restored developer {}", target))
                } else {
                    Err("Failed to restore developer".into())
                }
            }
        }
        "add_moderator" => {
            let target = details.get("target").and_then(|v| v.as_str()).unwrap_or("");
            if target.is_empty() {
                Err("Missing target in action log".into())
            } else {
                let resp = set_moderator(state, &json!({ "email": target, "active": false }), ctx);
                if resp.status().is_success() {
                    Ok(format!("Removed moderator {}", target))
                } else {
                    Err("Failed to remove moderator".into())
                }
            }
        }
        "remove_moderator" => {
            let target = details.get("target").and_then(|v| v.as_str()).unwrap_or("");
            if target.is_empty() {
                Err("Missing target in action log".into())
            } else {
                let resp = set_moderator(state, &json!({ "email": target, "active": true }), ctx);
                if resp.status().is_success() {
                    Ok(format!("Restored moderator {}", target))
                } else {
                    Err("Failed to restore moderator".into())
                }
            }
        }
        "shadow_ban" => {
            let email = details
                .get("targetEmail")
                .or_else(|| details.get("email"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if email.is_empty() {
                Err("Missing target email".into())
            } else {
                let sb_file = state.cfg.data_dir.join("shadow_bans.json");
                let mut sb = state.store.read_document(&sb_file, json!([]));
                let norm = mitch_lib::auth::normalize_email(email);
                if let Some(arr) = sb.as_array_mut() {
                    arr.retain(|x| {
                        mitch_lib::auth::normalize_email(x.as_str().unwrap_or("")) != norm
                    });
                    let _ = state.store.write_document(&sb_file, &sb);
                }
                Ok(format!("Unshadow-banned {}", email))
            }
        }
        "add_coins" | "gift_coins" => {
            let email = details
                .get("targetEmail")
                .or_else(|| details.get("email"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let amount = details
                .get("amount")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            if email.is_empty() || amount <= 0.0 {
                Err("Missing target email or positive amount in action log".into())
            } else {
                mitch_lib::coins::add_coins(
                    &state.store,
                    &state.cfg.data_dir,
                    email,
                    -amount,
                    1.0,
                    "reverted admin action",
                );
                Ok(format!("Deducted {} coins from {}", amount, email))
            }
        }
        "matrix_ban_user" | "matrix_chat_ban" => {
            let target = details
                .get("targetUserId")
                .or_else(|| details.get("userId"))
                .or_else(|| details.get("email"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if target.is_empty() {
                Err("Cannot revert Matrix ban: missing target user ID in action details".into())
            } else {
                let state_clone = state.clone();
                let target_clone = target.clone();
                tokio::spawn(async move {
                    let _ = crate::routes::matrix::unban_matrix_user_all_rooms(
                        &state_clone.id_secret,
                        &state_clone.store,
                        &state_clone.cfg.data_dir,
                        &target_clone,
                    )
                    .await;
                });
                Ok(format!("Unbanning {} across Matrix rooms", target))
            }
        }
        "matrix_add_channel" => {
            let alias = details
                .get("alias")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_lowercase();
            if alias.is_empty() {
                Err("Cannot revert channel creation: missing alias in action details".into())
            } else {
                let path = state.cfg.data_dir.join("matrix_official_rooms.json");
                let existing = state.store.read_document(&path, json!([]));
                let mut next = Vec::new();
                if let Some(arr) = existing.as_array() {
                    for item in arr {
                        let a = item
                            .get("alias")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_lowercase();
                        if a != alias {
                            next.push(item.clone());
                        }
                    }
                }
                let _ = state.store.write_document(&path, &Value::Array(next));
                Ok(format!(
                    "Removed #{}:mitch.pro from official channels",
                    alias
                ))
            }
        }
        other => Err(format!(
            "Action '{}' cannot be automatically reverted",
            other
        )),
    };

    match revert_result {
        Ok(msg) => {
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                &admin_email,
                "revert_action",
                json!({
                    "revertedActionId": action_id,
                    "originalAction": action_name,
                    "message": msg,
                }),
            );
            json_response(
                200,
                json!({ "ok": true, "message": msg, "reverted": action_name }),
            )
        }
        Err(err) => json_response(400, json!({ "error": err })),
    }
}

/// `POST /api/admin/moderator-requests/resolve` — approve/reject a pending
/// moderator request; approval executes the action server-side.
fn resolve_moderator_request(state: &Arc<AppState>, body: &Value, ctx: &AdminCtx) -> Response {
    let id = body.get("id").and_then(|v| v.as_str()).unwrap_or("").trim();
    let decision = body
        .get("decision")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let note = body
        .get("note")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .chars()
        .take(300)
        .collect::<String>();
    if id.is_empty() || (decision != "approve" && decision != "reject") {
        return json_response(
            400,
            json!({ "error": "request id and approve/reject decision required" }),
        );
    }
    let mut requests = mitch_lib::admin::load_moderator_requests(&state.store, &state.cfg.data_dir);
    let Some(pos) = requests
        .iter()
        .position(|req| req.get("id").and_then(|v| v.as_str()) == Some(id))
    else {
        return json_response(404, json!({ "error": "request not found" }));
    };
    if requests[pos].get("status").and_then(|v| v.as_str()) != Some("pending") {
        return json_response(409, json!({ "error": "request already resolved" }));
    }
    let admin_email = ctx.email(state);
    requests[pos]["resolvedAt"] = json!(now_millis());
    requests[pos]["resolvedBy"] = json!(admin_email);
    requests[pos]["note"] = json!(note);
    let action = requests[pos]
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let payload = requests[pos].get("payload").cloned().unwrap_or(json!({}));
    let requested_by = requests[pos]
        .get("requestedByEmail")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if decision == "reject" {
        requests[pos]["status"] = json!("rejected");
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &admin_email,
            "moderator_request_rejected",
            json!({ "id": id, "action": action, "requestedBy": requested_by }),
        );
        mitch_lib::admin::save_moderator_requests(&state.store, &state.cfg.data_dir, &requests);
        return json_response(
            200,
            json!({ "ok": true, "request": mitch_lib::admin::public_moderator_request(&requests[pos]) }),
        );
    }
    match execute_moderator_approved_action(state, &action, &payload, &admin_email, &requested_by) {
        Ok(result) => {
            requests[pos]["status"] = json!("approved");
            requests[pos]["result"] = result.clone();
            requests[pos]["error"] = json!("");
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                &admin_email,
                "moderator_request_approved",
                json!({ "id": id, "action": action, "requestedBy": requested_by }),
            );
            mitch_lib::admin::save_moderator_requests(&state.store, &state.cfg.data_dir, &requests);
            json_response(
                200,
                json!({ "ok": true, "result": result, "request": mitch_lib::admin::public_moderator_request(&requests[pos]) }),
            )
        }
        Err((status, error)) => {
            requests[pos]["status"] = json!("failed");
            requests[pos]["error"] = json!(error.clone());
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                &admin_email,
                "moderator_request_failed",
                json!({ "id": id, "action": action, "requestedBy": requested_by, "error": error }),
            );
            mitch_lib::admin::save_moderator_requests(&state.store, &state.cfg.data_dir, &requests);
            json_response(
                status,
                json!({ "error": error, "request": mitch_lib::admin::public_moderator_request(&requests[pos]) }),
            )
        }
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn user_agent(headers: &HeaderMap) -> String {
    let raw = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");
    let sanitized: String = raw.chars().filter(|c| !c.is_control()).take(200).collect();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

fn valid_email(email: &str) -> bool {
    let mut parts = email.split('@');
    let local = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("");
    !local.is_empty() && !rest.is_empty() && rest.contains('.') && parts.next().is_none()
}

/// `POST /api/admin/ban-account` — server.js:10627-10679.
/// Shared toggle for a simple per-user ban `HashSet` persisted to its own
/// JSON file — the same shape as `shadow_bans`/`restricted-mode` above, just
/// parameterized so `vm_bans` and `byo_os_bans` don't duplicate it twice.
fn toggle_ban_set(
    state: &Arc<AppState>,
    body: &Value,
    ctx: &AdminCtx,
    headers: &HeaderMap,
    set: &std::sync::RwLock<std::collections::HashSet<String>>,
    file: &str,
    action_name: &str,
) -> Response {
    if !ctx.is_admin(state) {
        return forbidden();
    }
    let target = super::resolve_admin_target(
        state,
        body.get("email").and_then(Value::as_str).unwrap_or(""),
    );
    if !valid_email(&target) {
        return json_response(
            400,
            json!({"error":"Choose an existing account by username or email."}),
        );
    }
    if mitch_lib::auth::is_admin_email(&state.store, &target) {
        return json_response(
            403,
            json!({"error":"Admins and owners cannot be restricted from this panel."}),
        );
    }
    if body.get("banned").is_some_and(|v| !v.is_boolean()) {
        return json_response(400, json!({"error":"banned must be true or false."}));
    }
    let active;
    {
        let mut bans = set.write().unwrap_or_else(|e| e.into_inner());
        let mut updated = bans.clone();
        active = body
            .get("banned")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| !bans.contains(&target));
        if active {
            updated.insert(target.clone());
        } else {
            updated.remove(&target);
        }
        let values: Vec<_> = updated.iter().cloned().collect();
        if state
            .store
            .write_document(&state.cfg.base_dir.join(file), &json!(values))
            .is_err()
        {
            return json_response(
                500,
                json!({"error":"Could not save the restriction. Nothing changed."}),
            );
        }
        *bans = updated;
    }
    let actor = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &ctx.sid)
        .unwrap_or_else(|| "moderator".to_string());
    let role = if ctx.is_admin(state) {
        "admin"
    } else {
        "moderator"
    };
    mitch_lib::admin::log_admin_action(
        &state.store,
        &state.cfg.data_dir,
        &actor,
        action_name,
        json!({
            "target": target,
            "reason": body.get("reason").and_then(Value::as_str).unwrap_or(""),
            "active": active,
            "ip": ctx.ip,
            "userAgent": headers
                .get("user-agent")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("unknown"),
            "role": role,
        }),
    );
    json_response(200, json!({ "ok": true, "active": active }))
}

fn queue_account_action(
    state: &Arc<AppState>,
    ctx: &AdminCtx,
    action: &str,
    body: &Value,
) -> Response {
    match mitch_lib::admin::create_moderator_action_request(
        &state.store,
        &state.id_secret,
        &state.cfg.data_dir,
        &ctx.sid,
        &json!({ "action":action, "payload":body }),
    ) {
        Ok(request) => json_response(202, json!({ "ok":true, "pending":true, "request":request })),
        Err(error) => json_response(error.status, json!({ "error":error.message })),
    }
}

fn apply_account_ban(
    state: &Arc<AppState>,
    raw_email: &str,
    reason: &str,
    duration: Option<&Value>,
    actor: &str,
) -> Result<Value, (u16, String)> {
    let raw_email = super::resolve_admin_target(state, raw_email);
    if !valid_email(raw_email.trim()) {
        return Err((400, "valid email required".into()));
    }
    let target = mitch_lib::auth::normalize_email(&raw_email);
    if mitch_lib::auth::is_admin_email(&state.store, &target) {
        return Err((403, "cannot ban admins or owners from this panel".into()));
    }
    let reason =
        mitch_lib::bans::required_reason(reason).map_err(|error| (400, error.to_string()))?;
    let duration_ms =
        mitch_lib::bans::duration_millis(duration).map_err(|error| (400, error.to_string()))?;
    let expires_at = duration_ms.map(|ms| now_millis() + ms);
    mitch_lib::bans::blacklist_insert_until(&state.store, &target, &reason, actor, expires_at)
        .map_err(|_| (500, "Could not save the account ban. Try again.".into()))?;
    crate::ws::disconnect_banned_user(state, &target, &reason);
    spawn_matrix_purge(
        state.id_secret.clone(),
        state.store.clone(),
        state.cfg.data_dir.clone(),
        crate::routes::matrix::resolve_matrix_user_id_for_email(state, &target),
        reason.clone(),
    );
    crate::chat_cleanup::clear_relay(state, &target);
    let _ = mitch_lib::chat_cleanup::remove_files(&state.store);
    mitch_lib::coins::add_admin_notification(
        &state.store,
        &state.cfg.data_dir,
        &target,
        "Account banned",
        &format!("Your account was banned. Reason: {reason}"),
        actor,
        "",
        "",
    );
    Ok(
        json!({ "ok":true, "targetEmail":target, "reason":reason, "duration":duration.and_then(Value::as_str).unwrap_or("permanent"), "expiresAt":expires_at }),
    )
}

pub(super) fn restore_account_access(
    state: &Arc<AppState>,
    raw_email: &str,
) -> Result<Value, (u16, String)> {
    let raw_email = super::resolve_admin_target(state, raw_email);
    if !valid_email(raw_email.trim()) {
        return Err((400, "valid email required".into()));
    }
    let target = mitch_lib::auth::normalize_email(&raw_email);
    let mut existed = mitch_lib::bans::blacklist_remove(&state.store, &target, &raw_email)
        .map_err(|_| {
            (
                500,
                "Could not remove the account ban. Try again.".to_string(),
            )
        })?;
    {
        let mut bans = state.shadow_bans.write().unwrap_or_else(|e| e.into_inner());
        let mut updated = bans.clone();
        updated.retain(|email| mitch_lib::auth::normalize_email(email) != target);
        existed |= updated.len() != bans.len();
        let entries: Vec<&String> = updated.iter().collect();
        state
            .store
            .write_document(
                &state.cfg.data_dir.join("shadow_bans.json"),
                &json!(entries),
            )
            .map_err(|_| {
                (
                    500,
                    "Could not finish restoring account access. Try again.".to_string(),
                )
            })?;
        *bans = updated;
    }
    let removed_ips =
        mitch_lib::bans::banned_ips_remove_for_account(&state.store, &target, &raw_email, None);
    existed |= !removed_ips.is_empty();
    let matrix_user_id = crate::routes::matrix::resolve_matrix_user_id_for_email(state, &target);
    spawn_matrix_unban(
        state.id_secret.clone(),
        state.store.clone(),
        state.cfg.data_dir.clone(),
        matrix_user_id,
    );
    Ok(json!({ "ok":true, "targetEmail":target, "existed":existed }))
}

fn account_action(
    state: &Arc<AppState>,
    body: &Value,
    ctx: &AdminCtx,
    headers: &HeaderMap,
    ban: bool,
) -> Response {
    let action = if ban { "ban_account" } else { "unban_account" };
    if !ctx.is_admin(state) {
        return queue_account_action(state, ctx, action, body);
    }
    let actor = ctx.email(state);
    let email = body.get("email").and_then(Value::as_str).unwrap_or("");
    let result = if ban {
        apply_account_ban(
            state,
            email,
            body.get("reason").and_then(Value::as_str).unwrap_or(""),
            body.get("duration"),
            &actor,
        )
    } else {
        restore_account_access(state, email)
    };
    match result {
        Ok(result) => {
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                &actor,
                action,
                json!({ "targetEmail":result["targetEmail"], "reason":result.get("reason"), "duration":result.get("duration"), "expiresAt":result.get("expiresAt"), "ip":ctx.ip, "userAgent":user_agent(headers), "role":"admin" }),
            );
            json_response(200, result)
        }
        Err((status, error)) => json_response(status, json!({ "error":error })),
    }
}

fn ban_account(
    state: &Arc<AppState>,
    body: &Value,
    ctx: &AdminCtx,
    headers: &HeaderMap,
) -> Response {
    account_action(state, body, ctx, headers, true)
}

fn unban_account(
    state: &Arc<AppState>,
    body: &Value,
    ctx: &AdminCtx,
    headers: &HeaderMap,
) -> Response {
    account_action(state, body, ctx, headers, false)
}

/// `POST /api/admin/chat-reports/resolve` — action 'delete' or resolve.
fn chat_report_resolve(state: &Arc<AppState>, body: &Value, ctx: &AdminCtx) -> Response {
    let report_ts = body.get("ts").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let action = body
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let actor = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &ctx.sid)
        .unwrap_or_else(|| "admin".to_string());
    let file = state.cfg.data_dir.join("chat_reports.json");
    let reports = state
        .store
        .read_document(&file, json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    if action == "delete" {
        let filtered: Vec<Value> = reports
            .into_iter()
            .filter(|r| r.get("ts").and_then(|v| v.as_f64()) != Some(report_ts))
            .collect();
        let _ = state.store.write_document(&file, &json!(filtered));
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &actor,
            "chat_report_delete",
            json!({ "ts": report_ts }),
        );
        return json_response(200, json!({ "ok": true, "deleted": true }));
    }
    let mut found = false;
    let new_reports: Vec<Value> = reports
        .into_iter()
        .map(|r| {
            if r.get("ts").and_then(|v| v.as_f64()) == Some(report_ts) {
                let mut r = r;
                r["status"] = json!("Resolved");
                found = true;
                r
            } else {
                r
            }
        })
        .collect();
    if found {
        let _ = state.store.write_document(&file, &json!(new_reports));
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &actor,
            "chat_report_resolve",
            json!({ "ts": report_ts }),
        );
        return json_response(200, json!({ "ok": true, "resolved": true }));
    }
    json_response(404, json!({ "error": "report not found" }))
}

/// `POST /api/admin/profile-reports/resolve` — server.js:8874-8896.
fn profile_report_resolve(state: &Arc<AppState>, body: &Value, ctx: &AdminCtx) -> Response {
    let admin_email = ctx.email(state);
    let id = body
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let status = body
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("reviewed")
        .trim()
        .to_lowercase();
    let allowed = ["needs review", "reviewed", "dismissed", "action taken"];
    if id.is_empty() {
        return json_response(400, json!({ "error": "report id required" }));
    }
    if !allowed.contains(&status.as_str()) {
        return json_response(400, json!({ "error": "invalid status" }));
    }
    let file = state.cfg.data_dir.join("profile_reports.json");
    let mut reports = state
        .store
        .read_document(&file, json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let Some(pos) = reports.iter().position(|row| {
        row.get("id")
            .and_then(|v| v.as_str())
            .map(|v| v.trim().to_string())
            == Some(id.clone())
    }) else {
        return json_response(404, json!({ "error": "report not found" }));
    };
    // status === 'needs review' ? 'Needs review' : title-case.
    let title_case = if status == "needs review" {
        "Needs review".to_string()
    } else {
        status
            .split_whitespace()
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    reports[pos]["status"] = json!(title_case);
    reports[pos]["resolvedBy"] = json!(mitch_lib::auth::normalize_email(&admin_email));
    reports[pos]["resolvedAt"] = json!(now_millis());
    reports[pos]["resolution"] = json!(body
        .get("resolution")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .chars()
        .take(300)
        .collect::<String>());
    let _ = state.store.write_document(&file, &json!(reports));
    mitch_lib::admin::log_admin_action(
        &state.store,
        &state.cfg.data_dir,
        &admin_email,
        "resolve_profile_report",
        json!({
            "id": id,
            "target": reports[pos].get("targetEmail").cloned().unwrap_or_default(),
            "status": reports[pos].get("status").cloned().unwrap_or_default(),
        }),
    );
    json_response(
        200,
        json!({ "ok": true, "reports": mitch_lib::admin::public_profile_reports(&state.store, &state.cfg.data_dir, 250) }),
    )
}

/// `executeModeratorApprovedAction` — the approved-action engine
/// (server.js:6836-7006). Delegates economy actions to economy.rs helpers.
pub fn execute_moderator_approved_action(
    state: &Arc<AppState>,
    action: &str,
    raw_payload: &Value,
    approver_email: &str,
    requester_email: &str,
) -> Result<Value, (u16, String)> {
    let payload = mitch_lib::admin::clean_moderator_action_payload(action, raw_payload)
        .map_err(|e| (e.status, e.message))?;
    let actor = if approver_email.is_empty() {
        "admin"
    } else {
        approver_email
    };
    let s = |k: &str| payload.get(k).cloned().unwrap_or(Value::Null);
    let requested_by = || {
        if requester_email.is_empty() {
            json!({})
        } else {
            json!({ "requestedBy": requester_email })
        }
    };
    match action {
        "grant_premium" => {
            if !mitch_lib::admin::can_grant_premium_email(&state.store, actor) {
                return Err((403, "approver cannot grant premium".into()));
            }
            let target_raw = s("targetEmail").as_str().unwrap_or("").to_string();
            let target_raw = super::resolve_admin_target(state, &target_raw);
            let target_email = mitch_lib::auth::normalize_email(&target_raw);
            let reason = s("reason")
                .as_str()
                .unwrap_or("free premium granted by admin")
                .to_string();
            if super::economy::is_premium_email(state, &target_email) {
                return Err((400, "That user is already Premium.".into()));
            }
            super::economy::grant_premium_application(state, &target_raw, actor, &reason);
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                actor,
                "grant_premium",
                json!({ "targetEmail": target_email, "reason": reason }).merge(requested_by()),
            );
            Ok(json!({ "ok": true, "targetEmail": target_raw }))
        }
        "revoke_premium" => {
            if !mitch_lib::admin::can_grant_premium_email(&state.store, actor) {
                return Err((403, "approver cannot revoke premium".into()));
            }
            let email_raw = s("email").as_str().unwrap_or("").to_string();
            let email_raw = super::resolve_admin_target(state, &email_raw);
            let norm = mitch_lib::auth::normalize_email(&email_raw);
            let reason = s("reason")
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| "premium revoked by admin".to_string());
            if !super::economy::revoke_premium_in_applications(state, &norm, actor, &json!(reason))
            {
                return Err((404, "active premium user not found".into()));
            }
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                actor,
                "revoke_premium",
                json!({ "targetEmail": email_raw, "reason": reason }).merge(requested_by()),
            );
            Ok(json!({ "ok": true, "targetEmail": email_raw }))
        }
        "send_notification" => {
            super::economy::send_admin_notice(state, raw_payload, actor, requester_email)
        }
        "unsend_notification" => {
            super::economy::remove_admin_notice(state, raw_payload, actor, requester_email)
        }
        "gift_coins" => {
            super::economy::moderator_gift_coins(state, &payload, actor, requester_email)
        }
        "burn_coins" => {
            super::economy::moderator_burn_coins(state, &payload, actor, requester_email)
        }
        "economy_multiplier" => {
            let multiplier = s("multiplier").as_f64().unwrap_or(f64::NAN);
            if !multiplier.is_finite() || multiplier <= 0.0 || multiplier > 25.0 {
                return Err((400, "invalid multiplier".into()));
            }
            state.set_coin_multiplier(multiplier);
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                actor,
                "set_multiplier",
                json!({ "multiplier": multiplier }).merge(requested_by()),
            );
            Ok(json!({ "ok": true, "multiplier": multiplier }))
        }
        "broadcast" => {
            let msg = s("msg").as_str().unwrap_or("").to_string();
            if msg.is_empty() {
                return Err((400, "message required".into()));
            }
            // WS fan-out (server.js:7093-7100).
            crate::ws::broadcast(
                state,
                crate::ws::WsRecipients::All,
                json!({
                    "type": "admin_broadcast",
                    "message": msg,
                })
                .to_string(),
            );
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                actor,
                "broadcast",
                json!({ "message": msg }).merge(requested_by()),
            );
            Ok(json!({ "ok": true }))
        }
        "shadow_ban" => Err((
            410,
            "Shadow bans have been retired. Use account bans.".into(),
        )),
        "ban_account" | "unban_account" => {
            let email = payload.get("email").and_then(Value::as_str).unwrap_or("");
            let result = if action == "ban_account" {
                apply_account_ban(
                    state,
                    email,
                    payload.get("reason").and_then(Value::as_str).unwrap_or(""),
                    payload.get("duration"),
                    actor,
                )?
            } else {
                restore_account_access(state, email)?
            };
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                actor,
                action,
                json!({ "targetEmail":result["targetEmail"], "reason":result.get("reason"), "duration":result.get("duration"), "expiresAt":result.get("expiresAt") })
                    .merge(requested_by()),
            );
            Ok(result)
        }
        "prox_block" => {
            let domain = s("domain").as_str().unwrap_or("").to_string();
            if domain.is_empty() {
                return Err((400, "domain required".into()));
            }
            state
                .prox_blocklist
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .insert(domain.clone());
            let arr: Vec<Value> = state
                .prox_blocklist
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .map(|d| json!(d))
                .collect();
            let _ = state.store.write_document(
                &state.cfg.base_dir.join("data/prox_blocklist.json"),
                &json!(arr),
            );
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                actor,
                "prox_block",
                json!({ "domain": domain }).merge(requested_by()),
            );
            Ok(json!({ "ok": true, "domain": domain }))
        }
        "content_mirror" => {
            let new_url = s("url").as_str().unwrap_or("").to_string();
            let ok = new_url.to_lowercase().starts_with("http://")
                || new_url.to_lowercase().starts_with("https://");
            if !ok {
                return Err((400, "valid mirror URL required".into()));
            }
            let sites_path = state.cfg.base_dir.join("data/sites");
            let Ok(contents) = std::fs::read_to_string(&sites_path) else {
                return Err((500, "internal_error".into()));
            };
            static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
            let re = RE.get_or_init(|| {
                regex::Regex::new(
                    r"url https://docs\.google\.com/document/d/[^\s]+ Mitch\.pro Mirrors",
                )
                .unwrap_or_else(|e| {
                    tracing::error!("mirror regex failed: {e}");
                    regex::Regex::new("$^").unwrap_or_else(|_| {
                        regex::Regex::new("$^").unwrap_or_else(|e| {
                            tracing::error!("fallback regex failed: {e}");
                            regex::Regex::new("$^").unwrap_or_else(|_| unreachable!())
                        })
                    })
                })
            });
            let updated = re
                .replace(&contents, format!("url {new_url} Mitch.pro Mirrors"))
                .to_string();
            let _ = std::fs::write(&sites_path, &updated);
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                actor,
                "update_mirror",
                json!({ "url": new_url }).merge(requested_by()),
            );
            Ok(json!({ "ok": true, "url": new_url }))
        }
        "content_featured" => {
            let href = s("href").as_str().unwrap_or("").to_string();
            if href.is_empty() {
                return Err((400, "href required".into()));
            }
            *state
                .featured_game_href
                .write()
                .unwrap_or_else(|e| e.into_inner()) = href.clone();
            let _ = state
                .store
                .write_document(&state.cfg.data_dir.join("featured_game.json"), &json!(href));
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                actor,
                "set_featured",
                json!({ "href": href }).merge(requested_by()),
            );
            Ok(json!({ "ok": true, "href": href }))
        }
        "moderator_role" => {
            let target_raw = s("email").as_str().unwrap_or("").trim().to_string();
            let target_raw = super::resolve_admin_target(state, &target_raw);
            let target = mitch_lib::auth::normalize_email(&target_raw);
            let active = s("active").as_bool() == Some(true);
            let mut mods = mitch_lib::auth::moderator_emails(&state.store);
            if active {
                if !mods
                    .iter()
                    .any(|m| mitch_lib::auth::normalize_email(m) == target)
                {
                    mods.push(target_raw.clone());
                }
            } else {
                mods.retain(|m| mitch_lib::auth::normalize_email(m) != target);
            }
            let _ = state.store.write_document(
                &state.cfg.base_dir.join("data/moderators.json"),
                &json!(mods),
            );
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                actor,
                if active {
                    "add_moderator"
                } else {
                    "remove_moderator"
                },
                json!({ "target": target_raw }).merge(requested_by()),
            );
            spawn_staff_sync(
                state.id_secret.clone(),
                state.store.clone(),
                state.cfg.data_dir.clone(),
            );
            Ok(json!({ "ok": true, "moderators": mods }))
        }
        "admin_role" => {
            let target_raw = s("email").as_str().unwrap_or("").trim().to_string();
            let target_raw = super::resolve_admin_target(state, &target_raw);
            let target = mitch_lib::auth::normalize_email(&target_raw);
            let active = s("active").as_bool() == Some(true);
            let admins_file = state.cfg.base_dir.join("data/admins.json");
            let mut doc = state.store.read_document(&admins_file, json!({}));
            let mut admins: Vec<String> = doc
                .get("admins")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();

            if active {
                if !admins
                    .iter()
                    .any(|m| mitch_lib::auth::normalize_email(m) == target)
                {
                    admins.push(target_raw.clone());
                }
            } else {
                admins.retain(|m| mitch_lib::auth::normalize_email(m) != target);
            }

            if let Some(obj) = doc.as_object_mut() {
                obj.insert("admins".to_string(), json!(admins));
            } else {
                doc = json!({ "admins": admins });
            }

            let _ = state.store.write_document(&admins_file, &doc);
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                actor,
                if active { "add_admin" } else { "remove_admin" },
                json!({ "target": target_raw }).merge(requested_by()),
            );
            spawn_staff_sync(
                state.id_secret.clone(),
                state.store.clone(),
                state.cfg.data_dir.clone(),
            );
            Ok(json!({ "ok": true, "admins": admins }))
        }
        "moderator_panel" => {
            let links = mitch_lib::admin::sanitize_moderator_panel_links(&s("links"));
            let links_json = json!(links.clone());
            let _ = state.store.write_document(
                &state.cfg.data_dir.join("moderator_panel.json"),
                &json!({ "links": links }),
            );
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                actor,
                "update_moderator_panel",
                json!({ "linkCount": links_json.as_array().map(|a| a.len()).unwrap_or(0) })
                    .merge(requested_by()),
            );
            Ok(json!({ "ok": true, "links": links_json }))
        }
        _ => Err((400, "unknown moderator action".into())),
    }
}

/// Tiny merge helper for building log details with an optional requestedBy.
trait MergeJson {
    fn merge(self, other: Value) -> Value;
}

impl MergeJson for Value {
    fn merge(self, other: Value) -> Value {
        let mut left = self;
        if let (Some(a), Some(b)) = (left.as_object_mut(), other.as_object()) {
            for (k, v) in b {
                a.insert(k.clone(), v.clone());
            }
        }
        left
    }
}

/// `GET /api/admin/moderation/lookup?q=<email or Matrix ID>` — the unified
/// moderation profile card: resolves the query to every known Matrix user id
/// for that person, then aggregates account-level ban/shadow-ban state with
/// per-room membership, power level, and local ban/mute settings across the
/// official rooms plus every room tracked in matrix_room_settings.json.
pub async fn lookup_profile(state: &Arc<AppState>, search: &str) -> Response {
    let query_map = crate::handler::query(search);
    let raw = query_map
        .get("q")
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_string();
    if raw.is_empty() {
        return json_response(
            400,
            json!({ "error": "q (email or Matrix user ID) is required" }),
        );
    }

    let norm_email = super::resolve_admin_target(state, &raw);
    let mut matrix_ids = crate::routes::matrix::resolve_all_matrix_user_ids_for_target(
        state,
        if norm_email.is_empty() {
            &raw
        } else {
            &norm_email
        },
    );
    if matrix_ids.is_empty() {
        let resolved = crate::routes::matrix::resolve_matrix_user_id_for_email(state, &norm_email);
        if !resolved.is_empty() {
            matrix_ids.push(resolved);
        } else if raw.starts_with('@') {
            matrix_ids.push(raw.clone());
        }
    }

    let ban_entry = mitch_lib::bans::blacklist_get(&state.store, &norm_email, &raw.to_lowercase());
    let shadow_banned = state
        .shadow_bans
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&norm_email);
    let vm_banned = state
        .vm_bans
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&norm_email);
    let byo_os_banned = state
        .byo_os_bans
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&norm_email);

    let settings_file = state.cfg.data_dir.join("matrix_room_settings.json");
    let settings_all = state.store.read_document(&settings_file, json!({}));

    let mut room_ids: Vec<(String, Option<String>)> = Vec::new();
    for (alias, name, topic) in
        crate::routes::matrix::all_official_rooms(&state.store, &state.cfg.data_dir)
    {
        if let Ok(room_id) =
            crate::routes::matrix::ensure_official_room(&state.id_secret, &alias, &name, &topic)
                .await
        {
            room_ids.push((room_id, Some(alias)));
        }
    }
    if let Some(map) = settings_all.as_object() {
        for room_id in map.keys() {
            if !room_ids.iter().any(|(id, _)| id == room_id) {
                room_ids.push((room_id.clone(), None));
            }
        }
    }

    let admin_tok = crate::routes::matrix::get_system_admin_matrix_token(&state.id_secret)
        .await
        .ok();

    let mut rooms_out: Vec<Value> = Vec::new();
    if !matrix_ids.is_empty() {
        for (room_id, alias) in &room_ids {
            let room_settings = settings_all.get(room_id).cloned().unwrap_or(json!({}));
            let banned_in_settings = room_settings
                .get("bannedUsers")
                .and_then(|v| v.as_object())
                .map(|m| matrix_ids.iter().any(|uid| m.contains_key(uid)))
                .unwrap_or(false);
            let muted_in_settings = room_settings
                .get("mutedUsers")
                .and_then(|v| v.as_object())
                .map(|m| matrix_ids.iter().any(|uid| m.contains_key(uid)))
                .unwrap_or(false);

            let mut membership = "unknown".to_string();
            let mut power_level: i64 = 0;

            if let Some(tok) = &admin_tok {
                let mut headers = HeaderMap::new();
                if let Ok(hv) = axum::http::HeaderValue::from_str(&format!("Bearer {tok}")) {
                    headers.insert("Authorization", hv);
                }
                let encoded_room =
                    url::form_urlencoded::byte_serialize(room_id.as_bytes()).collect::<String>();
                for uid in &matrix_ids {
                    let encoded_uid =
                        url::form_urlencoded::byte_serialize(uid.as_bytes()).collect::<String>();
                    if let Ok((status, _, bytes)) = crate::routes::matrix::call_conduit(
                        &format!(
                            "/_matrix/client/v3/rooms/{encoded_room}/state/m.room.member/{encoded_uid}"
                        ),
                        Method::GET,
                        Some(headers.clone()),
                        None,
                    )
                    .await
                    {
                        if status.is_success() {
                            if let Ok(member) = serde_json::from_slice::<Value>(&bytes) {
                                if let Some(m) = member.get("membership").and_then(|v| v.as_str()) {
                                    membership = m.to_string();
                                }
                            }
                        }
                    }
                }
                if let Ok((status, _, bytes)) = crate::routes::matrix::call_conduit(
                    &format!("/_matrix/client/v3/rooms/{encoded_room}/state/m.room.power_levels"),
                    Method::GET,
                    Some(headers),
                    None,
                )
                .await
                {
                    if status.is_success() {
                        if let Ok(pl) = serde_json::from_slice::<Value>(&bytes) {
                            if let Some(users) = pl.get("users").and_then(|v| v.as_object()) {
                                for uid in &matrix_ids {
                                    if let Some(lvl) = users.get(uid).and_then(|v| v.as_i64()) {
                                        power_level = lvl;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if membership == "unknown"
                && !banned_in_settings
                && !muted_in_settings
                && power_level == 0
            {
                continue;
            }

            rooms_out.push(json!({
                "roomId": room_id,
                "alias": alias,
                "membership": membership,
                "powerLevel": power_level,
                "bannedInSettings": banned_in_settings,
                "mutedInSettings": muted_in_settings,
            }));
        }
    }

    json_response(
        200,
        json!({
            "query": raw,
            "email": norm_email,
            "matrixUserIds": matrix_ids,
            "accountBanned": ban_entry.is_some(),
            "banReason": ban_entry.as_ref().and_then(|e| e.get("reason")).cloned().unwrap_or(Value::Null),
            "bannedAt": ban_entry.as_ref().and_then(|e| e.get("banned_at")).cloned().unwrap_or(Value::Null),
            "shadowBanned": shadow_banned,
            "vmBanned": vm_banned,
            "byoOsBanned": byo_os_banned,
            "rooms": rooms_out,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ban_test_state() -> (Arc<AppState>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "mitch-account-ban-test-{}-{}",
            mitch_lib::crypto::random_bytes_hex(8),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap();
        let store = Arc::new(mitch_lib::data::DataStore::open(&dir, &dir.join("data")).unwrap());
        let cfg = crate::hosts::SiteConfig {
            base_dir: dir.clone(),
            data_dir: dir.join("data"),
            ..crate::hosts::SiteConfig::load()
        };
        store
            .write_document(
                &cfg.data_dir.join("admins.json"),
                &json!({ "owners":["owner@example.com"], "admins":["admin@example.com"] }),
            )
            .unwrap();
        store
            .write_document(
                &cfg.data_dir.join("moderators.json"),
                &json!(["moderator@example.com"]),
            )
            .unwrap();
        (Arc::new(AppState::new(cfg, store)), dir)
    }

    fn ban_test_ctx(state: &AppState, email: &str) -> AdminCtx {
        let sid = mitch_lib::auth::make_email_id(email, 0, &state.id_secret);
        mitch_lib::auth::names_set(&state.store, &sid, email);
        AdminCtx {
            cookies: mitch_lib::auth::Cookies::default(),
            sid,
            ip: "192.0.2.1".into(),
        }
    }

    #[test]
    fn account_bans_disconnect_presence_and_restore_aliases_without_network_bans() {
        let (state, dir) = ban_test_state();
        let email = "firstlast@student.rjuhsd.us";
        state.user_presence.lock().unwrap().insert(
            email.into(),
            crate::state::UserPresence {
                last_seen: now_millis(),
                playing: "Dashboard".into(),
                ..Default::default()
            },
        );
        state.ws_broadcasts.lock().unwrap().insert(
            1,
            crate::ws::WsClient {
                email_norm: email.into(),
                ..Default::default()
            },
        );
        state
            .store
            .write_document(
                &state.cfg.data_dir.join("last_known_ips.json"),
                &json!({email:"192.0.2.1"}),
            )
            .unwrap();
        let result = apply_account_ban(
            &state,
            "First.Last+alias@student.mitch.pro",
            "  Repeated spam  ",
            None,
            "owner@example.com",
        )
        .unwrap();
        assert_eq!(result["reason"], "Repeated spam");
        assert!(mitch_lib::auth::banned_info_for_email(&state.store, email).is_some());
        assert_eq!(crate::ws::active_user_count(&state, now_millis()), 0);
        assert!(mitch_lib::bans::banned_ips_get(&state.store, "192.0.2.1").is_none());
        state
            .shadow_bans
            .write()
            .unwrap()
            .insert("First.Last+legacy@student.mitch.pro".into());
        mitch_lib::bans::banned_ips_insert(
            &state.store,
            "192.0.2.1",
            "another account",
            "owner",
            "other@example.com",
        );
        let restored = restore_account_access(&state, "First.Last@student.mitch.pro").unwrap();
        assert_eq!(restored["existed"], true);
        assert!(mitch_lib::auth::banned_info_for_email(&state.store, email).is_none());
        assert!(state.shadow_bans.read().unwrap().is_empty());
        assert!(mitch_lib::bans::banned_ips_get(&state.store, "192.0.2.1").is_some());
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn moderators_queue_bans_and_admin_approval_uses_the_same_flow() {
        let (state, dir) = ban_test_state();
        let owner = ban_test_ctx(&state, "owner@example.com");
        let moderator = ban_test_ctx(&state, "moderator@example.com");
        let headers = HeaderMap::new();
        let body = json!({ "email":"student@example.com", "reason":"spam", "duration":"3d" });
        let pending = handle(
            &state,
            &Method::POST,
            "/api/admin/ban-account",
            &headers,
            &body,
            &moderator,
        )
        .unwrap();
        assert_eq!(pending.status(), 202);
        assert!(
            mitch_lib::auth::banned_info_for_email(&state.store, "student@example.com").is_none()
        );
        let requests = mitch_lib::admin::load_moderator_requests(&state.store, &state.cfg.data_dir);
        let request = requests.first().unwrap();
        assert_eq!(request["payload"]["duration"], "3d");
        let approved = resolve_moderator_request(
            &state,
            &json!({ "id":request["id"], "decision":"approve" }),
            &owner,
        );
        assert_eq!(approved.status(), 200);
        assert!(
            mitch_lib::auth::banned_info_for_email(&state.store, "student@example.com").is_some()
        );
        assert_eq!(
            handle(
                &state,
                &Method::GET,
                "/api/admin/bans",
                &headers,
                &json!({}),
                &moderator
            )
            .unwrap()
            .status(),
            403
        );
        assert_eq!(
            handle(
                &state,
                &Method::GET,
                "/api/admin/bans",
                &headers,
                &json!({}),
                &owner
            )
            .unwrap()
            .status(),
            200
        );
        let entry =
            mitch_lib::auth::banned_info_for_email(&state.store, "student@example.com").unwrap();
        assert!(
            (entry["expires_at"].as_i64().unwrap() - now_millis() - 3 * 86_400_000).abs() < 1000
        );
        assert_eq!(unban_account(&state, &body, &owner, &headers).status(), 200);
        assert!(
            mitch_lib::auth::banned_info_for_email(&state.store, "student@example.com").is_none()
        );
        assert!(handle(
            &state,
            &Method::GET,
            "/api/admin/restricted-mode",
            &headers,
            &body,
            &owner
        )
        .is_none());
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bans_require_a_reason_and_validate_duration_before_saving() {
        let (state, dir) = ban_test_state();
        for (reason, duration) in [
            ("   ", json!("1d")),
            ("Spam", json!("forever")),
            ("Spam", json!(24)),
            ("Spam", Value::Null),
        ] {
            assert_eq!(
                apply_account_ban(
                    &state,
                    "student@example.com",
                    reason,
                    Some(&duration),
                    "admin"
                )
                .unwrap_err()
                .0,
                400
            );
            assert!(
                mitch_lib::auth::banned_info_for_email(&state.store, "student@example.com")
                    .is_none()
            );
        }
        let result = apply_account_ban(
            &state,
            "student@example.com",
            "Spam",
            Some(&json!("1d")),
            "admin",
        )
        .unwrap();
        assert!((result["expiresAt"].as_i64().unwrap() - now_millis() - 86_400_000).abs() < 1000);
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bans_reject_staff_and_invalid_targets() {
        let (state, dir) = ban_test_state();
        assert_eq!(
            apply_account_ban(&state, "owner@example.com", "reason", None, "admin")
                .unwrap_err()
                .0,
            403
        );
        assert_eq!(
            apply_account_ban(&state, "not-an-email", "reason", None, "admin")
                .unwrap_err()
                .0,
            400
        );
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn test_set_admin_add_and_remove() {
        let dir = std::env::temp_dir().join(format!(
            "mitch-test-admin-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap();
        let store = Arc::new(mitch_lib::data::DataStore::open(&dir, &dir.join("data")).unwrap());
        let mut cfg = crate::hosts::SiteConfig::load();
        cfg.base_dir = dir.clone();
        cfg.data_dir = dir.join("data");
        let state = Arc::new(AppState::new(cfg, store));

        let ctx = AdminCtx {
            cookies: mitch_lib::auth::Cookies::default(),
            sid: "test-sid".to_string(),
            ip: "127.0.0.1".to_string(),
        };

        // Add admin
        let add_resp = set_admin(
            &state,
            &json!({ "email": "newadmin@example.com", "active": true }),
            &ctx,
        );
        assert_eq!(add_resp.status(), axum::http::StatusCode::OK);

        let admins = mitch_lib::auth::admin_member_emails(&state.store);
        assert!(admins.contains(&"newadmin@example.com".to_string()));

        // Remove admin
        let remove_resp = set_admin(
            &state,
            &json!({ "email": "newadmin@example.com", "active": false }),
            &ctx,
        );
        assert_eq!(remove_resp.status(), axum::http::StatusCode::OK);

        let admins_after = mitch_lib::auth::admin_member_emails(&state.store);
        assert!(!admins_after.contains(&"newadmin@example.com".to_string()));
    }

    #[test]
    fn test_user_agent_sanitization() {
        let mut headers = axum::http::HeaderMap::new();
        let long_ua = "a".repeat(300);
        headers.insert(
            "user-agent",
            axum::http::HeaderValue::from_str(&long_ua).unwrap(),
        );
        let sanitized = user_agent(&headers);
        assert_eq!(sanitized.len(), 200);
    }
}
