//! Public support portal API (`/api/support/*`), for people who may not be able
//! to sign in. See `docs/support.md` for the whole flow and its rules.
//!
//! No session is required. A ticket's creator holds a secret token (shown once,
//! kept in their browser); only its SHA-256 hash is stored. Every call after
//! creation proves possession of that token in the JSON body (never the URL, so
//! it stays out of logs). Responses never say whether a typed username or email
//! matched an account, and never reveal staff identities.

use std::sync::Arc;

use axum::http::{HeaderMap, Method};
use serde_json::{json, Value};

use crate::errors::json_resp;
use crate::state::AppState;
use mitch_lib::support as db;

type Response = axum::response::Response;

const HOUR_MS: i64 = 60 * 60 * 1000;
const MAX_TICKETS_PER_IP_PER_HOUR: i64 = 5;
const MAX_TICKETS_PER_HOUR: i64 = 300;
const MAX_USER_MESSAGES: i64 = 200;
const MIN_USER_MESSAGE_GAP_MS: i64 = 3000;

pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Option<Response> {
    if !path.starts_with("/api/support/") || *method != Method::POST {
        return None;
    }
    let body: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
    Some(match path {
        "/api/support/open" => open_ticket(state, headers, &body).await,
        "/api/support/view" => view_ticket(state, &body),
        "/api/support/reply" => reply(state, &body),
        "/api/support/confirm-email" => confirm_email(state, &body).await,
        _ => return None,
    })
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

pub(crate) fn token_hash(token: &str) -> String {
    mitch_lib::crypto::sha256_hex(token.as_bytes())
}

/// Returns the ticket when `body` carries a matching `id` and `token`.
fn authorized_ticket(state: &AppState, body: &Value) -> Option<db::Ticket> {
    let id = text(body, "id", 64);
    let token = text(body, "token", 128);
    if id.is_empty()
        || token.is_empty()
        || !db::token_matches(&state.store, &id, &token_hash(&token))
    {
        return None;
    }
    db::ticket(&state.store, &id)
}

fn not_found() -> Response {
    json_resp(404, json!({ "error": "Ticket not found." }))
}

async fn open_ticket(state: &Arc<AppState>, headers: &HeaderMap, body: &Value) -> Response {
    // Hidden field real people never fill in; bots do. Answer as if it worked.
    if !text(body, "website", 200).is_empty() {
        return json_resp(
            200,
            json!({ "ok": true, "id": "0000000000000000", "token": "0".repeat(48) }),
        );
    }
    let category = text(body, "category", 20);
    if !db::CATEGORIES.contains(&category.as_str()) {
        return json_resp(400, json!({ "error": "Choose what you need help with." }));
    }
    let message = text(body, "message", 4000);
    if message.is_empty() {
        return json_resp(400, json!({ "error": "Describe the problem." }));
    }
    let account = text(body, "account", 254);
    if account.is_empty() {
        return json_resp(400, json!({ "error": "Enter your username or email." }));
    }
    let contact = text(body, "contactEmail", 254).to_lowercase();
    if !contact.is_empty() && (!contact.contains('@') || contact.contains(char::is_whitespace)) {
        return json_resp(
            400,
            json!({ "error": "That contact email doesn't look right." }),
        );
    }
    let subject = {
        let s = text(body, "subject", 120);
        if s.is_empty() {
            message.chars().take(60).collect()
        } else {
            s
        }
    };

    let ip = crate::handler::get_real_ip(headers, None);
    if !crate::routes::push::verify_recaptcha(state, &text(body, "recaptcha_token", 4000), &ip, "")
        .await
    {
        return json_resp(
            400,
            json!({ "error": "Verification failed. Please try again." }),
        );
    }
    let ip_hash =
        mitch_lib::crypto::hmac_sha256_hex(&state.id_secret, format!("support-ip:{ip}").as_bytes());
    let now = mitch_lib::school::now_millis();
    if db::count_recent_by_ip(&state.store, &ip_hash, now - HOUR_MS) >= MAX_TICKETS_PER_IP_PER_HOUR
        || db::count_recent(&state.store, now - HOUR_MS) >= MAX_TICKETS_PER_HOUR
    {
        return json_resp(
            429,
            json!({ "error": "Too many requests. Please try again later." }),
        );
    }

    // Link the ticket to an account when the typed text matches one, without
    // telling the visitor either way.
    let account_email = crate::routes::auth::resolve_login_identifier(state, &account)
        .filter(|e| mitch_lib::passwords::password_exists(&state.store, e))
        .unwrap_or_default();

    let token = mitch_lib::crypto::random_bytes_hex(24);
    let Some(id) = db::create_ticket(
        &state.store,
        &db::NewTicket {
            token_hash: &token_hash(&token),
            ip_hash: &ip_hash,
            category: &category,
            subject: &subject,
            claimed_account: &account,
            account_email: &account_email,
            contact_email: &contact,
        },
    ) else {
        return json_resp(
            500,
            json!({ "error": "Could not save your request. Try again." }),
        );
    };
    db::add_message(&state.store, &id, "user", "", &message);
    db::prune_old(&state.store, now);
    json_resp(200, json!({ "ok": true, "id": id, "token": token }))
}

/// What the ticket's owner may see of an email change: a state, never a name.
fn email_change_state(state: &AppState, ticket_id: &str) -> Value {
    let Some(req) = db::email_requests_for_ticket(&state.store, ticket_id)
        .into_iter()
        .next_back()
    else {
        return Value::Null;
    };
    let label = match req.status.as_str() {
        "pending" => "review",
        "approved" => "awaiting_code",
        "applied" => "done",
        "denied" => "denied",
        _ => "expired",
    };
    json!({
        "state": label,
        "newEmail": mitch_lib::admin::mask_email(&req.new_email),
    })
}

fn view_ticket(state: &Arc<AppState>, body: &Value) -> Response {
    let Some(t) = authorized_ticket(state, body) else {
        return not_found();
    };
    let messages: Vec<Value> = db::messages(&state.store, &t.id)
        .into_iter()
        .map(|m| {
            json!({
                "id": m.id,
                "from": m.sender,
                "body": m.body,
                "at": m.created_at,
            })
        })
        .collect();
    json_resp(
        200,
        json!({
            "ok": true,
            "ticket": {
                "id": t.id,
                "status": t.status,
                "category": t.category,
                "subject": t.subject,
                "createdAt": t.created_at,
            },
            "messages": messages,
            "emailChange": email_change_state(state, &t.id),
        }),
    )
}

fn reply(state: &Arc<AppState>, body: &Value) -> Response {
    let Some(t) = authorized_ticket(state, body) else {
        return not_found();
    };
    let message = text(body, "body", 4000);
    if message.is_empty() {
        return json_resp(400, json!({ "error": "Write a message first." }));
    }
    let now = mitch_lib::school::now_millis();
    if now - db::last_message_at(&state.store, &t.id, "user") < MIN_USER_MESSAGE_GAP_MS {
        return json_resp(429, json!({ "error": "Slow down a little." }));
    }
    if db::message_count(&state.store, &t.id, "user") >= MAX_USER_MESSAGES {
        return json_resp(
            429,
            json!({ "error": "This conversation is full. Start a new request." }),
        );
    }
    db::add_message(&state.store, &t.id, "user", "", &message);
    // A reply to a closed ticket reopens it so staff see it in the queue.
    if t.status != "open" {
        db::set_status(&state.store, &t.id, "open");
    }
    json_resp(200, json!({ "ok": true }))
}

/// The user types the code emailed to the *new* address; that proves they
/// control it. Only then is the admin-approved change applied.
async fn confirm_email(state: &Arc<AppState>, body: &Value) -> Response {
    let Some(t) = authorized_ticket(state, body) else {
        return not_found();
    };
    let code = text(body, "code", 12);
    let hash = mitch_lib::crypto::sha256_hex(format!("{}:{}", t.id, code).as_bytes());
    let req = match db::check_code(&state.store, &t.id, &hash) {
        db::CodeCheck::Ok(req) => *req,
        db::CodeCheck::Wrong => {
            return json_resp(400, json!({ "error": "That code isn't right." }));
        }
        db::CodeCheck::TooManyAttempts => {
            return json_resp(
                429,
                json!({ "error": "Too many wrong codes. Ask support to review again." }),
            );
        }
        db::CodeCheck::Expired => {
            return json_resp(
                400,
                json!({ "error": "That code expired. Ask support to review again." }),
            );
        }
        db::CodeCheck::Nothing => {
            return json_resp(
                400,
                json!({ "error": "There's no email change waiting for a code." }),
            );
        }
    };

    // Real address of the old account, for the heads-up email below.
    let old_address = mitch_lib::profile::canonical_delivery_email(
        &state.store,
        state.data_dir(),
        &state.id_secret,
        &req.old_email,
    );
    if let Err(err) = crate::routes::me::account::apply_email_change(
        state,
        &req.old_email,
        &req.new_email,
        &req.new_email,
    )
    .await
    {
        // A transient chat outage can be retried; anything else cancels it.
        if !matches!(
            err,
            crate::routes::me::account::EmailChangeError::ChatUnavailable
        ) {
            db::mark_expired(&state.store, req.id);
            db::add_message(
                &state.store,
                &t.id,
                "system",
                "",
                "The email change could not be completed and was cancelled.",
            );
        }
        return json_resp(err.status(), json!({ "error": err.message() }));
    }
    db::mark_applied(&state.store, req.id);

    // Compromised accounts: the old password and any passkeys may be known to
    // someone else, so neither survives. The person resets their password
    // from the sign-in page using the new address.
    if t.compromised {
        let random = mitch_lib::crypto::random_bytes_hex(32);
        mitch_lib::passwords::password_set(
            &state.store,
            &req.new_email,
            &mitch_lib::crypto::argon2_hash(&random),
        );
        let file = state.data_dir().join("passkeys.json");
        let mut passkeys = state.store.read_document(&file, json!({}));
        if let Some(map) = passkeys.as_object_mut() {
            if map.remove(&req.new_email).is_some() {
                let _ = state.store.write_document(&file, &passkeys);
            }
        }
    }
    db::add_message(
        &state.store,
        &t.id,
        "system",
        "",
        if t.compromised {
            "Your email was changed. Because the account was reported compromised, its password and passkeys were reset: use \"Reset password\" on the sign-in page with your new email."
        } else {
            "Your email was changed. Sign in with the new address."
        },
    );
    db::set_status(&state.store, &t.id, "resolved");
    if old_address.contains('@') {
        crate::routes::push::send_email_bg(
            state,
            &old_address,
            "Your mitch.pro email was changed",
            "The email on your mitch.pro account was changed after a support request. If this wasn't you, contact support right away.",
        );
    }
    json_resp(200, json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    fn test_state() -> (Arc<AppState>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "mitch-server-support-test-{}",
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

    async fn json_of(resp: Response) -> (u16, Value) {
        let status = resp.status().as_u16();
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(json!({})))
    }

    fn ticket_for(state: &AppState, account_email: &str) -> (String, String) {
        let token = "secret-token".to_string();
        let id = db::create_ticket(
            &state.store,
            &db::NewTicket {
                token_hash: &token_hash(&token),
                ip_hash: "ip",
                category: "email",
                subject: "help",
                claimed_account: "user",
                account_email,
                contact_email: "",
            },
        )
        .unwrap();
        (id, token)
    }

    #[tokio::test]
    async fn a_ticket_needs_its_own_token_and_replies_are_rate_limited() {
        let (state, dir) = test_state();
        let (id, token) = ticket_for(&state, "");
        let (status, _) =
            json_of(view_ticket(&state, &json!({ "id": id, "token": "wrong" }))).await;
        assert_eq!(status, 404);
        let (status, _) = json_of(view_ticket(&state, &json!({ "id": id, "token": "" }))).await;
        assert_eq!(status, 404);
        let (status, v) = json_of(view_ticket(&state, &json!({ "id": id, "token": token }))).await;
        assert_eq!(status, 200);
        assert!(v["ticket"].get("tokenHash").is_none());
        let reply_body = json!({ "id": id, "token": token, "body": "hello" });
        assert_eq!(json_of(reply(&state, &reply_body)).await.0, 200);
        assert_eq!(json_of(reply(&state, &reply_body)).await.0, 429);
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn approved_change_applies_only_with_the_emailed_code_and_resets_compromised_accounts() {
        let (state, dir) = test_state();
        mitch_lib::passwords::password_set(&state.store, "user@x.test", "oldhash");

        let passkeys = state.data_dir().join("passkeys.json");
        state
            .store
            .write_document(&passkeys, &json!({ "user@x.test": [{ "id": "cred" }] }))
            .unwrap();
        let (id, token) = ticket_for(&state, "user@x.test");
        db::set_compromised(&state.store, &id, true);
        let req =
            db::create_email_request(&state.store, &id, "new@x.test", "verified", "mod@x.test")
                .unwrap();
        let hash = mitch_lib::crypto::sha256_hex(format!("{id}:123456").as_bytes());
        db::decide_email_request(&state.store, req, true, "admin@x.test", "", &hash).unwrap();

        let wrong = json!({ "id": id, "token": token, "code": "000000" });
        assert_eq!(json_of(confirm_email(&state, &wrong).await).await.0, 400);
        assert!(mitch_lib::passwords::password_exists(
            &state.store,
            "user@x.test"
        ));
        let right = json!({ "id": id, "token": token, "code": "123456" });
        assert_eq!(json_of(confirm_email(&state, &right).await).await.0, 200);

        assert!(!mitch_lib::passwords::password_exists(
            &state.store,
            "user@x.test"
        ));
        assert!(mitch_lib::passwords::password_exists(
            &state.store,
            "new@x.test"
        ));
        assert_ne!(
            mitch_lib::passwords::password_hash_get(&state.store, "new@x.test").unwrap(),
            "oldhash",
            "a compromised account must not keep its old password"
        );
        let keys = state.store.read_document(&passkeys, json!({}));
        assert!(keys.get("new@x.test").is_none() && keys.get("user@x.test").is_none());

        assert_eq!(db::ticket(&state.store, &id).unwrap().status, "resolved");
        // The code works once.
        assert_eq!(json_of(confirm_email(&state, &right).await).await.0, 400);
        std::fs::remove_dir_all(dir).ok();
    }
}
