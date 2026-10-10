//! `POST /api/admin/data` — the legacy passphrase-key admin dashboard
//! transport (server.js:11453-11724). `body.pw` authenticates via
//! `checkAdminPw` (admin/admin.key); `body.type` dispatches. The sid-based
//! global gate in mod.rs still applies to this path exactly as in the JS.

use super::{forbidden, AdminCtx, Resp};
use crate::state::AppState;
use axum::http::Method;
use axum::response::Response;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    body: &Value,
    _ctx: &AdminCtx,
) -> Resp {
    if path != "/api/admin/data" || *method != Method::POST {
        return None;
    }
    if !check_admin_pw(state, body) {
        return Some(forbidden());
    }
    let dtype = body
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    match dtype.as_str() {
        "content" => {
            let path = state.cfg.base_dir.join("admin/admin_app.html");
            let (body, ct) = match std::fs::read_to_string(&path) {
                Ok(html) => (html, "text/html; charset=utf-8"),
                Err(_) => ("<h1>admin_app.html not found</h1>".to_string(), "text/html"),
            };
            Some(
                Response::builder()
                    .status(200)
                    .header("content-type", ct)
                    .body(axum::body::Body::from(body))
                    .unwrap_or_else(|_| {
                        crate::errors::json_resp(500, json!({ "error": "internal_error" }))
                    }),
            )
        }
        "history" => Some(history(state, body)),
        "users" => Some(users(state)),
        "suggestions" => Some(suggestions(state)),
        "requests" => Some(requests(state)),
        "revoke" => Some(revoke(state, body)),
        "approve" => Some(approve(state, body).await),
        "appeal" => Some(appeal(state, body).await),
        "unsub" => Some(unsub(state, body).await),
        "newsletter_list" => Some(newsletter_list(state)),
        "newsletter_send" => Some(newsletter_send(state, body).await),
        "newsletter_add" => Some(newsletter_add(state, body)),
        "newsletter_remove" => Some(newsletter_remove(state, body)),
        "rl_list" => Some(rl_list(state)),
        "rl_reset" => Some(rl_reset(state, body)),
        "rl_reset_all" => Some(rl_reset_all(state)),
        "gen_token" => Some(gen_token(state)),
        _ => Some(json_response(400, json!({ "error": "unknown type" }))),
    }
}

fn json_response(code: u16, obj: Value) -> Response {
    crate::errors::json_resp(code, obj)
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `checkAdminPw(body.pw)` — plaintext compare against admin/admin.key.
fn check_admin_pw(state: &Arc<AppState>, body: &Value) -> bool {
    let pw = body.get("pw").and_then(|v| v.as_str()).unwrap_or("");
    if pw.is_empty() {
        return false;
    }
    match std::fs::read_to_string(state.cfg.base_dir.join("admin/admin.key")) {
        Ok(key) => mitch_lib::crypto::timing_safe_equal(pw.as_bytes(), key.trim().as_bytes()),
        Err(_) => false,
    }
}

fn session_log_doc(state: &Arc<AppState>) -> Vec<Value> {
    state
        .store
        .read_document(&state.cfg.data_dir.join("sessions.json"), json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// dtype `history` — session log entries for one email's sids.
fn history(state: &Arc<AppState>, body: &Value) -> Response {
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let uids: std::collections::HashSet<String> =
        mitch_lib::auth::names_sids_for_exact_email(&state.store, name)
            .into_iter()
            .collect();
    let entries: Vec<Value> = session_log_doc(state)
        .into_iter()
        .filter(|e| {
            uids.contains(
                &e.get("id")
                    .map(|v| v.as_str().unwrap_or("").to_string())
                    .unwrap_or_default(),
            )
        })
        .map(|e| {
            json!({
                "page": e.get("page").and_then(|v| v.as_str()).unwrap_or(""),
                "ts": e.get("timestamp").and_then(|v| v.as_str()).unwrap_or(""),
                "ip": e.get("ip").and_then(|v| v.as_str()).unwrap_or(""),
            })
        })
        .rev()
        .collect();
    json_response(200, json!({ "history": entries }))
}

/// dtype `users` — last activity per sid.
fn users(state: &Arc<AppState>) -> Response {
    let mut latest: std::collections::HashMap<String, (i64, String)> =
        std::collections::HashMap::new();
    for e in session_log_doc(state) {
        let uid = e
            .get("id")
            .map(|v| v.as_str().unwrap_or("").to_string())
            .unwrap_or_default();
        if uid.is_empty() {
            continue;
        }
        let ts = e
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(super::economy::parse_timestamp);
        let Some(ts) = ts else { continue };
        let page = e
            .get("page")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let entry = latest.entry(uid.clone()).or_insert((0, String::new()));
        if ts > entry.0 {
            *entry = (ts, page);
        }
    }
    let mut users: Vec<Value> = latest
        .into_iter()
        .map(|(uid, (logts, page))| {
            let name = mitch_lib::auth::names_get_email(&state.store, &uid)
                .unwrap_or_else(|| uid[..uid.len().min(20)].to_string());
            json!({
                "uid": uid,
                "name": name,
                "page": page,
                "logts": logts,
            })
        })
        .collect();
    users.sort_by(|a, b| {
        b.get("logts")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
            .partial_cmp(&a.get("logts").and_then(|v| v.as_f64()).unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    json_response(200, json!({ "users": users }))
}

/// dtype `suggestions`.
fn suggestions(state: &Arc<AppState>) -> Response {
    let mut sugs = state
        .store
        .read_document(&state.cfg.data_dir.join("suggestions.json"), json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    for s in sugs.iter_mut() {
        let id = s
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let name = mitch_lib::auth::names_get_email(&state.store, &id)
            .or_else(|| s.get("name").and_then(|v| v.as_str()).map(str::to_string))
            .unwrap_or_else(|| id.chars().take(20).collect::<String>());
        if let Some(obj) = s.as_object_mut() {
            obj.insert("name".into(), json!(name));
        }
    }
    sugs.sort_by(|a, b| {
        b.get("ts")
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            .cmp(&a.get("ts").and_then(|v| v.as_i64()).unwrap_or(0))
    });
    json_response(200, json!({ "suggestions": sugs }))
}

/// dtype `requests` — pending tokens, appeals, unsub requests, premium.
fn requests(state: &Arc<AppState>) -> Response {
    let tokens = state
        .store
        .read_document(&state.cfg.base_dir.join("data/tokens.json"), json!({}));
    let mut pending: Vec<Value> = tokens
        .as_object()
        .map(|m| {
            m.iter()
                .filter(|(_, d)| {
                    let infinite = d.get("infinite").and_then(|v| v.as_bool()).unwrap_or(false);
                    let used = d.get("used").and_then(|v| v.as_bool()).unwrap_or(false);
                    let claimed = d.get("claimed_domains").is_some();
                    !infinite && !(used && !claimed)
                })
                .map(|(tok, d)| {
                    json!({
                        "token": tok,
                        "email": d.get("email").and_then(|v| v.as_str()).unwrap_or("?"),
                        "name": d.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                        "enroll_ip": d.get("enroll_ip").and_then(|v| v.as_str()).unwrap_or(""),
                        "created_at": d.get("created_at").and_then(|v| v.as_f64()).unwrap_or(0.0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    pending.sort_by(|a, b| {
        a.get("created_at")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
            .partial_cmp(&b.get("created_at").and_then(|v| v.as_f64()).unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let appeals = state
        .store
        .read_document(&state.cfg.data_dir.join("appeals.json"), json!([]));
    let unsub_reqs = state
        .store
        .read_document(&state.cfg.data_dir.join("unsub_requests.json"), json!([]));
    let applications = state
        .store
        .read_document(&state.cfg.data_dir.join("applications.json"), json!([]));
    let premium_members: Vec<Value> = applications
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|app| {
                    app.get("status").and_then(|v| v.as_str()) == Some("approved")
                        && (app.get("type").and_then(|v| v.as_str()) == Some("premium")
                            || app.get("grantPremium").and_then(|v| v.as_bool()) == Some(true))
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    json_response(
        200,
        json!({
            "pending": pending,
            "appeals": appeals,
            "unsub_requests": unsub_reqs,
            "premium_members": premium_members,
        }),
    )
}

/// dtype `revoke` — remove all premium applications for one email.
fn revoke(state: &Arc<AppState>, body: &Value) -> Response {
    let email = body
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if email.is_empty() {
        return json_response(400, json!({ "error": "email required" }));
    }
    let file = state.cfg.data_dir.join("applications.json");
    let apps = state
        .store
        .read_document(&file, json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let norm = mitch_lib::auth::normalize_email(&email);
    let new_apps: Vec<Value> = apps
        .into_iter()
        .filter(|app| {
            let is_premium = app.get("type").and_then(|v| v.as_str()) == Some("premium")
                || app.get("grantPremium").and_then(|v| v.as_bool()) == Some(true);
            !(is_premium
                && mitch_lib::auth::normalize_email(
                    app.get("email").and_then(|v| v.as_str()).unwrap_or(""),
                ) == norm)
        })
        .collect();
    let _ = state.store.write_document(&file, &json!(new_apps));
    json_response(200, json!({ "ok": true }))
}

/// dtype `approve` — approve/approve_team/deny/blacklist flows.
async fn approve(state: &Arc<AppState>, body: &Value) -> Response {
    let tok = body.get("token").and_then(|v| v.as_str()).unwrap_or("");
    let email = body.get("email").and_then(|v| v.as_str()).unwrap_or("");
    let action = body.get("action").and_then(|v| v.as_str()).unwrap_or("");
    let tokens_file = state.cfg.base_dir.join("data/tokens.json");
    let mut tokens = state.store.read_document(&tokens_file, json!({}));
    match action {
        "approve" | "approve_team" => {
            let new_tok = mitch_lib::crypto::random_bytes_hex(24);
            if let Some(map) = tokens.as_object_mut() {
                map.insert(
                    new_tok.clone(),
                    json!({ "email": email, "created_at": now_millis() as f64 / 1000.0, "used": false }),
                );
                map.remove(tok);
            }
            let _ = state.store.write_document(&tokens_file, &tokens);
            if action == "approve_team" {
                // Silent approval + premium grant; no email.
                let norm = mitch_lib::auth::normalize_email(email);
                let apps_file = state.cfg.data_dir.join("applications.json");
                let mut apps = state
                    .store
                    .read_document(&apps_file, json!([]))
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let exists = apps.iter().any(|app| {
                    mitch_lib::auth::normalize_email(
                        app.get("email").and_then(|v| v.as_str()).unwrap_or(""),
                    ) == norm
                });
                if !exists {
                    let mut next = vec![json!({
                        "name": email.split('@').next().unwrap_or(""),
                        "email": email.to_lowercase(),
                        "type": "team",
                        "status": "approved",
                        "grantPremium": true,
                        "neverExpire": true,
                        "why": "Silent approved as Team via Admin",
                        "submitted_at": now_millis(),
                        "approved_at": now_millis(),
                    })];
                    next.extend(apps);
                    apps = next;
                    let _ = state.store.write_document(&apps_file, &json!(apps));
                }
                return json_response(200, json!({ "ok": true }));
            }
            let site_name = site_name(state);
            let link = format!("{}/claim.html?token={new_tok}", site_url_of(email, state));
            let subject = format!("Your {site_name} Access Has Been Approved");
            let html = super::legacy::make_access_status_html(
                    state,

                email,
                "Your Access Has Been Approved",
                &format!(
                    "Your access request has been approved. Click the link below to claim your account. Save your token in case you need it later: {new_tok}"
                ),
                &link,
                "Claim Account",
            );
            crate::routes::push::send_email_bg(state, email, &subject, &html);
            json_response(200, json!({ "ok": true }))
        }
        "deny" => {
            if let Some(map) = tokens.as_object_mut() {
                map.remove(tok);
            }
            let _ = state.store.write_document(&tokens_file, &tokens);
            let site_name = site_name(state);
            let subject = format!("Your {site_name} Access Request Was Not Approved");
            let html = super::legacy::make_access_status_html(
                    state,

                email,
                "Access Request Update",
                "Unfortunately, your access request was not approved at this time. If you think this is a mistake, you can submit an appeal.",
                &format!("{}/appeal.html", site_url_of(email, state)),
                "Appeal Decision",
            );
            crate::routes::push::send_email_bg(state, email, &subject, &html);
            json_response(200, json!({ "ok": true }))
        }
        "blacklist" => {
            let reason = body
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("no reason given");
            if let Some(map) = tokens.as_object_mut() {
                map.remove(tok);
            }
            let _ = state.store.write_document(&tokens_file, &tokens);
            mitch_lib::bans::blacklist_insert(&state.store, email, reason, "admin");
            let site_name = site_name(state);
            let subject = format!("Your {site_name} Access Request Was Not Approved");
            let html = super::legacy::make_access_status_html(
                    state,

                email,
                "Access Request Denied",
                &format!("Your access request was not approved.<br><br><strong>Reason:</strong> {reason}"),
                "",
                "",
            );
            crate::routes::push::send_email_bg(state, email, &subject, &html);
            json_response(200, json!({ "ok": true }))
        }
        _ => json_response(400, json!({ "error": "unknown action" })),
    }
}

/// dtype `appeal` — approve removes revocations + re-issues a token.
async fn appeal(state: &Arc<AppState>, body: &Value) -> Response {
    let email = body.get("email").and_then(|v| v.as_str()).unwrap_or("");
    let submitted_at = body
        .get("submitted_at")
        .map(|v| v.to_string().replace('"', ""))
        .unwrap_or_default();
    let action = body.get("action").and_then(|v| v.as_str()).unwrap_or("");
    let mut appeals = state
        .store
        .read_document(&state.cfg.data_dir.join("appeals.json"), json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    if action == "approve" {
        let revoked_file = state.cfg.base_dir.join("data/revoked.json");
        let mut revoked = state.store.read_document(&revoked_file, json!({}));
        if let Some(map) = revoked.as_object_mut() {
            let remove: Vec<String> = map
                .iter()
                .filter(|(_, rec)| rec.get("email").and_then(|v| v.as_str()) == Some(email))
                .map(|(k, _)| k.clone())
                .collect();
            for k in remove {
                map.remove(&k);
            }
        }
        let _ = state.store.write_document(&revoked_file, &revoked);
        let tokens_file = state.cfg.base_dir.join("data/tokens.json");
        let mut tokens = state.store.read_document(&tokens_file, json!({}));
        let new_tok = mitch_lib::crypto::random_bytes_hex(24);
        if let Some(map) = tokens.as_object_mut() {
            map.insert(
                new_tok.clone(),
                json!({ "email": email, "created_at": now_millis() as f64 / 1000.0, "used": false }),
            );
        }
        let _ = state.store.write_document(&tokens_file, &tokens);
        let site_name = site_name(state);
        let link = format!("{}/claim.html?token={new_tok}", site_url_of(email, state));
        let subject = format!("Your {site_name} Appeal Has Been Approved");
        let html = super::legacy::make_access_status_html(
                state,

            email,
            "Appeal Approved",
            "Great news — your appeal has been approved and your access has been restored. Click the link below to claim your account. Save your token in case you need it later:",
            &link,
            "Claim Account",
        );
        crate::routes::push::send_email_bg(state, email, &subject, &html);
    }
    appeals.retain(|app| {
        !(app.get("email").and_then(|v| v.as_str()) == Some(email)
            && app
                .get("submitted_at")
                .map(|v| v.to_string().replace('"', ""))
                .unwrap_or_default()
                == submitted_at)
    });
    let _ = state
        .store
        .write_document(&state.cfg.data_dir.join("appeals.json"), &json!(appeals));
    json_response(200, json!({ "ok": true }))
}

/// dtype `unsub` — resolve an unsubscribe request.
async fn unsub(state: &Arc<AppState>, body: &Value) -> Response {
    let raw_email = body.get("email").and_then(|v| v.as_str()).unwrap_or("");
    let submitted_at = body
        .get("submitted_at")
        .map(|v| v.to_string().replace('"', ""))
        .unwrap_or_default();
    let action = body.get("action").and_then(|v| v.as_str()).unwrap_or("");
    let email = raw_email.trim().to_lowercase();
    let reqs_file = state.cfg.data_dir.join("unsub_requests.json");
    let reqs = state
        .store
        .read_document(&reqs_file, json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let kept: Vec<Value> = reqs
        .into_iter()
        .filter(|r| {
            !(r.get("email")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase()
                == email
                && r.get("submitted_at")
                    .map(|v| v.to_string().replace('"', ""))
                    .unwrap_or_default()
                    == submitted_at)
        })
        .collect();
    let _ = state.store.write_document(&reqs_file, &json!(kept));
    let site_name = site_name(state);
    if action == "approve" {
        let unsub_file = state.cfg.data_dir.join("newsletter_unsub.json");
        let unsub = state
            .store
            .read_document(&unsub_file, json!([]))
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut set: std::collections::BTreeSet<String> = unsub
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        set.insert(email.clone());
        let _ = state
            .store
            .write_document(&unsub_file, &json!(set.into_iter().collect::<Vec<_>>()));
        let subject = format!("You've Been Unsubscribed from the {site_name} Newsletter");
        let html = super::legacy::make_access_status_html(
                state,

            &email,
            "Unsubscribed Successfully",
            "You have been successfully unsubscribed from the newsletter. You won't receive any further emails.",
            &format!("{}/newsletter.html", site_url_of(&email, state)),
            "Resubscribe",
        );
        crate::routes::push::send_email_bg(state, &email, &subject, &html);
    } else {
        let subject = format!("Your {site_name} Unsubscribe Request");
        let html = super::legacy::make_access_status_html(
                state,

            &email,
            "Unsubscribe Request Not Processed",
            "Your request to unsubscribe from the newsletter was not processed. If you believe this is an error, please reply to this email.",
            "",
            "",
        );
        crate::routes::push::send_email_bg(state, &email, &subject, &html);
    }
    json_response(200, json!({ "ok": true }))
}

/// dtype `newsletter_list`.
fn newsletter_list(state: &Arc<AppState>) -> Response {
    let extra = state
        .store
        .read_document(&state.cfg.data_dir.join("newsletter_extra.json"), json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let unsub_set: std::collections::BTreeSet<String> = state
        .store
        .read_document(&state.cfg.data_dir.join("newsletter_unsub.json"), json!([]))
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_lowercase()))
                .collect()
        })
        .unwrap_or_default();
    let tokens = state
        .store
        .read_document(&state.cfg.base_dir.join("data/tokens.json"), json!({}));
    let mut emails: Vec<Value> = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for e in &extra {
        let s = e.as_str().unwrap_or("");
        if s.contains('*') {
            continue;
        }
        let lower = s.to_lowercase();
        if !unsub_set.contains(&lower) && !seen.contains(&lower) {
            seen.insert(lower);
            emails.push(json!({ "email": s, "source": "manual" }));
        }
    }
    if let Some(map) = tokens.as_object() {
        for (_tok, d) in map {
            let email = d.get("email").and_then(|v| v.as_str()).unwrap_or("").trim();
            if email.is_empty() {
                continue;
            }
            let lower = email.to_lowercase();
            if !unsub_set.contains(&lower) && !seen.contains(&lower) {
                seen.insert(lower);
                emails.push(json!({ "email": email, "source": "enrolled" }));
            }
        }
    }
    emails.sort_by(|a, b| {
        a.get("email")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase()
            .cmp(
                &b.get("email")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_lowercase(),
            )
    });
    let count = emails.len();
    json_response(
        200,
        json!({ "emails": emails, "count": count, "unsub": unsub_set.len() }),
    )
}

/// dtype `newsletter_send` — background fan-out via the mail service.
async fn newsletter_send(state: &Arc<AppState>, body: &Value) -> Response {
    let subject = body.get("subject").and_then(|v| v.as_str()).unwrap_or("");
    let msg_body = body.get("body").and_then(|v| v.as_str()).unwrap_or("");
    if subject.is_empty() || msg_body.is_empty() {
        return json_response(400, json!({ "error": "subject and body required" }));
    }
    let extra = state
        .store
        .read_document(&state.cfg.data_dir.join("newsletter_extra.json"), json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let unsub_set: std::collections::BTreeSet<String> = state
        .store
        .read_document(&state.cfg.data_dir.join("newsletter_unsub.json"), json!([]))
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_lowercase()))
                .collect()
        })
        .unwrap_or_default();
    let mut emails: Vec<String> = extra
        .iter()
        .filter_map(|v| v.as_str())
        .filter(|e| !unsub_set.contains(&e.to_lowercase()) && !e.contains('*'))
        .map(str::to_string)
        .collect();
    let mut seen: std::collections::BTreeSet<String> =
        emails.iter().map(|e| e.to_lowercase()).collect();
    let tokens = state
        .store
        .read_document(&state.cfg.base_dir.join("data/tokens.json"), json!({}));
    if let Some(map) = tokens.as_object() {
        for (_tok, d) in map {
            let email = d
                .get("email")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if !email.is_empty()
                && !unsub_set.contains(&email.to_lowercase())
                && !seen.contains(&email.to_lowercase())
            {
                seen.insert(email.to_lowercase());
                emails.push(email);
            }
        }
    }
    for email in &emails {
        crate::routes::push::send_email_bg(state, email, subject, msg_body);
    }
    json_response(
        200,
        json!({ "message": format!("Sending to {} recipients in background.", emails.len()) }),
    )
}

/// dtype `newsletter_add` — resolve the ref to a real email, refuse masks.
fn newsletter_add(state: &Arc<AppState>, body: &Value) -> Response {
    let raw = body
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let email = {
        let resolved = super::legacy::resolve_member_ref(state, &raw);
        if resolved.is_empty() {
            raw.clone()
        } else {
            resolved
        }
    }
    .trim()
    .to_lowercase();
    if !email.is_empty() && email.contains('@') && !email.contains('*') {
        let extra_file = state.cfg.data_dir.join("newsletter_extra.json");
        let extra = state
            .store
            .read_document(&extra_file, json!([]))
            .as_array()
            .cloned()
            .unwrap_or_default();
        // saveJsonSync(path, [...new Set(extra)].sort()) — exact-string dedupe.
        let mut combined: Vec<String> = extra
            .iter()
            .filter_map(|e| e.as_str().map(str::to_string))
            .collect();
        combined.push(email.clone());
        combined.sort();
        combined.dedup();
        let set: Vec<Value> = combined.into_iter().map(|e| json!(e)).collect();
        let _ = state.store.write_document(&extra_file, &json!(set));
        return json_response(200, json!({ "ok": true, "added": email }));
    }
    json_response(
        400,
        json!({ "ok": false, "error": "Could not resolve that member to a real email address." }),
    )
}

/// dtype `newsletter_remove`.
fn newsletter_remove(state: &Arc<AppState>, body: &Value) -> Response {
    let email = body
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let extra_file = state.cfg.data_dir.join("newsletter_extra.json");
    let extra = state
        .store
        .read_document(&extra_file, json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let filtered: Vec<Value> = extra
        .into_iter()
        .filter(|e| e.as_str().map(|s| s.to_lowercase()) != Some(email.clone()))
        .collect();
    let _ = state.store.write_document(&extra_file, &json!(filtered));
    json_response(200, json!({ "ok": true }))
}

/// dtype `rl_list` — per-endpoint rate-limit stats.
fn rl_list(state: &Arc<AppState>) -> Response {
    let rows = state.rate_limiter.rl_endpoints();
    let endpoints: Vec<Value> = rows
        .into_iter()
        .map(|(ep, keys, max_hits)| json!({ "endpoint": ep, "keys": keys, "max_hits": max_hits }))
        .collect();
    json_response(200, json!({ "endpoints": endpoints }))
}

/// dtype `rl_reset`.
fn rl_reset(state: &Arc<AppState>, body: &Value) -> Response {
    let endpoint = body.get("endpoint").and_then(|v| v.as_str()).unwrap_or("");
    state.rate_limiter.rl_reset_endpoint(endpoint);
    json_response(200, json!({ "ok": true }))
}

/// dtype `rl_reset_all`.
fn rl_reset_all(state: &Arc<AppState>) -> Response {
    state.rate_limiter.rl_clear();
    json_response(200, json!({ "ok": true }))
}

/// dtype `gen_token` — infinite admin token.
fn gen_token(state: &Arc<AppState>) -> Response {
    let tokens_file = state.cfg.base_dir.join("data/tokens.json");
    let mut tokens = state.store.read_document(&tokens_file, json!({}));
    let tok = mitch_lib::crypto::random_bytes_hex(24);
    if let Some(map) = tokens.as_object_mut() {
        map.insert(
            tok.clone(),
            json!({
                "email": "admin@mitch.pro",
                "norm_email": "admin@mitch.pro",
                "gen": 0,
                "created_at": now_millis() as f64 / 1000.0,
                "used": false,
                "infinite": true,
                "claim_count": 0,
            }),
        );
    }
    let _ = state.store.write_document(&tokens_file, &tokens);
    json_response(
        200,
        json!({ "url": format!("https://mitch.88chan.me/claim.html?token={tok}"), "token": tok }),
    )
}

fn site_name(state: &Arc<AppState>) -> String {
    let site = state
        .store
        .read_document(&state.cfg.data_dir.join("site.json"), json!({}));
    site.get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("mitch.pro")
        .to_string()
}

/// `siteUrl(email)` — the school-site origin for student addresses, the
/// primary origin otherwise, using data/site.json.
fn site_url_of(email: &str, state: &Arc<AppState>) -> String {
    let norm = mitch_lib::auth::normalize_email(email);
    let site = state
        .store
        .read_document(&state.cfg.data_dir.join("site.json"), json!({}));
    let primary = site
        .get("primary")
        .and_then(|v| v.as_str())
        .unwrap_or("https://mitch.pro");
    let alternate = site.get("alternate").and_then(|v| v.as_str()).unwrap_or("");
    if norm.ends_with("student.rjuhsd.us") && !alternate.is_empty() {
        alternate
    } else {
        primary
    }
    .to_string()
}
