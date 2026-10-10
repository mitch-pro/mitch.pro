//! `/api/dm/send` (server.js:19452-19670) — the full send ladder: rate limit,
//! auth, chat byte caps, image validation, E2E envelope validation,
//! clientId idempotency, at-rest sealing, notification fan-out, coin reward.
//!
//! Known Step-11-batch-1 divergences (documented in the scorecard):
//! - The WebSocket `new_dm` broadcast is deferred to the `/ws` batch.
//! - `e2eUsers` (the in-memory E2E presence map fed by /api/e2e/*) lives with
//!   the `/ws` batch too, so `recActive` is always false here — invisible in
//!   HTTP responses; it only gates which notification channel fans out.

use super::is_revoked_id;
use super::js_num_value;
use super::notif::{notif_allowed, notification_url, ntfy_notify_user};
use crate::handler::get_real_ip;
use crate::hosts::is_pickle_host;
use crate::routes::me::{
    cookies_of, data_file, dm_content_parts, json_response, parse_body_strict,
};
use crate::state::AppState;
use axum::http::HeaderMap;
use axum::response::Response;
use mitch_lib::auth::{self, encode_uri_component};
use mitch_lib::coins;
use mitch_lib::crypto;
use mitch_lib::dm::{self, DMS_MAIN, DMS_PICKLE};
use mitch_lib::jsval::{self, truthy};
use mitch_lib::profile::{canonical_delivery_email, display_email};
use mitch_lib::school::now_millis;
use serde_json::{json, Map, Value};
use std::sync::Arc;

/// What the branch stage hands to the post-save fan-out stage.
enum Fanout {
    Group {
        members: Vec<Value>,
        group_name: String,
    },
    Dm {
        to_canonical: String,
    },
}

pub(crate) async fn handle(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Option<Response> {
    // checkRateLimit(req, path) — getIdKey reads the same cookies the auth
    // check below reads.
    let ip = get_real_ip(headers, None);
    let cookies = cookies_of(state, headers);
    let sid = cookies.get("studentId").unwrap_or("");
    let sid = if sid.is_empty() {
        cookies.get("id").unwrap_or("")
    } else {
        sid
    };
    let id_key = if auth::valid_id(sid, &state.id_secret) {
        format!("id:{sid}")
    } else {
        "anon".to_string()
    };
    if let Some((code, msg)) = state.rate_limit_check(&ip, &id_key, "/api/dm/send") {
        return Some(json_response(code, json!({ "error": msg })));
    }
    if sid.is_empty() || !auth::valid_id(sid, &state.id_secret) || is_revoked_id(state, sid) {
        return Some(json_response(401, json!({ "error": "auth required" })));
    }
    let sender_email = auth::names_get_email(&state.store, sid)
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    if sender_email.is_empty() {
        return Some(json_response(403, json!({ "error": "email not found" })));
    }
    let store = if is_pickle_host(headers) {
        &DMS_PICKLE
    } else {
        &DMS_MAIN
    };
    let exp_pfx = if store.pickle { "spc:" } else { "" };
    // Push deep-links must open the app the message was sent from.
    let chat_app_url = |p: &str| -> String {
        if store.pickle {
            format!("https://sexypickleclub.com/cellar/{p}")
        } else {
            format!("/encrypt/{p}")
        }
    };
    // declaredChatBytes — `Number(Content-Length || 0)`; a garbage header is
    // NaN in JS and `NaN > cap` is false, so default to NaN here too.
    let declared = headers
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .unwrap_or(f64::NAN);
    if declared > dm::max_chat_json_body_bytes() as f64 {
        return Some(json_response(
            413,
            json!({ "error": "message request too large" }),
        ));
    }
    if body_bytes.len() > dm::max_chat_json_body_bytes() {
        return Some(json_response(400, json!({ "error": "bad json" })));
    }
    let Some(body) = parse_body_strict(body_bytes) else {
        return Some(json_response(400, json!({ "error": "bad json" })));
    };

    let group_id = match body.get("groupId") {
        Some(v) if truthy(v) => jsval::string(v),
        _ => String::new(),
    };
    let idx = dm::address_index_cached(&state.store, &state.cfg.data_dir);
    let to = if group_id.is_empty() {
        let raw = jsval::string(&jsval::or(body.get("to"), json!("")));
        let resolved = dm::resolve_member_ref(&idx, &raw);
        if resolved.is_empty() {
            auth::normalize_email(&raw)
        } else {
            resolved
        }
    } else {
        String::new()
    };
    let raw_text = jsval::string(&jsval::or(body.get("text"), json!("")))
        .trim()
        .to_string();

    // Image validation — {url+id} passthrough or inline data URL.
    let image_raw = body.get("image").cloned().unwrap_or(Value::Null);
    let safe_image = match validate_image(&image_raw) {
        Ok(img) => img,
        Err((code, msg)) => return Some(json_response(code, json!({ "error": msg }))),
    };

    // An E2E envelope is a JSON-parseable text body with e2e === true.
    let encrypted_envelope = serde_json::from_str::<Value>(&raw_text)
        .ok()
        .filter(|c| c.is_object() && c.get("e2e") == Some(&Value::Bool(true)));
    if encrypted_envelope.is_some() {
        if let Err((status, msg)) = dm::validate_e2e_envelope(&raw_text, !group_id.is_empty()) {
            return Some(json_response(status, json!({ "error": msg })));
        }
    }
    let text = if encrypted_envelope.is_some() {
        raw_text.clone()
    } else {
        jsval::js_slice_utf16(&raw_text, if safe_image.is_some() { 500 } else { 2000 })
    };
    if group_id.is_empty() && to.is_empty() {
        return Some(json_response(400, json!({ "error": "missing fields" })));
    }
    if text.is_empty() && safe_image.is_none() {
        return Some(json_response(400, json!({ "error": "missing fields" })));
    }

    // clientId is an idempotency key: strip disallowed chars, cap 120.
    let client_id = {
        let raw = jsval::string(&jsval::or(body.get("clientId"), json!("")));
        let filtered: String = raw
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
            .collect();
        jsval::js_slice_utf16(&filtered, 120)
    };
    let reply_to = match body.get("replyTo") {
        Some(rt) if truthy(rt) => match rt.as_object() {
            Some(o) => {
                let sliced = |k: &str, n: usize| {
                    jsval::js_slice_utf16(&jsval::string(&jsval::or(o.get(k), json!(""))), n)
                };
                let ts = jsval::number(o.get("ts").unwrap_or(&Value::Null)).unwrap_or(f64::NAN);
                Some(json!({
                    "id": sliced("id", 100),
                    "from": sliced("from", 100),
                    "text": sliced("text", 200),
                    "ts": if ts.is_nan() || ts == 0.0 { json!(0) } else { js_num_value(ts) },
                }))
            }
            None => None,
        },
        _ => None,
    };

    let mut dms: Vec<Value> = state
        .store
        .read_document(&data_file(state, store.dms), json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    // A retry after a slow/lost HTTP response must return the stored message
    // instead of creating a copy.
    if !client_id.is_empty() {
        let now = now_millis();
        let sender_norm = auth::normalize_email(&sender_email);
        let existing = dms.iter().rev().find(|m| {
            if m.get("clientId").and_then(|v| v.as_str()) != Some(client_id.as_str()) {
                return false;
            }
            // expiresAt && Date.now() > expiresAt
            if let Some(exp_v) = m.get("expiresAt").filter(|v| truthy(v)) {
                let exp = jsval::number(exp_v).unwrap_or(f64::NAN);
                if !exp.is_nan() && now as f64 > exp {
                    return false;
                }
            }
            if auth::normalize_email(&jsval::string(&jsval::or(m.get("from"), json!(""))))
                != sender_norm
            {
                return false;
            }
            if !group_id.is_empty() {
                m.get("kind").and_then(|v| v.as_str()) == Some("group")
                    && jsval::string(&jsval::or(m.get("groupId"), json!(""))) == group_id
            } else {
                m.get("kind").and_then(|v| v.as_str()) != Some("group")
                    && auth::normalize_email(&jsval::string(&jsval::or(m.get("to"), json!(""))))
                        == auth::normalize_email(&to)
            }
        });
        if let Some(existing) = existing {
            return Some(dup_response(state, existing));
        }
    }

    let vapid_public = std::env::var("VAPID_PUBLIC_KEY")
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    // VAPID_PUBLIC ? loadPushSubscriptions() : {} — normalized keys.
    let subs = if vapid_public.is_empty() {
        json!({})
    } else {
        load_push_subscriptions(state)
    };
    let requested_expiry =
        jsval::number(body.get("expiry").unwrap_or(&Value::Null)).unwrap_or(f64::NAN);
    let requested_expiry = if requested_expiry.is_nan() || requested_expiry == 0.0 {
        0.0
    } else {
        requested_expiry
    };
    let expiry = if dm::CHAT_EXPIRY_OPTIONS.contains(&requested_expiry) {
        requested_expiry
    } else {
        0.0
    };
    // The conversation-wide setting always wins; the sender's per-send value
    // is only a fallback for clients that haven't synced the setting yet.
    let conv_expiry_key = format!(
        "{exp_pfx}{}",
        if group_id.is_empty() {
            dm::dm_expiry_key(&sender_email, &to)
        } else {
            dm::group_expiry_key(&group_id)
        }
    );
    let conv_expiry = dm::get_chat_expiry(&state.store, &state.cfg.data_dir, &conv_expiry_key);
    let effective_expiry = if conv_expiry != 0.0 {
        conv_expiry
    } else {
        expiry
    };
    let now = now_millis();
    let sender_norm = auth::normalize_email(&sender_email);
    let sender_canonical = canonical_delivery_email(
        &state.store,
        &state.cfg.data_dir,
        &state.id_secret,
        &sender_email,
    );
    let profiles = state
        .store
        .read_document(&data_file(state, "profiles.json"), json!({}));
    let prof = profiles
        .get(sender_norm.as_str())
        .cloned()
        .unwrap_or(json!({}));
    let sender_display = {
        // displayName || nickname || username || canonical local-part
        let pick = ["displayName", "nickname", "username"]
            .iter()
            .find_map(|k| prof.get(*k).filter(|v| truthy(v)).cloned())
            .unwrap_or_else(|| json!(sender_canonical.split('@').next().unwrap_or("")));
        jsval::string(&pick)
    };
    // `[Secure Message]` / `[Secure Message: Attachment]` — pre-sealing values.
    let notify_body = if safe_image.is_some() && encrypted_envelope.is_none() {
        "[Secure Message: Attachment]"
    } else {
        "[Secure Message]"
    };
    let dm_at_rest_key =
        crypto::hmac_sha256(&state.id_secret, crypto::DM_AT_REST_PURPOSE.as_bytes());

    let is_group = !group_id.is_empty();
    let (mut msg, fanout) = if is_group {
        let groups = state
            .store
            .read_document(&data_file(state, store.groups), json!([]));
        let groups_arr = groups.as_array().cloned().unwrap_or_default();
        let Some(group) = groups_arr
            .iter()
            .find(|g| g.get("id") == Some(&Value::String(group_id.clone())))
        else {
            return Some(json_response(404, json!({ "error": "group not found" })));
        };
        let members = group
            .get("members")
            .cloned()
            .unwrap_or(json!([]))
            .as_array()
            .cloned()
            .unwrap_or_default();
        let member_norms: Vec<String> = members
            .iter()
            .map(|m| {
                let raw = jsval::string(&jsval::or(Some(m), json!("")));
                let resolved = dm::resolve_member_ref(&idx, &raw);
                if resolved.is_empty() {
                    auth::normalize_email(&raw)
                } else {
                    resolved
                }
            })
            .collect();
        if !member_norms.contains(&sender_norm) {
            return Some(json_response(403, json!({ "error": "not a member" })));
        }
        let mut m = Map::new();
        m.insert("kind".into(), json!("group"));
        m.insert("groupId".into(), json!(group_id));
        // groupName: group.name — a missing name drops the key (undefined).
        if let Some(gn) = group.get("name") {
            m.insert("groupName".into(), gn.clone());
        }
        m.insert("from".into(), json!(sender_email));
        m.insert("text".into(), json!(text));
        m.insert("image".into(), safe_image.clone().unwrap_or(Value::Null));
        m.insert("replyTo".into(), reply_to.clone().unwrap_or(Value::Null));
        m.insert("ts".into(), json!(now));
        m.insert("readBy".into(), json!([sender_email]));
        let group_name = group
            .get("name")
            .map(jsval::string)
            .unwrap_or_else(|| "undefined".to_string());
        (
            m,
            Fanout::Group {
                members,
                group_name,
            },
        )
    } else {
        let to_canonical = {
            let resolved = dm::resolve_member_ref(&idx, &to);
            if resolved.is_empty() {
                auth::normalize_email(&to)
            } else {
                resolved
            }
        };
        let mut m = Map::new();
        m.insert("kind".into(), json!("dm"));
        m.insert("from".into(), json!(sender_email));
        m.insert(
            "to".into(),
            json!(if to_canonical.is_empty() {
                &to
            } else {
                &to_canonical
            }),
        );
        m.insert("text".into(), json!(text));
        m.insert("image".into(), safe_image.clone().unwrap_or(Value::Null));
        m.insert("replyTo".into(), reply_to.clone().unwrap_or(Value::Null));
        m.insert("ts".into(), json!(now));
        m.insert("read".into(), json!(false));
        (m, Fanout::Dm { to_canonical })
    };

    // Non-E2E messages never touch disk in the clear: seal text+image at rest.
    if encrypted_envelope.is_none() {
        let payload = json!({
            "text": msg.get("text").cloned().unwrap_or(Value::Null),
            "image": msg.get("image").cloned().unwrap_or(Value::Null),
        });
        if let Some(sealed) = crypto::seal_at_rest(
            &dm_at_rest_key,
            &payload.to_string(),
            crypto::DM_AT_REST_PREFIX,
        ) {
            msg.insert("text".into(), json!(sealed));
            msg.insert("image".into(), json!(null));
        }
    }
    if !client_id.is_empty() {
        msg.insert("clientId".into(), json!(client_id));
    }
    if effective_expiry > 0.0 {
        msg.insert("autoDelete".into(), js_num_value(effective_expiry));
    }
    dms.push(Value::Object(msg.clone()));
    let pruned = dm::prune_dms(
        &state.store,
        &state.cfg.data_dir,
        &dms,
        store.pickle,
        now as f64,
    );
    state
        .store
        .write_document(&data_file(state, store.dms), &Value::Array(pruned))
        .ok();

    match &fanout {
        Fanout::Group {
            members,
            group_name,
        } => {
            let group_url = notification_url(&chat_app_url(&format!(
                "?group={}",
                encode_uri_component(&group_id)
            )));
            for member in members.iter() {
                let member_norm =
                    auth::normalize_email(&jsval::string(&jsval::or(Some(member), json!(""))));
                if member_norm == sender_norm {
                    continue;
                }
                // recActive comes from e2eUsers (the /ws batch) — always false here.
                if !notif_allowed(state, &member_norm, "group") {
                    continue;
                }
                if truthy(&jsval::or(subs.get(member_norm.as_str()), json!(null))) {
                    let payload = json!({
                        "title": format!("{sender_display} in {group_name}"),
                        "body": notify_body,
                        "url": group_url,
                        "tag": format!("group-{group_id}-{now}"),
                    });
                    crate::routes::push::send_web_push_clean(state, &member_norm, &payload).await;
                } else {
                    ntfy_notify_user(
                        state,
                        &member_norm,
                        &format!("{sender_display} in {group_name}"),
                        notify_body,
                        &group_url,
                    );
                }
            }
        }
        Fanout::Dm { to_canonical } => {
            // recActive comes from e2eUsers (the /ws batch) — always false here.
            if notif_allowed(state, to_canonical, "dm") {
                let dm_url = notification_url(&chat_app_url(&format!(
                    "?to={}",
                    encode_uri_component(&sender_canonical)
                )));
                if !vapid_public.is_empty()
                    && truthy(&jsval::or(subs.get(to_canonical.as_str()), json!(null)))
                {
                    let payload = json!({
                        "title": format!("Message from {sender_display}"),
                        "body": notify_body,
                        "url": dm_url,
                        "tag": format!("dm-{now}"),
                    });
                    crate::routes::push::send_web_push_clean(state, to_canonical, &payload).await;
                } else {
                    ntfy_notify_user(
                        state,
                        to_canonical,
                        &format!("Message from {sender_display}"),
                        notify_body,
                        &dm_url,
                    );
                }
            }
        }
    }

    coins::add_coins(
        &state.store,
        &state.cfg.data_dir,
        &sender_email,
        2.0,
        state.coin_multiplier(),
        "",
    );

    // The WebSocket `new_dm` broadcast (server.js:19600-19642) — recipients
    // are the group roster, or for a DM the sender + resolved peer (the JS
    // username leg never matches: broadcast sockets carry no username).
    let mut ws_recipients: std::collections::HashSet<String> = std::collections::HashSet::new();
    ws_recipients.insert(sender_norm.clone());
    match &fanout {
        Fanout::Group { members, .. } => {
            ws_recipients.extend(members.iter().map(|m| {
                let raw = jsval::string(&jsval::or(Some(m), json!("")));
                let resolved = dm::resolve_member_ref(&idx, &raw);
                if resolved.is_empty() {
                    auth::normalize_email(&raw)
                } else {
                    resolved
                }
            }));
        }
        Fanout::Dm { to_canonical } => {
            ws_recipients.insert(if to_canonical.is_empty() {
                auth::normalize_email(&to)
            } else {
                to_canonical.clone()
            });
        }
    }
    let masked = masked_msg(state, &Value::Object(msg));
    crate::ws::broadcast(
        state,
        crate::ws::WsRecipients::Emails(ws_recipients),
        json!({ "type": "new_dm", "message": masked }).to_string(),
    );
    Some(json_response(
        200,
        json!({ "success": true, "message": masked }),
    ))
}

/// `{...existing, text, image, from, to, readBy}` for a clientId retry —
/// 200 `{success:true, duplicate:true, message}`.
fn dup_response(state: &Arc<AppState>, existing: &Value) -> Response {
    let (wire_text, wire_image) = dm_content_parts(existing, &state.id_secret);
    let mut msg = existing.as_object().cloned().unwrap_or_default();
    if let Some(t) = wire_text {
        msg.insert("text".into(), t);
    }
    // `dupContent.image !== undefined ? dupContent.image : existing.image` —
    // when both are absent the spread never gains the key.
    if let Some(img) = wire_image {
        msg.insert("image".into(), img);
    }
    msg.insert(
        "from".into(),
        json!(display_email(
            &state.store,
            &state.cfg.data_dir,
            &state.id_secret,
            &jsval::string(&jsval::or(existing.get("from"), json!("")))
        )),
    );
    // `to: existing.to ? displayEmail(existing.to) : undefined` — falsy drops.
    if let Some(to_v) = existing.get("to").filter(|v| truthy(v)) {
        msg.insert(
            "to".into(),
            json!(display_email(
                &state.store,
                &state.cfg.data_dir,
                &state.id_secret,
                &jsval::string(to_v)
            )),
        );
    }
    msg.insert("readBy".into(), json!(display_read_by(state, existing)));
    json_response(
        200,
        json!({ "success": true, "duplicate": true, "message": Value::Object(msg) }),
    )
}

/// `{...msg, text, image, from, to, readBy}` — the wire shape with at-rest
/// rows unsealed and emails display-mapped.
fn masked_msg(state: &Arc<AppState>, msg: &Value) -> Value {
    let (wire_text, wire_image) = dm_content_parts(msg, &state.id_secret);
    let mut masked = msg.as_object().cloned().unwrap_or_default();
    if let Some(t) = wire_text {
        masked.insert("text".into(), t);
    }
    if let Some(img) = wire_image {
        masked.insert("image".into(), img);
    }
    masked.insert(
        "from".into(),
        json!(display_email(
            &state.store,
            &state.cfg.data_dir,
            &state.id_secret,
            &jsval::string(&jsval::or(msg.get("from"), json!("")))
        )),
    );
    // `to: displayEmail(msg.to)` — group messages have no `to`, and in JS the
    // resulting `undefined` value is dropped by JSON.stringify.
    if let Some(to_v) = msg.get("to").filter(|v| truthy(v)) {
        masked.insert(
            "to".into(),
            json!(display_email(
                &state.store,
                &state.cfg.data_dir,
                &state.id_secret,
                &jsval::string(to_v)
            )),
        );
    }
    masked.insert("readBy".into(), json!(display_read_by(state, msg)));
    Value::Object(masked)
}

/// `(msg.readBy || []).map(displayEmail)` — a truthy non-array readBy would
/// throw in JS (500); here it degrades to an empty list.
fn display_read_by(state: &Arc<AppState>, msg: &Value) -> Vec<Value> {
    let read_by = msg.get("readBy").cloned().unwrap_or(json!([]));
    let Some(entries) = read_by.as_array() else {
        return Vec::new();
    };
    entries
        .iter()
        .map(|v| {
            json!(display_email(
                &state.store,
                &state.cfg.data_dir,
                &state.id_secret,
                &jsval::string(v)
            ))
        })
        .collect()
}

/// `body.image && typeof body.image === 'object'` — `Ok(None)` for the
/// falsy/non-object case (arrays fall in too and yield None in JS).
/// `Err((status, error))` mirrors the three early `jsonResp`s.
fn validate_image(image_raw: &Value) -> Result<Option<Value>, (u16, &'static str)> {
    let Some(obj) = image_raw.as_object().filter(|_| truthy(image_raw)) else {
        return Ok(None);
    };
    let has_url_id = truthy(&jsval::or(obj.get("url"), json!("")))
        && truthy(&jsval::or(obj.get("id"), json!("")));
    if has_url_id {
        let image_id = jsval::string(&jsval::or(obj.get("id"), json!("")));
        let image_url = jsval::string(&jsval::or(obj.get("url"), json!("")));
        let name = jsval::string(&jsval::or(obj.get("name"), json!("attachment")));
        let size = jsval::number(obj.get("size").unwrap_or(&Value::Null)).unwrap_or(f64::NAN);
        let size = if size.is_nan() || size == 0.0 {
            0.0
        } else {
            size
        };
        let mime = jsval::string(&jsval::or(
            obj.get("mime"),
            json!("application/octet-stream"),
        ));
        return Ok(Some(json!({
            "id": jsval::js_slice_utf16(&image_id, 64),
            "url": jsval::js_slice_utf16(&image_url, 250),
            "name": jsval::js_slice_utf16(&name, 180),
            "size": js_num_value(size),
            "mime": jsval::js_slice_utf16(&mime, 80),
        })));
    }
    if truthy(&jsval::or(obj.get("data"), json!(""))) {
        let data = jsval::string(&jsval::or(obj.get("data"), json!("")));
        let mime = jsval::string(&jsval::or(obj.get("mime"), json!(""))).to_lowercase();
        let name = jsval::js_slice_utf16(
            &jsval::string(&jsval::or(obj.get("name"), json!("image"))),
            80,
        );
        // /^image\/(png|jpe?g|gif|webp)$/
        let mime_ok = mime
            .strip_prefix("image/")
            .is_some_and(|ext| matches!(ext, "png" | "jpg" | "jpeg" | "gif" | "webp"));
        if !mime_ok {
            return Err((400, "unsupported image type"));
        }
        if !data.starts_with(&format!("data:{mime};base64,")) {
            return Err((400, "invalid image data"));
        }
        // Buffer.byteLength(data, 'utf8')
        if data.len() > 900_000 {
            return Err((413, "image too large"));
        }
        return Ok(Some(json!({ "data": data, "mime": mime, "name": name })));
    }
    Ok(None)
}

/// `loadPushSubscriptions()` (server.js:2154) — push_subs.json with
/// normalized keys; only truthy object subscriptions survive.
fn load_push_subscriptions(state: &AppState) -> Value {
    let stored = state
        .store
        .read_document(&data_file(state, "push_subs.json"), json!({}));
    let mut normalized = Map::new();
    if let Some(obj) = stored.as_object() {
        for (k, v) in obj {
            let key = auth::normalize_email(k);
            if !key.is_empty() && truthy(v) && v.is_object() {
                normalized.insert(key, v.clone());
            }
        }
    }
    Value::Object(normalized)
}
