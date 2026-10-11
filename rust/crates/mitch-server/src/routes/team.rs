//! `/api/team/*` — the 11-endpoint gmail/support bridge (plan Step 13,
//! server.js:20094-20145 GET + 22984-23096 POST + 23814-23867 AI).
//!
//! Auth is a per-member `Authorization: Bearer <tok>` looked up in the
//! BASE-relative `admin/team_tokens.json` DISK file — deliberately outside
//! `dataDir`, so it is a plain file read (`shouldStoreInDb` is false for
//! `admin/`), NOT a DB doc (server.js:385, `checkTeamToken` 20094-20102).
//! These routes sit behind the standard password + CSRF gates (they are not
//! in PUBLIC_API_PATHS and the POSTs are not CSRF-exempt), like the JS.
//!
//! `data/team_inbox_cache.json` is read RAW from disk on purpose (JS
//! `readFileSync`, not `loadJson`) — the IMAP watcher owns that file and
//! writes it directly; a DB blob here would shadow its refetches.

use crate::routes::me::{json_response, parse_body_strict};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use serde_json::{json, Value};
use std::sync::Arc;

pub(crate) async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
    search: &str,
) -> Option<Response> {
    let get = *method == Method::GET;
    let post = *method == Method::POST;
    if !path.starts_with("/api/team/") {
        return None;
    }
    if path == "/api/team/gmail" && get {
        let Some(_member) = check_team_token(state, headers) else {
            return Some(json_response(401, json!({ "error": "Invalid team token" })));
        };
        let emails = state
            .store
            .read_document(&gmail_cache_file(state), json!([]));
        return Some(json_response(200, json!({ "messages": emails })));
    }
    if path == "/api/team/gmail-status" && get {
        let Some(_member) = check_team_token(state, headers) else {
            return Some(json_response(401, json!({ "error": "Invalid team token" })));
        };
        let paused = gmail_pause_file(state).exists();
        return Some(json_response(200, json!({ "paused": paused })));
    }
    if path == "/api/team/gmail-sent" && get {
        let Some(_member) = check_team_token(state, headers) else {
            return Some(json_response(401, json!({ "error": "Invalid team token" })));
        };
        let sent = state
            .store
            .read_document(&gmail_sent_file(state), json!({}));
        return Some(json_response(200, sent));
    }
    if path == "/api/team/inbox" && get {
        let Some(_member) = check_team_token(state, headers) else {
            return Some(json_response(401, json!({ "error": "Invalid team token" })));
        };
        return Some(inbox(state));
    }
    if path == "/api/team/email" && get {
        let Some(_member) = check_team_token(state, headers) else {
            return Some(json_response(401, json!({ "error": "Invalid team token" })));
        };
        let uid = crate::handler::query(search)
            .get("uid")
            .cloned()
            .unwrap_or_default();
        if uid.is_empty() {
            return Some(json_response(400, json!({ "error": "uid required" })));
        }
        let cache = read_inbox_cache(state);
        let msg = cache
            .as_array()
            .and_then(|a| {
                a.iter().find(|m| {
                    mitch_lib::jsval::string_of(m.get("uid")).as_str() == uid.as_str()
                        && m.get("mailbox").and_then(Value::as_str) == Some("INBOX")
                })
            })
            .cloned();
        let Some(msg) = msg else {
            return Some(json_response(404, json!({ "error": "not found" })));
        };
        return Some(json_response(200, msg));
    }
    if post {
        let member = check_team_token(state, headers);
        let Some(member) = member else {
            return Some(json_response(401, json!({ "error": "Invalid team token" })));
        };
        let member_name = mitch_lib::jsval::string_of(member.get("name"));
        return Some(match path {
            "/api/team/reply" => reply(state, body_bytes, &member_name).await,
            "/api/team/handled" => handled(state, body_bytes),
            "/api/team/unsubscribe" => unsubscribe(state, body_bytes, &member_name),
            "/api/team/gmail-toggle" => gmail_toggle(state, &member_name),
            "/api/team/gmail-reply" => gmail_reply(state, body_bytes, &member_name).await,
            "/api/team/ai" => ai(state, body_bytes, &member_name).await,
            _ => return None,
        });
    }
    None
}

// ── Token + path helpers ─────────────────────────────────────────────────────

/// `checkTeamToken(req)` / `checkTeamTokenPost(req)` (server.js:20094-20101,
/// 22987-22994): Bearer token -> `{token, ...tokens[tok]}`, or None.
fn check_team_token(state: &AppState, headers: &HeaderMap) -> Option<Value> {
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let tok = auth.strip_prefix("Bearer ")?.trim();
    if tok.is_empty() {
        return None;
    }
    // BASE-relative DISK file (admin/ is outside dataDir → readDocument falls
    // through to the plain file read; rust read_document mirrors that).
    let tokens = state.store.read_document(
        &state.cfg.base_dir.join("admin/team_tokens.json"),
        json!({}),
    );
    let rec = tokens.get(tok)?;
    let mut out = json!({ "token": tok });
    if let Some(obj) = rec.as_object() {
        for (k, v) in obj {
            out[k] = v.clone();
        }
    }
    Some(out)
}

/// `GMAIL_CACHE_FILE` (server.js:4346) — DB-backed like every file under
/// `mail/`.
fn gmail_cache_file(state: &AppState) -> std::path::PathBuf {
    state.cfg.base_dir.join("mail/check_email/emails.json")
}

/// `GMAIL_PAUSE_FILE` (server.js:4347) — marker file, plain disk.
fn gmail_pause_file(state: &AppState) -> std::path::PathBuf {
    state.cfg.base_dir.join("mail/check_email/gmail_paused")
}

/// `GMAIL_SENT_FILE` (server.js:4348) — DB-backed.
fn gmail_sent_file(state: &AppState) -> std::path::PathBuf {
    state.cfg.base_dir.join("mail/check_email/gmail_sent.json")
}

/// `TEAM_INBOX_CACHE` (server.js:382) — RAW disk reads/writes; the IMAP
/// watcher owns this file.
fn team_inbox_cache(state: &AppState) -> std::path::PathBuf {
    state.cfg.data_dir.join("team_inbox_cache.json")
}

/// The deliberate raw disk read (server.js:20123, 22980):
/// `JSON.parse(readFileSync(TEAM_INBOX_CACHE))` with catch → `[]`.
fn read_inbox_cache(state: &AppState) -> Value {
    std::fs::read_to_string(team_inbox_cache(state))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or(json!([]))
}

/// `threadKey(subject)` (server.js:1725) — strip one leading `re:`/`fw:`/`fwd:`
/// (case-insensitive, with the colon run of spaces), then trim + lowercase.
fn thread_key(subject: &str) -> String {
    let lower = subject.to_lowercase();
    let stripped = if let Some(rest) = lower.strip_prefix("fwd:") {
        let _ = rest;
        subject[4..].to_string()
    } else if lower.starts_with("re:") || lower.starts_with("fw:") {
        subject[3..].to_string()
    } else {
        subject.to_string()
    };
    stripped.trim().to_lowercase()
}

/// GET /api/team/inbox (server.js:20119-20135).
fn inbox(state: &Arc<AppState>) -> Response {
    let cached = read_inbox_cache(state);
    let Some(arr) = cached.as_array() else {
        return json_response(200, json!({ "messages": [] }));
    };
    let handled = state
        .store
        .read_document(&state.cfg.data_dir.join("team_handled.json"), json!({}));
    let messages: Vec<Value> = arr
        .iter()
        .map(|m| {
            let mut out = m.clone();
            if let Some(obj) = out.as_object_mut() {
                let uid_key = mitch_lib::jsval::string_of(m.get("uid"));
                obj.insert(
                    "handled".into(),
                    json!(handled.get(&uid_key).is_some_and(mitch_lib::jsval::truthy)),
                );
                obj.insert(
                    "senderPremium".into(),
                    json!(mitch_lib::auth::is_premium_email(
                        &state.store,
                        &m.get("from")
                            .map(mitch_lib::jsval::string)
                            .unwrap_or_default()
                    )),
                );
            }
            out
        })
        .collect();
    json_response(200, json!({ "messages": messages }))
}

// ── POST endpoints ───────────────────────────────────────────────────────────

/// POST /api/team/reply (server.js:22996-23046) — send via the mitch-mail
/// service, then append the Sent record straight to the disk cache the
/// IMAP watcher owns (tmp+rename).
async fn reply(state: &Arc<AppState>, body_bytes: &[u8], member_name: &str) -> Response {
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    // JS destructuring: absent keys are `undefined` (falsy), so the
    // missing-field gate is truthiness on the raw values, not String().
    let has = |k: &str| body.get(k).map(mitch_lib::jsval::truthy).unwrap_or(false);
    let to = mitch_lib::jsval::string(&body["to"]);
    let subject = mitch_lib::jsval::string(&body["subject"]);
    let reply_body = mitch_lib::jsval::string(&body["body"]);
    let in_reply_to = mitch_lib::jsval::string_of(body.get("inReplyTo"));
    if !(has("to") && has("subject") && has("body")) {
        return json_response(400, json!({ "error": "missing fields" }));
    }
    let in_reply_to_header = (!in_reply_to.is_empty()).then_some(in_reply_to);
    if let Err(err) = send_via_mitch_mail(
        "support",
        &to,
        &subject,
        &reply_body,
        in_reply_to_header,
        true,
    )
    .await
    {
        return json_response(500, json!({ "error": err }));
    }
    tracing::info!("[team] reply to {to} by {member_name}");

    // Append sent message to cache so it shows up immediately on next inbox
    // fetch. Written straight to the same disk file the IMAP watcher owns
    // (tmp+rename) — a DB blob here would shadow the watcher's refetches.
    let cache_path = team_inbox_cache(state);
    let mut cache = read_inbox_cache(state);
    if !cache.is_array() {
        cache = json!([]);
    }
    if let Some(arr) = cache.as_array_mut() {
        arr.push(json!({
            "uid": mitch_lib::school::now_millis(),
            "mailbox": "Sent",
            "dir": "out",
            "from": "support@mitch.pro",
            "fromName": "mitch.pro Support",
            "to": to,
            "subject": subject,
            "threadKey": thread_key(&subject),
            "date": mitch_lib::coins::js_iso_date(),
            "messageId": "",
            "seen": true,
            "body": reply_body,
        }));
        // JS: sort((a,b) => new Date(b.date) - new Date(a.date)) — newest first.
        arr.sort_by_key(|m| {
            std::cmp::Reverse(
                m.get("date")
                    .and_then(Value::as_str)
                    .and_then(crate::routes::admin::economy::parse_timestamp)
                    .unwrap_or(0),
            )
        });
        let content = mitch_lib::data::js_stringify_pretty(&cache);
        let tmp = cache_path.with_extension("json.tmp");
        if std::fs::write(&tmp, content).is_ok() {
            let _ = std::fs::rename(&tmp, &cache_path);
        }
    }
    json_response(200, json!({ "ok": true }))
}

/// POST /api/team/handled (server.js:23048-23060) — mark/unmark an inbox
/// message handled in the DB-backed team_handled.json.
fn handled(state: &Arc<AppState>, body_bytes: &[u8]) -> Response {
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let uid = mitch_lib::jsval::string_of(body.get("uid"));
    let val = body
        .get("handled")
        .map(mitch_lib::jsval::truthy)
        .unwrap_or(false);
    if uid.is_empty() {
        return json_response(400, json!({ "error": "uid required" }));
    }
    let file = state.cfg.data_dir.join("team_handled.json");
    let mut doc = state.store.read_document(&file, json!({}));
    if let Some(obj) = doc.as_object_mut() {
        if val {
            obj.insert(uid.clone(), json!(mitch_lib::school::now_millis()));
        } else {
            obj.remove(&uid);
        }
    }
    let _ = state.store.write_document(&file, &doc);
    json_response(200, json!({ "ok": true }))
}

/// POST /api/team/unsubscribe (server.js:23062-23076) — append to the
/// newsletter unsub list. The JS reads it via loadJson (DB) but writes the
/// DISK file with a sorted-unique 2-space dump; the DB copy is untouched by
/// this path (parity with that quirk preserved).
fn unsubscribe(state: &Arc<AppState>, body_bytes: &[u8], member_name: &str) -> Response {
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let email = mitch_lib::jsval::string_of(body.get("email"))
        .trim()
        .to_lowercase();
    if email.is_empty() || !email.contains('@') {
        return json_response(400, json!({ "error": "email required" }));
    }
    let file = state.cfg.data_dir.join("newsletter_unsub.json");
    let mut unsub = state
        .store
        .read_document(&file, json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    if !unsub.iter().any(|v| v == &json!(email)) {
        unsub.push(json!(email));
        let mut uniq: Vec<String> = unsub.iter().map(mitch_lib::jsval::string).collect();
        uniq.sort();
        uniq.dedup();
        // JS writeFileSync(file, JSON.stringify([...new Set(unsub)].sort(), null, 2))
        let content = mitch_lib::data::js_stringify_pretty(&json!(uniq));
        let _ = std::fs::write(&file, &content);
        let _ = state.store.write_document(&file, &json!(uniq));
    }
    tracing::info!("[team] unsubscribed {email} by {member_name}");
    json_response(200, json!({ "ok": true }))
}

/// POST /api/team/gmail-toggle (server.js:23078-23090) — flip the
/// gmail_paused marker file.
fn gmail_toggle(state: &Arc<AppState>, member_name: &str) -> Response {
    let pause = gmail_pause_file(state);
    if pause.exists() {
        let _ = std::fs::remove_file(&pause);
        tracing::info!("[team] gmail scraping resumed by {member_name}");
        json_response(200, json!({ "paused": false }))
    } else {
        let _ = std::fs::write(&pause, "");
        tracing::info!("[team] gmail scraping paused by {member_name}");
        json_response(200, json!({ "paused": true }))
    }
}

/// POST /api/team/gmail-reply (server.js:23092-23096) — send via the
/// mitch-mail service, then append under the thread key in the DB-backed
/// sent doc.
async fn gmail_reply(state: &Arc<AppState>, body_bytes: &[u8], member_name: &str) -> Response {
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    // JS destructuring truthiness gate, same as /reply.
    let has = |k: &str| body.get(k).map(mitch_lib::jsval::truthy).unwrap_or(false);
    let to = mitch_lib::jsval::string(&body["to"]);
    let subject = mitch_lib::jsval::string(&body["subject"]);
    let reply_body = mitch_lib::jsval::string(&body["body"]);
    let message_id = mitch_lib::jsval::string_of(body.get("messageId"));
    let body_thread_key = mitch_lib::jsval::string_of(body.get("threadKey"));
    if !(has("to") && has("subject") && has("body")) {
        return json_response(400, json!({ "error": "missing fields" }));
    }
    let in_reply_to_header = (!message_id.is_empty()).then(|| format!("<{message_id}>"));
    if let Err(err) = send_via_mitch_mail(
        "gmail",
        &to,
        &subject,
        &reply_body,
        in_reply_to_header,
        true,
    )
    .await
    {
        return json_response(500, json!({ "error": err }));
    }
    // JS: threadKey || messageId || (to + '|' + subject)
    let key = if !body_thread_key.is_empty() {
        body_thread_key
    } else if !message_id.is_empty() {
        message_id
    } else {
        format!("{to}|{subject}")
    };
    let file = gmail_sent_file(state);
    let mut sent = state.store.read_document(&file, json!({}));
    if let Some(obj) = sent.as_object_mut() {
        let entry = obj.entry(key).or_insert_with(|| json!([]));
        if let Some(arr) = entry.as_array_mut() {
            arr.push(json!({ "body": reply_body, "date": mitch_lib::coins::js_iso_date() }));
        }
    }
    let _ = state.store.write_document(&file, &sent);
    tracing::info!("[team] gmail reply to {to} by {member_name}");
    json_response(200, json!({ "ok": true }))
}

/// POST /api/team/ai (server.js:23814-23867) — the Groq-backed review/draft
/// helper, iterating the model list with 429/503 fallback.
async fn ai(_state: &Arc<AppState>, body_bytes: &[u8], member_name: &str) -> Response {
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let groq_key = std::env::var("GROQ_API_KEY")
        .unwrap_or_default()
        .trim()
        .to_string();
    if groq_key.is_empty() {
        return json_response(503, json!({ "error": "AI not configured" }));
    }
    let mode = mitch_lib::jsval::string_of(body.get("mode"));
    let subject = mitch_lib::jsval::string_of(body.get("subject"));
    let email_body = mitch_lib::jsval::string_of(body.get("emailBody"));
    let thread = body.get("thread").cloned().unwrap_or(Value::Null);
    if !mitch_lib::jsval::truthy(&json!(mode)) || !mitch_lib::jsval::truthy(&json!(email_body)) {
        return json_response(400, json!({ "error": "missing fields" }));
    }

    let thread_text = if thread.as_array().is_some_and(|t| t.len() > 1) {
        let Some(arr) = thread.as_array() else {
            return json_response(400, json!({ "error": "missing fields" }));
        };
        let parts: Vec<String> = arr
            .iter()
            .map(|m| {
                let who = if m.get("dir").and_then(Value::as_str) == Some("in") {
                    "Customer"
                } else {
                    "Support"
                };
                format!("{who}: {}", mitch_lib::jsval::string_of(m.get("body")))
            })
            .collect();
        format!("\n\nConversation thread:\n{}", parts.join("\n\n"))
    } else {
        String::new()
    };

    let (sys_prompt, user_prompt, is_draft) = if mode == "review" {
        (
            "You are a support email analyst for mitch.pro, a student-run tech/gaming service. Be concise and practical.".to_string(),
            format!(
                "Analyze this support email and return JSON only.\n\nFrom: {}\nSubject: {}\n\n{}{}\n\nReturn: {{\"summary\":\"1-2 sentence summary of what they need\",\"tone\":\"positive|neutral|negative|frustrated|urgent\",\"priority\":\"high|medium|low\",\"keyPoints\":[\"point1\",\"point2\"],\"suggestedAction\":\"one-line action to take\"}}",
                mitch_lib::jsval::str_or(body.get("sender"), "unknown"),
                subject,
                email_body,
                thread_text
            ),
            false,
        )
    } else if mode == "draft" {
        (
            "You are a support email writer for mitch.pro. Write concise, friendly, professional replies. Do NOT include a greeting (no \"Hi\" or \"Dear\"), no sign-off, and no subject line — just the reply body text.".to_string(),
            format!(
                "Write a reply to this support email.\n\nFrom: {}\nSubject: {}\n\n{}{}",
                mitch_lib::jsval::str_or(body.get("sender"), "unknown"),
                subject,
                email_body,
                thread_text
            ),
            true,
        )
    } else {
        return json_response(400, json!({ "error": "invalid mode" }));
    };

    let msgs = vec![
        json!({ "role": "system", "content": sys_prompt }),
        json!({ "role": "user", "content": user_prompt }),
    ];
    // GROQ_MODELS (server.js:188) — try each in order.
    let models = ["llama-3.3-70b-versatile", "llama-3.1-8b-instant"];
    for model in models {
        let payload = json!({
            "model": model,
            "messages": msgs,
            "max_tokens": if is_draft { 600 } else { 400 },
        });
        let resp = reqwest::Client::new()
            .post("https://api.groq.com/openai/v1/chat/completions")
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {groq_key}"))
            .header("User-Agent", "python-requests/2.31.0")
            .json(&payload)
            .timeout(std::time::Duration::from_secs(25))
            .send()
            .await;
        let Ok(resp) = resp else {
            tracing::error!("[team ai] request error");
            continue;
        };
        let status = resp.status().as_u16();
        if status == 429 || status == 503 {
            continue;
        }
        if !(200..300).contains(&status) {
            return json_response(502, json!({ "error": "AI request failed" }));
        }
        let Ok(data) = resp.json::<Value>().await else {
            tracing::error!("[team ai] bad response json");
            continue;
        };
        let text = data
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .map(|v| mitch_lib::jsval::or(Some(v), json!("")))
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        tracing::info!("[team ai] {mode} by {member_name} via {model}");
        if !is_draft {
            // JS: text.match(/\{[\s\S]*?\}/) — the first `{` through the
            // first `}` after it; a JSON.parse failure falls through to the
            // next model; no match at all → `{ summary: text }`.
            let Some(open) = text.find('{') else {
                return json_response(200, json!({ "summary": text }));
            };
            let Some(close_rel) = text[open..].find('}') else {
                return json_response(200, json!({ "summary": text }));
            };
            let candidate = &text[open..=open + close_rel];
            match serde_json::from_str::<Value>(candidate) {
                Ok(parsed) => return json_response(200, parsed),
                Err(_) => continue,
            }
        }
        return json_response(200, json!({ "draft": text.trim() }));
    }
    json_response(500, json!({ "error": "AI failed" }))
}

/// Sends one message through the mitch-mail service's `/send` endpoint
/// (replaces spawning `mail/send_email.js` / `mail/support_send.js` via
/// node/bun). `MAIL_RS_URL` matches the JS shim's own env var, same default.
async fn send_via_mitch_mail(
    sender: &str,
    to: &str,
    subject: &str,
    body: &str,
    in_reply_to: Option<String>,
    raw: bool,
) -> Result<(), String> {
    let base = std::env::var("MAIL_RS_URL").unwrap_or_else(|_| "http://127.0.0.1:6902".to_string());
    let resp = reqwest::Client::new()
        .post(format!("{base}/send"))
        .timeout(std::time::Duration::from_secs(30))
        .json(&json!({
            "sender": sender,
            "to": to,
            "subject": subject,
            "body": body,
            "inReplyTo": in_reply_to,
            "alt": false,
            "raw": raw,
            "dry_run": false,
        }))
        .send()
        .await
        .map_err(|e| format!("mitch-mail unreachable: {e}"))?;
    if resp.status().is_success() {
        return Ok(());
    }
    let parsed: Value = resp.json().await.unwrap_or_default();
    Err(parsed
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("send failed")
        .to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, Method};

    fn test_state() -> (Arc<AppState>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "mitch-server-team-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap_or_default();
        let cfg = crate::hosts::SiteConfig::load();
        let cfg = crate::hosts::SiteConfig {
            data_dir: dir.join("data"),
            base_dir: dir.clone(),
            ..cfg
        };
        let store = Arc::new(
            mitch_lib::data::DataStore::open(&dir, &dir.join("data"))
                .unwrap_or_else(|e| panic!("store: {e}")),
        );
        (Arc::new(AppState::new(cfg, Arc::clone(&store))), dir)
    }

    fn bearer(tok: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {tok}").parse().unwrap(),
        );
        h
    }

    fn body(v: Value) -> Vec<u8> {
        serde_json::to_vec(&v).unwrap()
    }

    fn code(resp: &Response) -> u16 {
        resp.status().as_u16()
    }

    async fn body_json(resp: Response) -> Value {
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap_or_default();
        serde_json::from_slice(&bytes).unwrap_or(json!({}))
    }

    fn seed_tokens(dir: &std::path::Path) {
        std::fs::create_dir_all(dir.join("admin")).unwrap_or_default();
        std::fs::create_dir_all(dir.join("mail/check_email")).unwrap_or_default();
        std::fs::write(
            dir.join("admin/team_tokens.json"),
            r#"{"tok-mitch": {"name": "Mitch", "role": "admin"}}"#,
        )
        .unwrap();
    }

    #[tokio::test]
    async fn team_token_gate() {
        let (state, dir) = test_state();
        seed_tokens(&dir);
        // Missing / non-Bearer / unknown / empty → None → 401.
        let h = HeaderMap::new();
        assert!(check_team_token(&state, &h).is_none());
        let mut bad = HeaderMap::new();
        bad.insert(
            axum::http::header::AUTHORIZATION,
            "Basic xyz".parse().unwrap(),
        );
        assert!(check_team_token(&state, &bad).is_none());
        assert!(check_team_token(&state, &bearer("nope")).is_none());
        assert!(check_team_token(&state, &bearer("")).is_none());
        // Valid → {token, ...record}.
        let member = check_team_token(&state, &bearer("tok-mitch")).unwrap();
        assert_eq!(member.get("token"), Some(&json!("tok-mitch")));
        assert_eq!(member.get("name"), Some(&json!("Mitch")));
        assert_eq!(member.get("role"), Some(&json!("admin")));
    }

    #[tokio::test]
    async fn get_endpoints_401_without_token() {
        let (state, _dir) = test_state();
        for p in [
            "/api/team/gmail",
            "/api/team/gmail-status",
            "/api/team/gmail-sent",
            "/api/team/inbox",
            "/api/team/email",
        ] {
            let resp = handle(&state, &Method::GET, p, &HeaderMap::new(), &[], "").await;
            let resp = resp.expect(p);
            assert_eq!(code(&resp), 401, "{p}");
        }
    }

    #[tokio::test]
    async fn gmail_getters() {
        let (state, dir) = test_state();
        seed_tokens(&dir);
        let h = bearer("tok-mitch");
        // /gmail — DB-backed cache doc (fallback [] when unset).
        let resp = handle(&state, &Method::GET, "/api/team/gmail", &h, &[], "")
            .await
            .unwrap();
        let v = body_json(resp).await;
        assert_eq!(v.get("messages"), Some(&json!([])));
        // /gmail-sent — raw doc (fallback {}).
        let resp = handle(&state, &Method::GET, "/api/team/gmail-sent", &h, &[], "")
            .await
            .unwrap();
        assert_eq!(code(&resp), 200);
        assert_eq!(body_json(resp).await, json!({}));
        // /gmail-status — paused mirrors the marker file.
        let resp = handle(&state, &Method::GET, "/api/team/gmail-status", &h, &[], "")
            .await
            .unwrap();
        assert_eq!(body_json(resp).await.get("paused"), Some(&json!(false)));
        std::fs::write(gmail_pause_file(&state), "").unwrap();
        let resp = handle(&state, &Method::GET, "/api/team/gmail-status", &h, &[], "")
            .await
            .unwrap();
        assert_eq!(body_json(resp).await.get("paused"), Some(&json!(true)));
    }

    #[tokio::test]
    async fn inbox_maps_handled_and_premium() {
        let (state, dir) = test_state();
        seed_tokens(&dir);
        std::fs::write(
            dir.join("data/team_inbox_cache.json"),
            r#"[{"uid": 7, "mailbox": "INBOX", "from": "p@x.com", "body": "hi"},
                {"uid": 8, "mailbox": "Sent", "from": "p@x.com", "body": "out"}]"#,
        )
        .unwrap();
        // No premium apps in a fresh store → senderPremium false; uid 7 unhandled.
        let resp = handle(
            &state,
            &Method::GET,
            "/api/team/inbox",
            &bearer("tok-mitch"),
            &[],
            "",
        )
        .await
        .unwrap();
        let v = body_json(resp).await;
        let msgs = v.get("messages").and_then(Value::as_array).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].get("handled"), Some(&json!(false)));
        assert_eq!(msgs[0].get("senderPremium"), Some(&json!(false)));
        assert_eq!(msgs[0].get("uid"), Some(&json!(7)));
        // Mark handled → reflected on the next fetch.
        let resp = handle(
            &state,
            &Method::POST,
            "/api/team/handled",
            &bearer("tok-mitch"),
            &body(json!({ "uid": "7", "handled": true })),
            "",
        )
        .await
        .unwrap();
        assert_eq!(code(&resp), 200);
        let resp = handle(
            &state,
            &Method::GET,
            "/api/team/inbox",
            &bearer("tok-mitch"),
            &[],
            "",
        )
        .await
        .unwrap();
        let msgs = body_json(resp)
            .await
            .get("messages")
            .and_then(Value::as_array)
            .unwrap()
            .to_vec();
        assert_eq!(msgs[0].get("handled"), Some(&json!(true)));
        // Unmarking deletes the key (server.js:23056).
        let resp = handle(
            &state,
            &Method::POST,
            "/api/team/handled",
            &bearer("tok-mitch"),
            &body(json!({ "uid": "7", "handled": false })),
            "",
        )
        .await
        .unwrap();
        assert_eq!(code(&resp), 200);
        let resp = handle(
            &state,
            &Method::GET,
            "/api/team/inbox",
            &bearer("tok-mitch"),
            &[],
            "",
        )
        .await
        .unwrap();
        let msgs = body_json(resp)
            .await
            .get("messages")
            .and_then(Value::as_array)
            .unwrap()
            .to_vec();
        assert_eq!(msgs[0].get("handled"), Some(&json!(false)));
    }

    #[tokio::test]
    async fn email_lookup_by_uid() {
        let (state, dir) = test_state();
        seed_tokens(&dir);
        std::fs::write(
            dir.join("data/team_inbox_cache.json"),
            r#"[{"uid": 7, "mailbox": "INBOX", "body": "in"},
                {"uid": 7, "mailbox": "Sent", "body": "out"}]"#,
        )
        .unwrap();
        // mailbox must be INBOX — the Sent copy with the same uid doesn't match.
        let resp = handle(
            &state,
            &Method::GET,
            "/api/team/email",
            &bearer("tok-mitch"),
            &[],
            "uid=7",
        )
        .await
        .unwrap();
        assert_eq!(body_json(resp).await.get("body"), Some(&json!("in")));
        // 400 without uid, 404 unknown.
        let resp = handle(
            &state,
            &Method::GET,
            "/api/team/email",
            &bearer("tok-mitch"),
            &[],
            "",
        )
        .await
        .unwrap();
        assert_eq!(code(&resp), 400);
        let resp = handle(
            &state,
            &Method::GET,
            "/api/team/email",
            &bearer("tok-mitch"),
            &[],
            "uid=99",
        )
        .await
        .unwrap();
        assert_eq!(code(&resp), 404);
    }

    #[tokio::test]
    async fn reply_validates_and_reports_send_failure() {
        let (state, dir) = test_state();
        seed_tokens(&dir);
        let h = bearer("tok-mitch");
        // 400 on missing fields.
        let resp = handle(
            &state,
            &Method::POST,
            "/api/team/reply",
            &h,
            &body(json!({"to": "x@y.z"})),
            "",
        )
        .await
        .unwrap();
        assert_eq!(code(&resp), 400);
        // 400 bad json.
        let resp = handle(
            &state,
            &Method::POST,
            "/api/team/reply",
            &h,
            b"not json",
            "",
        )
        .await
        .unwrap();
        assert_eq!(code(&resp), 400);
        // A real HTTP call: nothing listens on MAIL_RS_URL in tests →
        // mitch-mail unreachable → 500 with an error payload.
        let resp = handle(
            &state,
            &Method::POST,
            "/api/team/reply",
            &h,
            &body(json!({"to": "a@b.c", "subject": "s", "body": "b"})),
            "",
        )
        .await
        .unwrap();
        assert_eq!(code(&resp), 500);
        assert!(body_json(resp).await.get("error").is_some());
        let _ = dir;
    }

    #[tokio::test]
    async fn unsubscribe_writes_sorted_unique_disk_list() {
        let (state, dir) = test_state();
        seed_tokens(&dir);
        let h = bearer("tok-mitch");
        let resp = handle(
            &state,
            &Method::POST,
            "/api/team/unsubscribe",
            &h,
            &body(json!({ "email": " B@C.com " })),
            "",
        )
        .await
        .unwrap();
        assert_eq!(code(&resp), 200);
        // 400: no @.
        let resp = handle(
            &state,
            &Method::POST,
            "/api/team/unsubscribe",
            &h,
            &body(json!({"email": "nope"})),
            "",
        )
        .await
        .unwrap();
        assert_eq!(code(&resp), 400);
        // Second, lower-sorting address — the disk dump must be sorted/unique
        // with 2-space indent (JS writeFileSync parity).
        let resp = handle(
            &state,
            &Method::POST,
            "/api/team/unsubscribe",
            &h,
            &body(json!({ "email": "a@b.c" })),
            "",
        )
        .await
        .unwrap();
        assert_eq!(code(&resp), 200);
        let raw = std::fs::read_to_string(dir.join("data/newsletter_unsub.json")).unwrap();
        assert_eq!(raw.trim(), "[\n  \"a@b.c\",\n  \"b@c.com\"\n]");
    }

    #[tokio::test]
    async fn gmail_toggle_flips_marker() {
        let (state, dir) = test_state();
        seed_tokens(&dir);
        let h = bearer("tok-mitch");
        let resp = handle(&state, &Method::POST, "/api/team/gmail-toggle", &h, &[], "")
            .await
            .unwrap();
        assert_eq!(body_json(resp).await.get("paused"), Some(&json!(true)));
        assert!(gmail_pause_file(&state).exists());
        let resp = handle(&state, &Method::POST, "/api/team/gmail-toggle", &h, &[], "")
            .await
            .unwrap();
        assert_eq!(body_json(resp).await.get("paused"), Some(&json!(false)));
        assert!(!gmail_pause_file(&state).exists());
        let _ = dir;
    }

    #[tokio::test]
    async fn gmail_reply_appends_under_thread_key() {
        let (state, dir) = test_state();
        seed_tokens(&dir);
        let h = bearer("tok-mitch");
        // Missing fields → 400 (no send attempted).
        let resp = handle(
            &state,
            &Method::POST,
            "/api/team/gmail-reply",
            &h,
            &body(json!({"to": "x"})),
            "",
        )
        .await
        .unwrap();
        assert_eq!(code(&resp), 400);
        // Nothing listens on MAIL_RS_URL in tests → mitch-mail unreachable → 500.
        let resp = handle(
            &state,
            &Method::POST,
            "/api/team/gmail-reply",
            &h,
            &body(json!({"to": "a@b.c", "subject": "s", "body": "b", "threadKey": "t1"})),
            "",
        )
        .await
        .unwrap();
        assert_eq!(code(&resp), 500);
        // After a FAILURE the sent doc must be untouched (JS returns before
        // the append).
        let sent = state
            .store
            .read_document(&gmail_sent_file(&state), json!({}));
        assert_eq!(sent, json!({}));
        let _ = dir;
    }

    #[tokio::test]
    async fn ai_gates() {
        let (state, dir) = test_state();
        seed_tokens(&dir);
        let h = bearer("tok-mitch");
        // Bad json → 400 before the key check.
        let resp = handle(&state, &Method::POST, "/api/team/ai", &h, b"nope", "")
            .await
            .unwrap();
        assert_eq!(code(&resp), 400);
        // 503 AI not configured when GROQ_API_KEY is absent; when present,
        // the missing-fields check fires before any network call.
        let configured = std::env::var("GROQ_API_KEY")
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false);
        let resp = handle(
            &state,
            &Method::POST,
            "/api/team/ai",
            &h,
            &body(json!({})),
            "",
        )
        .await
        .unwrap();
        if configured {
            assert_eq!(code(&resp), 400);
        } else {
            assert_eq!(code(&resp), 503);
            let resp = handle(
                &state,
                &Method::POST,
                "/api/team/ai",
                &h,
                &body(json!({"mode": "review", "emailBody": "x"})),
                "",
            )
            .await
            .unwrap();
            assert_eq!(code(&resp), 503);
        }
        let _ = dir;
    }

    #[test]
    fn thread_key_strips_one_prefix() {
        assert_eq!(thread_key("RE: Hello"), "hello");
        assert_eq!(thread_key("FWD:  Subject X"), "subject x");
        assert_eq!(thread_key("Fw: y"), "y");
        assert_eq!(thread_key("re:re: nested"), "re: nested");
        assert_eq!(thread_key("Plain"), "plain");
        assert_eq!(thread_key(""), "");
    }
}
