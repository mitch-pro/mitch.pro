//! Legacy passphrase-key tools + passphrase management — /api/admin/js
//! (11436), ssh-key GET/generate/save (10242-10373), passphrase-status
//! (11246/15579), change/reset-other passphrase (11274/11296),
//! maintenance-toggle (11151), shop catalog save (11171), team-token
//! (20418), canvas-unban (19153), canvas-report-status (18801),
//! reset-ratelimit (15889), plus the access-status email template.

#![allow(clippy::expect_used)] // infallible static regexes
use super::{forbidden, unauthorized, AdminCtx, Resp};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    search: &str,
    body: &Value,
    ctx: &AdminCtx,
) -> Resp {
    // POST /api/admin/js — serve admin/admin.js behind the admin key.
    if path == "/api/admin/js" {
        if state
            .rate_limiter
            .rate_limited(&format!("ip:{}", ctx.ip), "/api/admin/js")
        {
            return Some(json_response(429, json!({ "error": "too many attempts" })));
        }
        if !check_admin_pw(state, body) {
            return Some(forbidden());
        }
        let js_path = state.cfg.base_dir.join("admin/admin.js");
        return Some(match std::fs::read_to_string(&js_path) {
            Ok(js) => Response::builder()
                .status(200)
                .header("content-type", "application/javascript; charset=utf-8")
                .header("cache-control", "no-store")
                .body(axum::body::Body::from(js))
                .unwrap_or_else(|_| internal_error()),
            Err(_) => Response::builder()
                .status(200)
                .header("content-type", "application/javascript")
                .body(axum::body::Body::from(
                    "console.error(\"admin.js not found\")",
                ))
                .unwrap_or_else(|_| internal_error()),
        });
    }

    // GET /api/admin/passphrase-status (outside the global gate; admins only).
    if path == "/api/admin/passphrase-status" && *method == Method::GET {
        if !valid_id(&ctx.sid, state) {
            return Some(unauthorized());
        }
        if !ctx.is_admin(state) {
            return Some(forbidden());
        }
        let email = ctx.email(state);
        let norm = mitch_lib::auth::normalize_email(&email);
        let data = mitch_lib::admin::load_admin_passphrase(&state.store, &state.cfg.data_dir);
        let entry = data.get(norm.as_str()).cloned().unwrap_or(json!({}));
        return Some(json_response(
            200,
            json!({
                "set": !entry.get("hash").and_then(|v| v.as_str()).unwrap_or("").is_empty(),
                "updatedAt": entry.get("updatedAt").and_then(|v| v.as_i64()).unwrap_or(0),
            }),
        ));
    }

    // POST /api/admin/passphrase-status — set or verify.
    if path == "/api/admin/passphrase-status" && *method == Method::POST {
        if !valid_id(&ctx.sid, state) {
            return Some(unauthorized());
        }
        if !ctx.is_admin(state) {
            return Some(forbidden());
        }
        let header_pass = headers
            .get("X-Admin-Passphrase")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let body_pass = body
            .get("passphrase")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let passphrase = if header_pass.is_empty() {
            body_pass
        } else {
            header_pass
        }
        .trim();
        if passphrase.len() < 4 {
            return Some(json_response(
                400,
                json!({ "error": "passphrase_too_short" }),
            ));
        }
        let email = ctx.email(state);
        let norm = mitch_lib::auth::normalize_email(&email);
        let data = mitch_lib::admin::load_admin_passphrase(&state.store, &state.cfg.data_dir);
        let entry = data.get(norm.as_str()).cloned().unwrap_or(json!({}));
        if entry
            .get("hash")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .is_empty()
        {
            mitch_lib::admin::save_admin_passphrase_for_user(
                &state.store,
                &state.cfg.data_dir,
                &norm,
                json!({
                    "hash": mitch_lib::crypto::argon2_hash(passphrase),
                    "createdAt": now_millis(),
                    "updatedAt": now_millis(),
                    "setBy": email,
                }),
            );
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                &email,
                "set_admin_passphrase",
                json!({}),
            );
            return Some(json_response(200, json!({ "ok": true, "set": true })));
        }
        if !mitch_lib::admin::verify_admin_passphrase_raw(
            &state.store,
            &state.id_secret,
            &state.cfg.data_dir,
            &ctx.sid,
            passphrase,
        ) {
            return Some(json_response(403, json!({ "error": "invalid_passphrase" })));
        }
        return Some(json_response(200, json!({ "ok": true, "set": true })));
    }

    // /api/admin/change-passphrase.
    if path == "/api/admin/change-passphrase" {
        if !valid_id(&ctx.sid, state) {
            return Some(unauthorized());
        }
        if !ctx.is_admin(state) {
            return Some(forbidden());
        }
        let current = headers
            .get("X-Admin-Passphrase")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .trim();
        if !mitch_lib::admin::verify_admin_passphrase_raw(
            &state.store,
            &state.id_secret,
            &state.cfg.data_dir,
            &ctx.sid,
            current,
        ) {
            return Some(json_response(403, json!({ "error": "invalid_passphrase" })));
        }
        let new_passphrase = body
            .get("newPassphrase")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if new_passphrase.len() < 4 {
            return Some(json_response(
                400,
                json!({ "error": "passphrase_too_short" }),
            ));
        }
        let email = ctx.email(state);
        let norm = mitch_lib::auth::normalize_email(&email);
        mitch_lib::admin::save_admin_passphrase_for_user(
            &state.store,
            &state.cfg.data_dir,
            &norm,
            json!({
                "hash": mitch_lib::crypto::argon2_hash(new_passphrase),
                "updatedAt": now_millis(),
                "setBy": email,
            }),
        );
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &email,
            "change_admin_passphrase",
            json!({}),
        );
        return Some(json_response(200, json!({ "ok": true, "set": true })));
    }

    // POST /api/admin/reset-other-passphrase.
    if path == "/api/admin/reset-other-passphrase" && *method == Method::POST {
        if !valid_id(&ctx.sid, state) {
            return Some(unauthorized());
        }
        if !ctx.is_admin(state) {
            return Some(forbidden());
        }
        let target_email = body
            .get("targetEmail")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let target_email = super::resolve_admin_target(state, target_email);
        let target_email = target_email.as_str();
        let new_passphrase = body
            .get("newPassphrase")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if target_email.is_empty() {
            return Some(json_response(
                400,
                json!({ "error": "target_email_required" }),
            ));
        }
        if new_passphrase.len() < 4 {
            return Some(json_response(
                400,
                json!({ "error": "passphrase_too_short" }),
            ));
        }
        let caller_email = ctx.email(state);
        if !mitch_lib::auth::is_admin_email(&state.store, target_email) {
            return Some(json_response(
                400,
                json!({ "error": "target_user_is_not_an_admin" }),
            ));
        }
        if mitch_lib::auth::is_protected_owner_email(&state.store, target_email) {
            return Some(json_response(
                403,
                json!({ "error": "Owner and co-owner passphrases can only be changed by their own account." }),
            ));
        }
        let target_norm = mitch_lib::auth::normalize_email(target_email);
        mitch_lib::admin::save_admin_passphrase_for_user(
            &state.store,
            &state.cfg.data_dir,
            &target_norm,
            json!({
                "hash": mitch_lib::crypto::argon2_hash(new_passphrase),
                "updatedAt": now_millis(),
                "setBy": caller_email,
            }),
        );
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &caller_email,
            "reset_other_admin_passphrase",
            json!({ "target": target_email }),
        );
        return Some(json_response(
            200,
            json!({ "ok": true, "message": "Passphrase reset successfully." }),
        ));
    }

    // POST /api/admin/maintenance-toggle (admins only).
    if path == "/api/admin/maintenance-toggle" {
        if state.rate_limit_check(&ctx.ip, "anon", path).is_some() {
            return Some(json_response(
                429,
                json!({ "error": "Too many requests, slow down" }),
            ));
        }
        if !valid_id(&ctx.sid, state) {
            return Some(json_response(
                401,
                json!({ "success": false, "error": "auth required" }),
            ));
        }
        if !ctx.is_admin(state) {
            return Some(json_response(
                403,
                json!({ "success": false, "error": "forbidden" }),
            ));
        }
        let active = body.get("active").and_then(|v| v.as_bool()) == Some(true)
            || body.get("active").and_then(|v| v.as_str()) == Some("true");
        let _ = state.store.write_document(
            &state.cfg.data_dir.join("soft_maintenance.json"),
            &json!({ "active": active }),
        );
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &ctx.email(state),
            "maintenance_toggle",
            json!({ "active": active }),
        );
        return Some(json_response(
            200,
            json!({ "success": true, "active": active }),
        ));
    }

    // POST /api/admin/shop/catalog/save (admins only).
    if path == "/api/admin/shop/catalog/save" {
        if state.rate_limit_check(&ctx.ip, "anon", path).is_some() {
            return Some(json_response(
                429,
                json!({ "error": "Too many requests, slow down" }),
            ));
        }
        if !valid_id(&ctx.sid, state) {
            return Some(json_response(
                401,
                json!({ "success": false, "error": "auth required" }),
            ));
        }
        if !ctx.is_admin(state) {
            return Some(json_response(
                403,
                json!({ "success": false, "error": "forbidden" }),
            ));
        }
        let catalog = body.get("catalog").cloned().unwrap_or(Value::Null);
        let Some(catalog) = catalog.as_array() else {
            return Some(json_response(
                400,
                json!({ "success": false, "error": "catalog must be an array" }),
            ));
        };
        let admin_email = ctx.email(state);
        let catalog_path = state.cfg.data_dir.join("shop_catalog.json");
        if catalog.is_empty() {
            // Reset to default: drop the override file.
            let _ = std::fs::remove_file(&catalog_path);
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                &admin_email,
                "shop_catalog_reset",
                json!({}),
            );
        } else {
            let _ = state.store.write_document(&catalog_path, &json!(catalog));
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                &admin_email,
                "shop_catalog_save",
                json!({ "itemsCount": catalog.len() }),
            );
        }
        return Some(json_response(200, json!({ "success": true })));
    }

    // GET /api/admin/ssh-key.
    if path == "/api/admin/ssh-key" && *method == Method::GET {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        let Some(email) = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &ctx.sid)
        else {
            return Some(unauthorized());
        };
        let saved = state
            .store
            .read_document(&state.cfg.data_dir.join("admin_ssh_keys.json"), json!({}));
        let user_key = saved.get(mitch_lib::auth::normalize_email(&email));
        return Some(match user_key {
            Some(k) => json_response(
                200,
                json!({
                    "hasKey": true,
                    "publicKey": k.get("publicKey").cloned().unwrap_or(Value::Null),
                    "createdAt": k.get("createdAt").and_then(|v| v.as_i64()).unwrap_or(0),
                }),
            ),
            None => json_response(200, json!({ "hasKey": false })),
        });
    }

    // POST /api/admin/ssh-key/generate.
    if path == "/api/admin/ssh-key/generate" && *method == Method::POST {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        let Some(email) = email_from_sid(state, ctx) else {
            return Some(unauthorized());
        };
        let passphrase = body
            .get("passphrase")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if passphrase.is_empty() {
            return Some(json_response(
                400,
                json!({ "error": "Passphrase is required to encrypt the private key." }),
            ));
        }
        let temp_base = state.cfg.data_dir.join(format!(
            "temp_gen_{}",
            mitch_lib::crypto::random_bytes_hex(8)
        ));
        let result = (|| -> Result<(String, String), String> {
            let out = std::process::Command::new("ssh-keygen")
                .args([
                    "-t",
                    "ed25519",
                    "-N",
                    passphrase,
                    "-f",
                    &temp_base.to_string_lossy(),
                    "-C",
                    &email,
                ])
                .output()
                .map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(format!(
                    "Failed to generate SSH key pair: {}",
                    String::from_utf8_lossy(&out.stderr)
                ));
            }
            Ok((
                std::fs::read_to_string(&temp_base).map_err(|e| e.to_string())?,
                std::fs::read_to_string(format!("{}.pub", temp_base.to_string_lossy()))
                    .map_err(|e| e.to_string())?,
            ))
        })();
        let _ = std::fs::remove_file(&temp_base);
        let _ = std::fs::remove_file(format!("{}.pub", temp_base.to_string_lossy()));
        return Some(match result {
            Ok((priv_key, pub_key)) => {
                save_ssh_key(state, &email, &priv_key, &pub_key);
                mitch_lib::admin::log_admin_action(
                    &state.store,
                    &state.cfg.data_dir,
                    &email,
                    "generate_ssh_key",
                    json!({ "email": email }),
                );
                json_response(200, json!({ "ok": true, "publicKey": pub_key }))
            }
            Err(e) => json_response(500, json!({ "error": e })),
        });
    }

    // POST /api/admin/ssh-key/save — verify (and re-encrypt) a private key.
    if path == "/api/admin/ssh-key/save" && *method == Method::POST {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        let Some(email) = email_from_sid(state, ctx) else {
            return Some(unauthorized());
        };
        let private_key = body
            .get("privateKey")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let passphrase = body
            .get("passphrase")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if private_key.is_empty() {
            return Some(json_response(
                400,
                json!({ "error": "Private key is required." }),
            ));
        }
        if passphrase.is_empty() {
            return Some(json_response(
                400,
                json!({ "error": "Passphrase is required to secure the key." }),
            ));
        }
        let temp_base = state.cfg.data_dir.join(format!(
            "temp_save_{}",
            mitch_lib::crypto::random_bytes_hex(8)
        ));
        let result = (|| -> Result<(String, String), (u16, String)> {
            std::fs::write(&temp_base, &private_key).map_err(|e| (500u16, e.to_string()))?;
            // Re-encrypt unencrypted keys with ssh-keygen -p (the JS uses node
            // crypto PKCS8 aes-256-cbc; both formats are accepted by russh).
            let check = std::process::Command::new("ssh-keygen")
                .args(["-y", "-P", "", "-f", &temp_base.to_string_lossy()])
                .output();
            let _final_key = if check.map(|o| o.status.success()).unwrap_or(false) {
                let re = std::process::Command::new("ssh-keygen")
                    .args([
                        "-p",
                        "-P",
                        "",
                        "-N",
                        passphrase,
                        "-f",
                        &temp_base.to_string_lossy(),
                    ])
                    .output()
                    .map_err(|e| (400u16, e.to_string()))?;
                if !re.status.success() {
                    return Err((
                        400,
                        format!(
                            "Failed to process and encrypt unencrypted private key: {}",
                            String::from_utf8_lossy(&re.stderr)
                        ),
                    ));
                }
                std::fs::read_to_string(&temp_base).unwrap_or_else(|_| private_key.clone())
            } else {
                private_key.clone()
            };
            let verify = std::process::Command::new("ssh-keygen")
                .args(["-y", "-P", passphrase, "-f", &temp_base.to_string_lossy()])
                .output()
                .map_err(|e| (400u16, e.to_string()))?;
            if !verify.status.success() {
                return Err((
                    400,
                    "Invalid private key or incorrect passphrase.".to_string(),
                ));
            }
            let pub_key = String::from_utf8_lossy(&verify.stdout).trim().to_string();
            let final_key =
                std::fs::read_to_string(&temp_base).unwrap_or_else(|_| private_key.clone());
            Ok((final_key, pub_key))
        })();
        let _ = std::fs::remove_file(&temp_base);
        return Some(match result {
            Ok((final_key, pub_key)) => {
                save_ssh_key(state, &email, &final_key, &pub_key);
                mitch_lib::admin::log_admin_action(
                    &state.store,
                    &state.cfg.data_dir,
                    &email,
                    "save_ssh_key",
                    json!({ "email": email }),
                );
                json_response(200, json!({ "ok": true, "publicKey": pub_key }))
            }
            Err((status, error)) => json_response(status, json!({ "error": error })),
        });
    }

    // POST /api/admin/ssh-key/copy-id — needs an SSH client; wired with
    // russh in Step 13 alongside the Proxmox work.
    if path == "/api/admin/ssh-key/copy-id" && *method == Method::POST {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        if email_from_sid(state, ctx).is_none() {
            return Some(unauthorized());
        }
        return Some(json_response(
            502,
            json!({ "error": "ssh-copy-id is not yet available in the Rust build (Step 13 port)." }),
        ));
    }

    // POST /api/admin/team-token.
    if path == "/api/admin/team-token" {
        if !check_admin_pw(state, body) {
            return Some(forbidden());
        }
        let name = body
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if name.is_empty() {
            return Some(json_response(400, json!({ "error": "name required" })));
        }
        let token = mitch_lib::crypto::random_bytes_hex(24);
        let tokens_file = state.cfg.data_dir.join("team_tokens.json");
        let mut tokens = state.store.read_document(&tokens_file, json!({}));
        if let Some(map) = tokens.as_object_mut() {
            map.insert(
                token.clone(),
                json!({ "name": name, "created": now_millis() }),
            );
        }
        let _ = state.store.write_document(&tokens_file, &tokens);
        return Some(json_response(200, json!({ "token": token })));
    }

    // POST /api/admin/canvas-unban.
    if path == "/api/admin/canvas-unban" {
        if !check_admin_pw(state, body) {
            return Some(forbidden());
        }
        let painter = body.get("painter").and_then(|v| v.as_str()).unwrap_or("");
        if painter.is_empty() {
            return Some(json_response(400, json!({ "error": "painter required" })));
        }
        let banned_file = state.cfg.data_dir.join("canvas_banned.json");
        let mut banned = state.store.read_document(&banned_file, json!({}));
        if !banned.get(painter).is_some() {
            return Some(json_response(404, json!({ "error": "painter not banned" })));
        }
        if let Some(map) = banned.as_object_mut() {
            map.remove(painter);
        }
        let _ = state.store.write_document(&banned_file, &banned);
        return Some(json_response(200, json!({ "ok": true })));
    }

    // POST /api/admin/canvas-report-status.
    if path == "/api/admin/canvas-report-status" && *method == Method::POST {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        let id = body.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let status = body
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(40)
            .collect::<String>();
        if id.is_empty()
            || !["Needs review", "Reviewing", "Resolved", "Dismissed"].contains(&status.as_str())
        {
            return Some(json_response(400, json!({ "error": "invalid status" })));
        }
        let file = state.cfg.data_dir.join("canvas_reports.json");
        let mut reports = state
            .store
            .read_document(&file, json!([]))
            .as_array()
            .cloned()
            .unwrap_or_default();
        let Some(pos) = reports
            .iter()
            .position(|r| r.get("id").and_then(|v| v.as_str()) == Some(id))
        else {
            return Some(json_response(404, json!({ "error": "report found" })));
        };
        reports[pos]["status"] = json!(status);
        reports[pos]["reviewedAt"] = json!(now_millis());
        reports[pos]["reviewedBy"] = json!(ctx.email(state));
        let _ = state.store.write_document(&file, &json!(reports));
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &ctx.email(state),
            "canvas_report_status",
            json!({ "id": id, "status": status }),
        );
        return Some(json_response(200, json!({ "ok": true })));
    }

    // GET /api/admin/reset-ratelimit.
    if path == "/api/admin/reset-ratelimit" {
        let params = parse_query(search);
        let key = params.get("key").cloned().unwrap_or_default();
        let admin_key = match std::fs::read_to_string(state.cfg.base_dir.join("admin/admin.key")) {
            Ok(k) => k.trim().to_string(),
            Err(_) => {
                return Some(json_response(
                    500,
                    json!({ "error": "no admin key configured" }),
                ))
            }
        };
        if key != admin_key {
            return Some(forbidden());
        }
        let endpoints: Vec<String> = params
            .get("endpoints")
            .cloned()
            .unwrap_or_else(|| "/api/request-access,/api/newsletter-signup".to_string())
            .split(',')
            .map(str::to_string)
            .collect();
        let cleared = state.rate_limiter.rl_reset_many(&endpoints);
        return Some(json_response(
            200,
            json!({ "cleared": cleared, "endpoints": endpoints }),
        ));
    }

    None
}

fn internal_error() -> Response {
    crate::errors::json_resp(500, json!({ "error": "internal_error" }))
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

fn valid_id(sid: &str, state: &Arc<AppState>) -> bool {
    !sid.is_empty() && mitch_lib::auth::valid_id(sid, &state.id_secret)
}

fn email_from_sid(state: &Arc<AppState>, ctx: &AdminCtx) -> Option<String> {
    mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &ctx.sid)
}

fn save_ssh_key(state: &Arc<AppState>, email: &str, private_key: &str, public_key: &str) {
    let file = state.cfg.data_dir.join("admin_ssh_keys.json");
    let mut saved = state.store.read_document(&file, json!({}));
    if let Some(map) = saved.as_object_mut() {
        map.insert(
            mitch_lib::auth::normalize_email(email),
            json!({
                "privateKey": private_key,
                "publicKey": public_key,
                "createdAt": now_millis(),
            }),
        );
    }
    let _ = state.store.write_document(&file, &saved);
}

fn parse_query(search: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let query = search.trim_start_matches('?');
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        out.insert(percent_decode(k), percent_decode(v));
    }
    out
}

/// `makeAccessStatusHtml(email, title, messageText, actionUrl, actionLabel)`.
pub fn make_access_status_html(
    state: &std::sync::Arc<crate::state::AppState>,
    email: &str,
    title: &str,
    message_text: &str,
    action_url: &str,
    action_label: &str,
) -> String {
    let action_html = if action_url.is_empty() {
        String::new()
    } else {
        format!(
            "\n    <div style=\"text-align: center; margin-bottom: 8px;\">\n      <a href=\"{action_url}\" style=\"display: inline-block; background: linear-gradient(135deg, #a855f7, #6366f1); color: #ffffff; text-decoration: none; padding: 12px 24px; border-radius: 10px; font-weight: 700;\">{action_label}</a>\n    </div>\n    "
        )
    };
    let content = format!(
        "\n    <h2 style=\"margin: 0 0 16px; font-size: 20px; font-weight: 700; color: #f4f4f5; text-align: center;\">{title}</h2>\n    <div style=\"background-color: rgba(255, 255, 255, 0.03); border: 1px solid rgba(255,255,255,0.08); border-radius: 12px; padding: 20px; text-align: center; margin-bottom: 24px;\">\n      <p style=\"margin: 0; color: #cbd5e1; line-height: 1.6;\">{message_text}</p>\n    </div>\n    {action_html}\n  "
    );
    html_base_template(state, email, title, &content)
}

/// `unsubscribeEmailKey(email)` (server.js:2029-2031) — String(email || '')
/// lowercased then trimmed.
fn unsubscribe_email_key(email: &str) -> String {
    email.to_lowercase().trim().to_string()
}

/// `unsubscribeUrl(email)` (server.js:2033-2046) — a stable random 24-byte
/// hex token per delivery address, persisted in data/unsubscribe_tokens.json
/// (a DB-routed doc). Missing/falsy token → generate + persist
/// (saveJsonSync → write_document); a non-object doc resets to {}.
pub fn unsubscribe_url(state: &crate::state::AppState, email: &str) -> String {
    let key = unsubscribe_email_key(email);
    if key.is_empty() {
        return String::new();
    }
    let path = state.cfg.data_dir.join("unsubscribe_tokens.json");
    let stored = state.store.read_document(&path, serde_json::json!({}));
    // JS: `if (!tokens || typeof tokens !== 'object' || Array.isArray(tokens)) tokens = {}`.
    let map = match stored.as_object() {
        Some(o) => o.clone(),
        None => serde_json::Map::new(),
    };
    // JS: `let token = tokens[key]; if (!token) { … }` — missing, null and ''
    // all regenerate.
    if let Some(Value::String(t)) = map.get(&key) {
        if !t.is_empty() {
            return format!("{}/unsubscribe/{t}", state.cfg.alternate);
        }
    }
    let token = mitch_lib::crypto::random_bytes_hex(24);
    let mut out = map;
    out.insert(key, Value::String(token.clone()));
    let _ = state.store.write_document(&path, &Value::Object(out));
    format!("{}/unsubscribe/{token}", state.cfg.alternate)
}

/// JS `s.replace(/\/$/, '')` — removes at most one trailing slash.
fn strip_one_trailing_slash(s: &str) -> &str {
    s.strip_suffix('/').unwrap_or(s)
}

/// `htmlBaseTemplate(email, subject, contentHtml)` — the shared dark email
/// shell with the standard footer, including the unsubscribe link for any
/// real address (server.js:1689-1759). (The JS resolves PRIMARY/ALT from
/// data/site.json; those values come from the SiteConfig here.)
pub fn html_base_template(
    state: &crate::state::AppState,
    email: &str,
    subject: &str,
    content_html: &str,
) -> String {
    // JS: `s.primary.replace(/\/$/, '')` — one trailing slash only.
    let alt = strip_one_trailing_slash(&state.cfg.alternate);
    let primary = strip_one_trailing_slash(&state.cfg.primary);
    let unsub = if email.contains('@') {
        unsubscribe_url(state, email)
    } else {
        String::new()
    };
    // The exact template-literal whitespace: the interpolated block carries a
    // trailing "\n        " and the surrounding line contributes its own.
    let unsub_block = if unsub.is_empty() {
        String::new()
    } else {
        format!(
            "\n        <p style=\"margin: 0 0 8px;\">\n          To opt-out of these communications, you can <a href=\"{unsub}\" style=\"color: #38bdf8; text-decoration: underline;\">unsubscribe from this list</a>.\n        </p>\n        "
        )
    };
    let footer = format!(
        "\n      <div style=\"margin-top: 32px; padding-top: 16px; border-top: 1px solid rgba(255,255,255,0.08); font-size: 12px; color: #64748b; line-height: 1.5; text-align: center;\">\n        <p style=\"margin: 0 0 8px;\">\n          Delivered by <a href=\"{alt}\" style=\"color: #64748b; text-decoration: underline; font-weight: 600;\">mitchdog.com</a> | <a href=\"{primary}\" style=\"color: #64748b; text-decoration: underline;\">mitch.pro</a>\n        </p>\n        {unsub_block}\n        <p style=\"margin: 0;\">\n          For support: email SUPPORT to <a href=\"mailto:support@mitch.pro\" style=\"color: #64748b; text-decoration: none;\">support@mitch.pro</a> or mitchell.fogler@student.rjuhsd.us\n        </p>\n        <p style=\"margin: 8px 0 0; font-size: 11px; color: #475569;\">\n          2014 Capitol Ave #100, Sacramento, CA 95811\n        </p>\n      </div>\n    "
    );
    let shell = r##"
<!DOCTYPE html>
<html lang="en" style="background:#06060c;">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <meta name="color-scheme" content="light dark">
  <meta name="supported-color-schemes" content="light dark">
  <style> :root { color-scheme: light dark; supported-color-schemes: light dark; } </style>
  <title>@@SUBJECT@@</title>
</head>
<body style="margin: 0; padding: 0; background-color: #06060c; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif; color: #f8fafc; -webkit-font-smoothing: antialiased;">
  <table border="0" cellpadding="0" cellspacing="0" width="100%" bgcolor="#06060c" style="background-color: #06060c; border-collapse: collapse;">
    <tr>
      <td align="center" bgcolor="#06060c" style="background-color: #06060c; padding: 40px 20px;">
        <table border="0" cellpadding="0" cellspacing="0" width="100%" style="max-width: 580px; background-color: #0f172a; border-radius: 16px; overflow: hidden; border: 1px solid #0f172a; box-shadow: 0 20px 40px rgba(0,0,0,0.5);">
          <tr>
            <td height="6" style="background: linear-gradient(to right, #a855f7, #38bdf8);"></td>
          </tr>
          <tr>
            <td style="padding: 32px 32px 16px;">
              <table border="0" cellpadding="0" cellspacing="0" width="100%">
                <tr>
                  <td style="vertical-align: middle;">
                    <img src="https://mitchdog.com/favicon.ico" width="24" height="24" style="vertical-align: middle; margin-right: 10px; border-radius: 4px;" alt="mitch.pro">
                    <span style="font-size: 24px; font-weight: 800; color: #ffffff; vertical-align: middle; letter-spacing: -0.02em;">mitch.pro</span>
                    <span style="font-size: 24px; font-weight: 300; color: #64748b; vertical-align: middle; margin: 0 8px;">/</span>
                    <span style="font-size: 24px; font-weight: 800; color: #38bdf8; vertical-align: middle; letter-spacing: -0.02em;">mitchdog.com</span>
                  </td>
                </tr>
              </table>
            </td>
          </tr>
          <tr>
            <td style="padding: 0 32px 32px; font-size: 15px; color: #cbd5e1; line-height: 1.6;">
              @@CONTENT@@
              @@FOOTER@@
            </td>
          </tr>
        </table>
      </td>
    </tr>
  </table>
</body>
</html>
  "##;
    shell
        .replace("@@SUBJECT@@", subject)
        .replace("@@CONTENT@@", content_html)
        .replace("@@FOOTER@@", &footer)
        .trim()
        .to_string()
}

/// `resolveMemberRef(raw)` — profiles-based member lookup (email or username).
pub fn resolve_member_ref(state: &Arc<AppState>, raw: &str) -> String {
    let q = raw.to_lowercase().trim().to_string();
    if q.is_empty() {
        return String::new();
    }
    let profiles = mitch_lib::profiles::profile_all(&state.store);
    if q.contains('@') {
        let norm = mitch_lib::auth::normalize_email(&q);
        if profiles.get(norm.as_str()).is_some() {
            return norm;
        }
        return String::new();
    }
    // Username path: exact normalized match against profiles' username field.
    if let Some(map) = profiles.as_object() {
        for (key, p) in map {
            let username = p
                .get("username")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase()
                .trim()
                .to_string();
            if username == q {
                // Return the profile's real email when present.
                return p
                    .get("email")
                    .and_then(|v| v.as_str())
                    .map(|e| e.to_lowercase().trim().to_string())
                    .unwrap_or_else(|| key.clone());
            }
        }
    }
    String::new()
}
/// Minimal `+`-safe percent decoding for query strings.
fn percent_decode(s: &str) -> String {
    let plus_fixed = s.replace('+', " ");
    let bytes = plus_fixed.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let _hex = &plus_fixed[i + 1..i + 3];
            if let Ok(byte) = u8::from_str_radix(&plus_fixed[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `makePremiumAlertHtml(email, title, messageText, actionUrl='', actionLabel='')`
/// (server.js:2085-2104) — the 🌟 Premium Status Alert card on the shared
/// base template. `messageText`/`actionUrl`/`actionLabel` interpolate raw
/// (the JS call sites pass trusted literals).
pub fn premium_alert_html(
    state: &crate::state::AppState,
    email: &str,
    title: &str,
    message_text: &str,
    action_url: &str,
    action_label: &str,
) -> String {
    let action_block = if action_url.is_empty() {
        String::new()
    } else {
        format!(
            "\n    <div style=\"text-align: center; margin-bottom: 8px;\">\n      <a href=\"{action_url}\" style=\"display: inline-block; background: linear-gradient(135deg, #a855f7, #6366f1); color: #ffffff; text-decoration: none; padding: 12px 24px; border-radius: 10px; font-weight: 700; box-shadow: 0 10px 20px rgba(168, 85, 247, 0.2);\">{action_label}</a>\n    </div>\n    "
        )
    };
    let content = format!(
        "\n    <h2 style=\"margin: 0 0 16px; font-size: 20px; font-weight: 700; color: #fbbf24; text-align: center;\">🌟 Premium Status Alert</h2>\n    <div style=\"background-color: rgba(251, 191, 36, 0.08); border: 1px solid rgba(251, 191, 36, 0.25); border-radius: 12px; padding: 20px; text-align: center; margin-bottom: 24px;\">\n      <p style=\"margin: 0 0 12px; font-size: 16px; font-weight: 700; color: #f4f4f5;\">{title}</p>\n      <p style=\"margin: 0; color: #cbd5e1; line-height: 1.6;\">{message_text}</p>\n    </div>\n    {action_block}"
    );
    html_base_template(state, email, title, &content)
}

/// `sendPremiumEmailOffer(targetEmail)` (server.js:4309-4324) — the free
/// @mitch.pro address offer sent whenever a premium grant lands.
pub fn send_premium_email_offer(
    state: &std::sync::Arc<crate::state::AppState>,
    target_email: &str,
) {
    let to_email = mitch_lib::profile::canonical_delivery_email(
        &state.store,
        &state.cfg.data_dir,
        &state.id_secret,
        target_email,
    );
    let base = crate::workers_email::site_url(state, &to_email);
    let subject = "Eligible for a Free @mitch.pro Email Address!";
    let message = "Congratulations on getting Premium! As a Premium member, your main benefit is eligibility for a free custom @student.mitch.pro email address! Claim yours now by submitting your application.";
    let html = premium_alert_html(
        state,
        &to_email,
        subject,
        message,
        &format!("{base}/premium-email"),
        "Claim Email Address",
    );
    // Same deliverability rule as the expiry warning: school addresses must
    // ride the Gmail script (sendEmailBg), not support@mitch.pro SMTP.
    crate::routes::push::send_email_bg(state, &to_email, subject, &html);
    tracing::info!("[premium] Sent premium email offer to {to_email}");
}
