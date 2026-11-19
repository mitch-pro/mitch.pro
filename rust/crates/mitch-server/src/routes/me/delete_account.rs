//! `POST /api/delete-account` — permanent, immediate account deletion.
//!
//! Order matters and is the whole point of this file:
//!
//! 1. **Verify.** Valid session, the account's current password (or its email
//!    address when it has no password), and the authenticator code when TOTP is
//!    on. Staff accounts are refused: the owner removes those.
//! 2. **Computers.** A VM is a real Proxmox guest, so it is destroyed first and
//!    a failure aborts the whole request before anything is deleted. Nothing
//!    would be left pointing at it.
//! 3. **Chat tombstone.** `begin_erasure` records the account's Matrix ids and
//!    a cutoff; from that moment the gateway hides everything the account
//!    wrote. It needs the email-to-Matrix mapping, so it runs before step 4.
//! 4. **Data.** `account_deletion::erase_account` deletes every row and
//!    document entry. Synchronous: it is finished before the response.
//! 5. **Chat server.** The cleanup job redacts every message and deactivates
//!    the Matrix account. It is tried immediately (a small account is fully
//!    gone before the response) and otherwise left to the background worker,
//!    which survives restarts. See `docs/account-deletion.md`.

use super::{cookies_of, json_response, me_uid, parse_body_strict};
use crate::state::AppState;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::Response;
use mitch_lib::account_deletion::{self, Identity};
use mitch_lib::{auth, chat_cleanup, vm as vmlib};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// How long the request waits for the chat server before answering and
/// leaving the rest to the background worker.
const CHAT_WAIT: Duration = Duration::from_secs(12);

pub(crate) async fn delete_account(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Response {
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let cookies = cookies_of(state, headers);
    let uid = me_uid(&cookies);
    if !auth::valid_id(&uid, &state.id_secret) {
        return json_response(401, json!({ "error": "Not signed in." }));
    }
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &uid) else {
        return json_response(401, json!({ "error": "Not signed in." }));
    };
    let norm = auth::normalize_email(&email);

    if auth::is_admin_email(&state.store, &norm)
        || auth::is_owner_email(&state.store, &norm)
        || auth::is_co_owner_email(&state.store, &norm)
        || auth::is_moderator_email(&state.store, &norm)
    {
        return json_response(
            403,
            json!({ "error": "Staff accounts can't be deleted here. Ask the owner." }),
        );
    }

    if let Err(response) = confirm_identity(state, &norm, &body) {
        return *response;
    }

    if has_open_marketplace_listing(state, &norm) {
        return json_response(
            409,
            json!({ "error": "You have an open marketplace listing. Cancel or finish it first." }),
        );
    }

    // Everything that names the account, gathered while the lookups still work.
    let sids = account_deletion::session_ids_for(&state.store, &norm);
    let chat_ids = crate::routes::matrix::matrix_user_ids_for_account(state, &norm);

    if let Err(message) = destroy_computers(state, &norm).await {
        return json_response(502, json!({ "error": message }));
    }

    // Before anything is erased: re-registering this address never earns the
    // one-time referral bonus again.
    account_deletion::mark_deleted(&state.store, &state.id_secret, &norm);

    let cutoff = mitch_lib::school::now_millis();
    if chat_cleanup::begin_erasure(&state.store, &norm, cutoff, &chat_ids).is_err() {
        return json_response(
            500,
            json!({ "error": "Could not start deleting your chat history. Nothing was deleted; try again." }),
        );
    }

    let aliases =
        sids.into_iter()
            .chain(chat_ids.iter().cloned())
            .chain(chat_ids.iter().filter_map(|id| {
                id.strip_prefix('@')
                    .and_then(|rest| rest.split(':').next())
                    .map(str::to_string)
            }));
    let identity = Identity::new(&norm, aliases);
    let report = account_deletion::erase_account(&state.store, &identity);
    tracing::info!(
        rows = report.rows,
        documents = report.documents,
        log_lines = report.log_lines,
        "account deleted"
    );

    forget_in_memory(state, &norm);

    // Chat: redact history and deactivate the Matrix account. A timeout just
    // hands the rest to the background worker, which picks it up within a minute.
    let chat_done = tokio::time::timeout(
        CHAT_WAIT,
        crate::chat_cleanup::run_job(state, &norm, cutoff),
    )
    .await
    .unwrap_or(false);
    let _ = mitch_lib::chat_cleanup::remove_files(&state.store);

    let mut resp = json_response(200, json!({ "success": true, "chatPending": !chat_done }));
    clear_login_cookies(&mut resp);
    resp
}

/// Marketplace listings hold other people's coins in escrow; erasing one
/// would strand them, so deletion waits until the account has none open.
fn has_open_marketplace_listing(state: &Arc<AppState>, norm: &str) -> bool {
    let listings = state
        .store
        .read_document(&state.data_dir().join("marketplace.json"), json!([]));
    let is_me = |field: Option<&Value>| {
        field
            .and_then(Value::as_str)
            .is_some_and(|e| auth::normalize_email(e) == norm)
    };
    let open = listings.as_array().cloned().unwrap_or_else(|| {
        listings
            .get("listings")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    });
    open.iter().any(|l| {
        matches!(
            l.get("status").and_then(Value::as_str),
            Some("active" | "pending")
        ) && (is_me(l.get("seller")) || is_me(l.get("buyer")) || is_me(l.get("mediator")))
    })
}

/// Password (or, for accounts without one, the email address) and the
/// authenticator code when TOTP is enabled.
fn confirm_identity(state: &Arc<AppState>, norm: &str, body: &Value) -> Result<(), Box<Response>> {
    let wrong = |message: &str| Box::new(json_response(401, json!({ "error": message })));
    let stored = mitch_lib::passwords::password_hash_get(&state.store, norm).unwrap_or_default();
    if stored.is_empty() {
        let typed = body
            .get("confirmEmail")
            .and_then(Value::as_str)
            .unwrap_or("");
        if typed.is_empty() {
            return Err(Box::new(json_response(
                400,
                json!({ "error": "Type your email address to confirm.", "needs": "email" }),
            )));
        }
        if auth::normalize_email(typed) != norm {
            return Err(wrong("That isn't this account's email address."));
        }
    } else {
        let password = body.get("password").and_then(Value::as_str).unwrap_or("");
        if password.is_empty() || !mitch_lib::crypto::argon2_verify(&stored, password) {
            return Err(wrong("Current password incorrect."));
        }
    }
    let twofa = super::security::two_factor_config(state, norm);
    if twofa.enabled && twofa.type_ == "totp" {
        let code = body.get("totpCode").and_then(Value::as_str).unwrap_or("");
        if code.is_empty() {
            return Err(Box::new(json_response(
                400,
                json!({ "error": "Enter your authenticator code.", "needs": "totp" }),
            )));
        }
        if !mitch_lib::totp::verify_totp(&twofa.secret, code) {
            return Err(wrong("Invalid authenticator code."));
        }
    }
    Ok(())
}

/// Destroys every computer the account owns. Any failure aborts the deletion.
async fn destroy_computers(state: &Arc<AppState>, norm: &str) -> Result<(), String> {
    for record in vmlib::get_virtual_machines_for_owner(&state.store, norm) {
        if record.vmid > 0.0 {
            let json = record.to_json();
            if let Err(error) = crate::proxmox_desktop::desktop()
                .delete_guest(&json, false)
                .await
            {
                tracing::warn!("account deletion: computer removal failed: {error:?}");
                return Err(
                    "Your computer couldn't be removed, so nothing was deleted. Try again in a minute."
                        .into(),
                );
            }
        }
        vmlib::delete_virtual_machine(&state.store, &record.id);
        crate::routes::admin::vm::remove_vm_desktop_credentials(state, &record.id);
        crate::routes::vm::revoke_vm_desktop_connections(state, &record.id);
    }
    Ok(())
}

/// State that lives only in memory: relay keys and queued messages, running
/// casino hands, live sockets and presence.
fn forget_in_memory(state: &Arc<AppState>, norm: &str) {
    crate::chat_cleanup::clear_relay(state, norm);
    state
        .e2e_users
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|(_, user)| auth::normalize_email(&user.email) != norm);
    state
        .blackjack_games
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(norm);
    state
        .poker_games
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(norm);
    crate::ws::disconnect_banned_user(state, norm, "This account was deleted.");
}

fn clear_login_cookies(resp: &mut Response) {
    let secure_flag = std::env::var("SESSION_COOKIE_SECURE").unwrap_or_default();
    let production = std::env::var("NODE_ENV").unwrap_or_default() == "production";
    for (name, http_only) in [
        ("mitch_session", true),
        ("studentId", false),
        ("id", false),
        ("password", false),
        ("adminId", false),
    ] {
        if let Ok(value) = HeaderValue::from_str(&auth::clear_cookie_header(
            name,
            &secure_flag,
            production,
            http_only,
        )) {
            resp.headers_mut()
                .append(axum::http::header::SET_COOKIE, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> (Arc<AppState>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "mitch-server-delete-account-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap_or_default();
        let cfg = crate::hosts::SiteConfig {
            data_dir: dir.join("data"),
            ..crate::hosts::SiteConfig::load()
        };
        let store = Arc::new(
            mitch_lib::data::DataStore::open(&dir, &dir.join("data"))
                .unwrap_or_else(|e| panic!("store: {e}")),
        );
        (Arc::new(AppState::new(cfg, store)), dir)
    }

    fn headers_for(state: &Arc<AppState>, email: &str) -> HeaderMap {
        let norm = auth::normalize_email(email);
        let sess = auth::create_auth_session(&state.store, &state.id_secret, &norm, email, false);
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_str(&format!(
                "mitch_session={}; studentId={}",
                sess.token, sess.sid
            ))
            .unwrap(),
        );
        headers
    }

    fn coin_balance(state: &Arc<AppState>, email: &str) -> i64 {
        mitch_lib::coins::get_coins(&state.store, state.data_dir(), email) as i64
    }

    #[tokio::test]
    async fn deletes_everything_after_the_right_password_and_nothing_before() {
        let (state, dir) = test_state();
        let email = "test.student1@student.rjuhsd.us";
        let other = "test.student2@student.rjuhsd.us";
        mitch_lib::passwords::password_set(
            &state.store,
            &auth::normalize_email(email),
            &mitch_lib::crypto::argon2_hash("hunter2hunter2"),
        );
        mitch_lib::coins::set_coins(&state.store, state.data_dir(), email, 50.0, "test");
        mitch_lib::coins::set_coins(&state.store, state.data_dir(), other, 70.0, "test");
        state
            .store
            .write_document(
                &dir.join("data/dms.json"),
                &json!([
                    {"from": email, "to": other, "text": "bye", "ts": 1},
                    {"from": other, "to": "third@x.test", "text": "keep", "ts": 1}
                ]),
            )
            .unwrap();
        let headers = headers_for(&state, email);

        let wrong = delete_account(&state, &headers, br#"{"password":"nope"}"#).await;
        assert_eq!(wrong.status(), 401);
        assert_eq!(
            coin_balance(&state, email),
            50,
            "a wrong password deletes nothing"
        );

        let ok = delete_account(&state, &headers, br#"{"password":"hunter2hunter2"}"#).await;
        assert_eq!(ok.status(), 200);
        assert!(ok
            .headers()
            .get_all(axum::http::header::SET_COOKIE)
            .iter()
            .any(|c| c.to_str().unwrap_or("").starts_with("mitch_session=")));
        assert_eq!(coin_balance(&state, email), 0);
        assert_eq!(coin_balance(&state, other), 70);
        assert!(!mitch_lib::passwords::password_exists(&state.store, email));
        let dms = state
            .store
            .read_document(&dir.join("data/dms.json"), json!([]));
        assert_eq!(dms.as_array().unwrap().len(), 1);
        assert_eq!(dms[0]["text"], "keep");
        // The old session no longer resolves to anyone.
        let again = delete_account(&state, &headers, br#"{"password":"hunter2hunter2"}"#).await;
        assert_eq!(again.status(), 401);
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn an_open_marketplace_listing_blocks_deletion_and_a_referral_cannot_be_replayed() {
        let (state, dir) = test_state();
        let email = "test.student3@student.rjuhsd.us";
        mitch_lib::passwords::password_set(
            &state.store,
            &auth::normalize_email(email),
            &mitch_lib::crypto::argon2_hash("hunter2hunter2"),
        );
        state
            .store
            .write_document(
                &dir.join("data/marketplace.json"),
                &json!([{"seller": email, "status": "pending", "buyer": "b@x.test"}]),
            )
            .unwrap();
        let headers = headers_for(&state, email);
        let blocked = delete_account(&state, &headers, br#"{"password":"hunter2hunter2"}"#).await;
        assert_eq!(blocked.status(), 409);
        state
            .store
            .write_document(&dir.join("data/marketplace.json"), &json!([]))
            .unwrap();
        let ok = delete_account(&state, &headers, br#"{"password":"hunter2hunter2"}"#).await;
        assert_eq!(ok.status(), 200);
        assert!(mitch_lib::account_deletion::was_deleted(
            &state.store,
            &state.id_secret,
            &auth::normalize_email(email)
        ));
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn staff_cannot_delete_themselves() {
        let (state, dir) = test_state();
        state
            .store
            .write_document(
                &dir.join("data/admins.json"),
                &json!({ "owners": ["owner@x.test"], "admins": ["admin@x.test"] }),
            )
            .unwrap();
        let headers = headers_for(&state, "admin@x.test");
        let resp = delete_account(&state, &headers, br#"{"password":"x"}"#).await;
        assert_eq!(resp.status(), 403);
        std::fs::remove_dir_all(dir).ok();
    }
}
