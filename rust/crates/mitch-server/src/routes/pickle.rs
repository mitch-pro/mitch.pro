//! `/api/pickle-*` — the Sexy Pickle Club (sexypickleclub.com) route group
//! (plan Step 9 batch 5). 16 endpoints over The Barrel (chat), the Club
//! Bulletin, and the membership status layer:
//! - pickle-chat: send (16605), react (16693), presence (16733), history (23955)
//! - pickle-club: vote (16750), crunch (16771), join (16791), decide (16816),
//!   owners (16853), membership GET (17776), applicants GET (17795),
//!   features GET (23992)
//! - pickle-bulletin: post (16895), react (16931), delete (16968), state GET (17852)
//!
//! The founder set and membership helpers mirror server.js:244-312. The
//! `isBroadcast` WS fan-outs inside send/react/post/delete land with the
//! Step 11 WebSocket work (like friends' presence leg) — every HTTP response
//! is complete today.

use super::me::{cookies_of, data_file, json_response, me_uid, parse_body_strict};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::admin::{log_admin_action, log_cheat, mask_email};
use mitch_lib::auth;
use mitch_lib::blog;
use mitch_lib::crypto::random_bytes_hex;
use mitch_lib::jsval;
use mitch_lib::profile;
use serde_json::{json, Map, Value};
use std::sync::Arc;

/// `PICKLE_FOUNDERS` (server.js:251) — insertion order fixes the Jar numbers.
const PICKLE_FOUNDERS: [&str; 3] = [
    "mitch@student.rjuhsd.us",
    "lochlann@student.rjuhsd.us",
    "admin@mitch.pro",
];

pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Option<Response> {
    match (method.as_str(), path) {
        ("POST", "/api/pickle-chat/send") => Some(chat_send(state, headers, body_bytes).await),
        ("POST", "/api/pickle-chat/react") => Some(chat_react(state, headers, body_bytes)),
        ("POST", "/api/pickle-chat/presence") => Some(chat_presence(state, headers)),
        (_, "/api/pickle-chat/history") => Some(chat_history(state, headers)),
        ("POST", "/api/pickle-club/vote") => Some(club_vote(state, headers, body_bytes)),
        ("POST", "/api/pickle-club/crunch") => Some(club_crunch(state, headers, body_bytes)),
        ("POST", "/api/pickle-club/join") => Some(club_join(state, headers)),
        ("POST", "/api/pickle-club/decide") => Some(club_decide(state, headers, body_bytes)),
        ("POST", "/api/pickle-club/owners") => Some(club_owners(state, headers, body_bytes)),
        ("GET", "/api/pickle-club/membership") => Some(club_membership(state, headers)),
        ("GET", "/api/pickle-club/applicants") => Some(club_applicants(state, headers)),
        ("GET", "/api/pickle-club/features") => Some(club_features(state, headers)),
        ("POST", "/api/pickle-bulletin/post") => {
            Some(bulletin_post(state, headers, body_bytes).await)
        }
        ("POST", "/api/pickle-bulletin/react") => Some(bulletin_react(state, headers, body_bytes)),
        ("POST", "/api/pickle-bulletin/delete") => {
            Some(bulletin_delete(state, headers, body_bytes))
        }
        ("GET", "/api/pickle-bulletin/state") => Some(bulletin_state(state, headers)),
        _ => None,
    }
}

// ── founder / membership layer (server.js:253-312) ──────────────────────────

/// `pickleCoOwners()`.
fn pickle_co_owners(state: &Arc<AppState>) -> Value {
    state
        .store
        .read_document(&data_file(state, "pickle_owners.json"), json!({}))
}

/// `isPickleFounderEmail(norm)`.
fn is_pickle_founder_email(state: &Arc<AppState>, norm: &str) -> bool {
    PICKLE_FOUNDERS.contains(&norm)
        || jsval::truthy(pickle_co_owners(state).get(norm).unwrap_or(&Value::Null))
}

/// `isRevoked(id)` (server.js:2769) — `id in loadRevoked()`.
fn is_revoked_id(state: &Arc<AppState>, sid: &str) -> bool {
    state
        .store
        .read_document(&data_file(state, "revoked.json"), json!({}))
        .get(sid)
        .is_some()
}

/// `isPickleFounderId(sid)` (server.js:260-264).
fn is_pickle_founder_id(state: &Arc<AppState>, sid: &str) -> bool {
    if sid.is_empty() || !auth::valid_id(sid, &state.id_secret) || is_revoked_id(state, sid) {
        return false;
    }
    match auth::email_from_sid(&state.store, &state.id_secret, sid) {
        Some(email) => is_pickle_founder_email(state, &auth::normalize_email(&email)),
        None => false,
    }
}

/// `pickleMembershipFor(normEmail)` (server.js:267-278).
fn pickle_membership_for(state: &Arc<AppState>, norm: &str) -> Option<Value> {
    if norm.is_empty() {
        return None;
    }
    if let Some(order) = PICKLE_FOUNDERS.iter().position(|f| *f == norm) {
        return Some(json!({
            "status": "approved",
            "memberNumber": order + 1,
            "founder": true,
        }));
    }
    if let Some(coowner) = pickle_co_owners(state).get(norm) {
        return Some(json!({
            "status": "approved",
            "memberNumber": jsval::or(coowner.get("memberNumber"), json!(Value::Null)),
            "coowner": true,
        }));
    }
    state
        .store
        .read_document(&data_file(state, "pickle_members.json"), json!({}))
        .get(norm)
        .cloned()
}

/// `pickleJarFor(email, membersCache)` (server.js:280-289) — Jar number or null.
fn pickle_jar_for(state: &Arc<AppState>, email: &str, members: Option<&Value>) -> Value {
    let norm = auth::normalize_email(email);
    if norm.is_empty() {
        return Value::Null;
    }
    if let Some(order) = PICKLE_FOUNDERS.iter().position(|f| *f == norm) {
        return json!(order + 1);
    }
    if let Some(coowner) = pickle_co_owners(state).get(&norm) {
        return jsval::or(coowner.get("memberNumber"), json!(Value::Null));
    }
    let rec = match members {
        Some(m) => m.get(&norm).cloned(),
        None => state
            .store
            .read_document(&data_file(state, "pickle_members.json"), json!({}))
            .get(&norm)
            .cloned(),
    };
    match rec {
        Some(r) if r.get("status").and_then(|v| v.as_str()) == Some("approved") => {
            jsval::or(r.get("memberNumber"), json!(Value::Null))
        }
        _ => Value::Null,
    }
}

/// `ensurePickleMember(sid)` (server.js:291-312) — idempotent pending
/// registration on first visit; never resurrects a rejected application.
fn ensure_pickle_member(state: &Arc<AppState>, sid: &str) -> Option<Value> {
    if sid.is_empty() || !auth::valid_id(sid, &state.id_secret) || is_revoked_id(state, sid) {
        return None;
    }
    let email = auth::email_from_sid(&state.store, &state.id_secret, sid)?;
    let norm = auth::normalize_email(&email);
    if PICKLE_FOUNDERS.contains(&norm.as_str())
        || jsval::truthy(pickle_co_owners(state).get(&norm).unwrap_or(&Value::Null))
    {
        return pickle_membership_for(state, &norm);
    }
    let file = data_file(state, "pickle_members.json");
    let mut all = state.store.read_document(&file, json!({}));
    match all.get(&norm) {
        Some(rec) => Some(rec.clone()),
        None => {
            let rec = json!({
                "status": "pending",
                "requestedAt": mitch_lib::school::now_millis(),
                "approvedAt": 0,
                "approvedBy": "",
                "memberNumber": 0,
                "note": "",
            });
            if let Some(map) = all.as_object_mut() {
                map.insert(norm.clone(), rec.clone());
            }
            let _ = state.store.write_document(&file, &all);
            Some(rec)
        }
    }
}

/// `pickleBulletinPublic(post, viewerNorm)` (server.js:17852-17872).
fn pickle_bulletin_public(post: &Value, viewer_norm: &str) -> Value {
    let mut out = Map::new();
    for key in ["id", "title", "html", "ts"] {
        if let Some(v) = post.get(key) {
            if !v.is_null() {
                out.insert(key.to_string(), v.clone());
            }
        }
    }
    let author_email = jsval::string(&jsval::or(post.get("authorEmail"), json!("")));
    out.insert(
        "author".to_string(),
        json!(jsval::str_or(
            post.get("authorName").filter(|v| jsval::truthy(v)),
            &mask_email(&author_email)
        )),
    );
    out.insert("authorEmail".to_string(), json!(mask_email(&author_email)));
    out.insert(
        "updatedAt".to_string(),
        json!(jsval::number(post.get("updatedAt").unwrap_or(&Value::Null)).unwrap_or(0.0) as i64),
    );
    out.insert(
        "pinned".to_string(),
        json!(jsval::truthy(post.get("pinned").unwrap_or(&Value::Null))),
    );
    let (reactions, my_reactions) = reactions_tally(post.get("reactions"), viewer_norm);
    out.insert("reactions".to_string(), reactions);
    out.insert("myReactions".to_string(), my_reactions);
    out.insert(
        "mine".to_string(),
        json!(!viewer_norm.is_empty() && auth::normalize_email(&author_email) == viewer_norm),
    );
    Value::Object(out)
}

/// Shared reaction walker: `{ emoji: [normEmail, ...] }` → (counts, viewer's
/// own emoji picks). The JS walks `Object.entries` in insertion order.
fn reactions_tally(reactions: Option<&Value>, viewer_norm: &str) -> (Value, Value) {
    let mut tally = Map::new();
    let mut mine = Vec::new();
    if let Some(map) = reactions.and_then(|r| r.as_object()) {
        for (emoji, arr) in map {
            let Some(list) = arr.as_array() else { continue };
            if list.is_empty() {
                continue;
            }
            tally.insert(emoji.clone(), json!(list.len()));
            if !viewer_norm.is_empty() && list.iter().any(|v| jsval::string(v) == viewer_norm) {
                mine.push(json!(emoji));
            }
        }
    }
    (Value::Object(tally), Value::Array(mine))
}

/// Shared gate: validId → 401 unauthorized, emailFromSid → 401 email not
/// found. Returns the original (unnormalized) email.
fn session_email(
    state: &Arc<AppState>,
    headers: &HeaderMap,
) -> Result<String, (u16, &'static str)> {
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    if !auth::valid_id(&sid, &state.id_secret) {
        return Err((401, "unauthorized"));
    }
    match auth::email_from_sid(&state.store, &state.id_secret, &sid) {
        Some(email) => Ok(email),
        None => Err((401, "email not found")),
    }
}

// ── The Barrel (pickle-chat) ────────────────────────────────────────────────

/// `/api/pickle-chat/send` (server.js:16605-16691).
async fn chat_send(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let email = match session_email(state, headers) {
        Ok(e) => e,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let ip = crate::handler::get_real_ip(headers, None);
    if !crate::routes::push::verify_recaptcha(
        state,
        &jsval::string(&jsval::or(body.get("recaptcha_token"), json!(""))),
        &ip,
        "",
    )
    .await
    {
        return json_response(
            400,
            json!({ "error": "reCAPTCHA failed. Please try again." }),
        );
    }
    let text = jsval::js_slice_utf16(
        jsval::string(&jsval::or(body.get("text"), json!(""))).trim(),
        1000,
    );
    if text.is_empty() {
        return json_response(400, json!({ "error": "empty message" }));
    }
    let norm_email = auth::normalize_email(&email);

    // Bans/timeouts live under their own room key — the Barrel bans nobody
    // from the Plaza and vice versa (server.js:16634-16651).
    let ban_file = data_file(state, "chatroom_bans.json");
    let mut chatroom_bans = state.store.read_document(&ban_file, json!({}));
    let now = mitch_lib::school::now_millis();
    let ban_entry = chatroom_bans
        .get("pickle")
        .and_then(|b| b.get(&norm_email))
        .cloned();
    if let Some(ban_entry) = ban_entry {
        match ban_entry.get("type").and_then(|v| v.as_str()) {
            Some("ban") => {
                return json_response(403, json!({ "error": "You are banned from The Barrel." }));
            }
            _ => {
                let expires = ban_entry.get("expires").and_then(|v| v.as_f64());
                let expired = match expires {
                    Some(exp) => (now as f64) >= exp,
                    None => true, // `now < undefined` → false → cleanup branch
                };
                if !expired {
                    let seconds = ((expires.unwrap_or(0.0) - now as f64) / 1000.0).ceil() as i64;
                    return json_response(
                        403,
                        json!({ "error": format!("You are timed out from The Barrel for another {seconds} seconds.") }),
                    );
                }
                // Timeout expired — clean it up.
                if let Some(map) = chatroom_bans.as_object_mut() {
                    if let Some(barrel) = map.get_mut("pickle").and_then(|b| b.as_object_mut()) {
                        barrel.remove(&norm_email);
                    }
                }
                let _ = state.store.write_document(&ban_file, &chatroom_bans);
            }
        }
    }

    // Last-10-consecutive spam limit (server.js:16653-16661).
    let chat_file = data_file(state, "pickle_chat.json");
    let history = state.store.read_document(&chat_file, json!([]));
    if let Some(arr) = history.as_array() {
        if arr.len() >= 10 {
            let last10_all_mine = arr[arr.len() - 10..].iter().all(|m| {
                auth::normalize_email(&jsval::string(&jsval::or(m.get("email"), json!(""))))
                    == norm_email
            });
            if last10_all_mine {
                log_cheat(
                    &state.store,
                    state.data_dir(),
                    &email,
                    "chat_spam",
                    "User sent 10 consecutive messages in pickle_chat",
                    &ip,
                );
                return json_response(
                    400,
                    json!({ "error": "Spam detected. The last 10 messages in The Barrel are yours." }),
                );
            }
        }
    }

    let p = mitch_lib::profiles::profile_get(&state.store, &norm_email);
    let name = jsval::str_or(
        p.get("displayName").filter(|v| jsval::truthy(v)),
        email.split('@').next().unwrap_or(""),
    );
    let user_cosm = mitch_lib::shop::cosmetics_get(&state.store, &norm_email).unwrap_or(json!({}));
    let mut msg = Map::new();
    msg.insert("id".to_string(), json!(random_bytes_hex(8)));
    msg.insert("name".to_string(), json!(name));
    msg.insert("email".to_string(), json!(email));
    msg.insert("text".to_string(), json!(text));
    msg.insert("ts".to_string(), json!(mitch_lib::school::now_millis()));
    if let Some(color) = mitch_lib::shop::public_active_color(
        &state.store,
        &email,
        &jsval::or(user_cosm.get("activeColor"), json!("")),
    ) {
        msg.insert("color".to_string(), json!(color));
    }
    msg.insert(
        "badge".to_string(),
        jsval::or(user_cosm.get("activeBadge"), json!(Value::Null)),
    );
    msg.insert(
        "chatEffect".to_string(),
        jsval::or(user_cosm.get("activeChatEffect"), json!(Value::Null)),
    );
    msg.insert("reactions".to_string(), json!({}));
    let msg = Value::Object(msg);

    // Shadow-banned senders get a success response with nothing stored.
    if state
        .shadow_bans
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&norm_email)
    {
        return json_response(200, json!({ "ok": true }));
    }
    let mut next_history = history.as_array().cloned().unwrap_or_default();
    next_history.push(msg.clone());
    let start = next_history.len().saturating_sub(1000);
    let _ = state
        .store
        .write_document(&chat_file, &json!(next_history[start..].to_vec()));
    touch_presence(state, &norm_email);

    // `pickle_chat` WS fan-out (server.js:16676-16686) — the payload
    // recomputes the masked email, active color, and jar at send time.
    {
        let mut out = msg.as_object().cloned().unwrap_or_default();
        out.insert("email".into(), json!(mask_email(&email)));
        match mitch_lib::shop::public_active_color(
            &state.store,
            &jsval::str_or(
                msg.get("email"),
                &jsval::string(&jsval::or(msg.get("name"), json!(""))),
            ),
            &jsval::or(msg.get("color"), json!(Value::Null)),
        ) {
            Some(color) => {
                out.insert("color".into(), json!(color));
            }
            None => {
                out.remove("color"); // JS: `{...msg, color: undefined}` drops the key
            }
        }
        out.insert(
            "jar".into(),
            pickle_jar_for(
                state,
                &jsval::string(&jsval::or(msg.get("email"), json!(""))),
                None,
            ),
        );
        crate::ws::broadcast(
            state,
            crate::ws::WsRecipients::All,
            json!({ "type": "pickle_chat", "msg": Value::Object(out) }).to_string(),
        );
    }
    json_response(200, json!({ "ok": true }))
}

/// `/api/pickle-chat/react` (server.js:16693-16731) — one toggle per member
/// per emoji; the stored value is `{ emoji: [normEmail, ...] }`.
fn chat_react(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let email = match session_email(state, headers) {
        Ok(e) => e,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let msg_id = jsval::string(&jsval::or(body.get("msgId"), json!("")));
    let emoji = jsval::string(&jsval::or(body.get("emoji"), json!("")));
    if msg_id.is_empty() || emoji.is_empty() || emoji.chars().count() > 3 {
        return json_response(400, json!({ "error": "msgId and emoji required" }));
    }
    let chat_file = data_file(state, "pickle_chat.json");
    let mut history = state.store.read_document(&chat_file, json!([]));
    let idx = history.as_array().and_then(|arr| {
        arr.iter()
            .position(|m| jsval::string(&jsval::or(m.get("id"), json!(""))) == msg_id)
    });
    let Some(idx) = idx else {
        return json_response(404, json!({ "error": "message not found" }));
    };
    let norm_email = auth::normalize_email(&email);
    let tally = toggle_reaction(&mut history, idx, &emoji, &norm_email);
    let _ = state.store.write_document(&chat_file, &history);
    // `pickle_chat_react` WS fan-out (server.js:16718-16723).
    crate::ws::broadcast(
        state,
        crate::ws::WsRecipients::All,
        json!({ "type": "pickle_chat_react", "msgId": msg_id, "reactions": tally }).to_string(),
    );
    let my_reactions = my_reactions_of(history.as_array().and_then(|a| a.get(idx)), &norm_email);
    json_response(
        200,
        json!({ "ok": true, "reactions": tally, "myReactions": my_reactions }),
    )
}

/// `/api/pickle-chat/presence` (server.js:16733-16746) — heartbeat; prunes
/// stale entries so the count stays honest.
fn chat_presence(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    if !auth::valid_id(&sid, &state.id_secret) {
        return json_response(401, json!({ "error": "unauthorized" }));
    }
    let now = mitch_lib::school::now_millis();
    let mut presence = state
        .pickle_presence
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Ok(email) = session_email(state, headers) {
        touch_presence_locked(&mut presence, &auth::normalize_email(&email), now);
    }
    presence.retain(|(_, ts)| now - *ts <= 90_000);
    let online = presence.len();
    drop(presence);
    json_response(200, json!({ "ok": true, "online": online }))
}

/// `/api/pickle-chat/history` (server.js:23955-23990) — last 100 messages
/// with viewer-relative fields.
fn chat_history(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let viewer_email = match session_email(state, headers) {
        Ok(e) => e,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    let sid = me_uid(&cookies_of(state, headers));
    ensure_pickle_member(state, &sid); // page-load beacon: register as pending on first visit
    let history = state
        .store
        .read_document(&data_file(state, "pickle_chat.json"), json!([]));
    let members = state
        .store
        .read_document(&data_file(state, "pickle_members.json"), json!({}));
    let viewer_norm = auth::normalize_email(&viewer_email);
    let arr = history.as_array().cloned().unwrap_or_default();
    let start = arr.len().saturating_sub(100);
    let messages: Vec<Value> = arr[start..]
        .iter()
        .map(|msg| {
            let raw_email = jsval::string(&jsval::or(msg.get("email"), json!("")));
            let processed = profile::process_member_fields(
                &state.store,
                state.data_dir(),
                &raw_email,
                None,
                Some(&viewer_email),
            );
            let (reactions, my_reactions) = reactions_tally(msg.get("reactions"), &viewer_norm);
            let mut out = match msg.as_object() {
                Some(map) => map.clone(),
                None => Map::new(),
            };
            out.insert(
                "email".to_string(),
                processed.get("email").cloned().unwrap_or(json!("")),
            );
            match mitch_lib::shop::public_active_color(
                &state.store,
                &jsval::str_or(
                    msg.get("email"),
                    &jsval::string(&jsval::or(msg.get("name"), json!(""))),
                ),
                &jsval::or(msg.get("color"), json!(Value::Null)),
            ) {
                Some(color) => {
                    out.insert("color".to_string(), json!(color));
                }
                None => {
                    out.remove("color"); // JS: `{...msg, color: undefined}` drops the key
                }
            }
            out.insert(
                "chatEffect".to_string(),
                jsval::or(msg.get("chatEffect"), json!(Value::Null)),
            );
            out.insert(
                "jar".to_string(),
                pickle_jar_for(state, &raw_email, Some(&members)),
            );
            out.insert("reactions".to_string(), reactions);
            out.insert("myReactions".to_string(), my_reactions);
            Value::Object(out)
        })
        .collect();
    json_response(200, json!({ "messages": messages }))
}

// ── Pickle Clubhouse (pickle-club) ──────────────────────────────────────────

/// `/api/pickle-club/vote` (server.js:16750-16769) — one vote per member per
/// UTC day; the store keeps only the last 30 days.
fn club_vote(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let email = match session_email(state, headers) {
        Ok(e) => e,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    // `Number(body.choice)` — an absent key is `undefined` → NaN → invalid
    // (a JSON null is `Number(null) = 0` → valid).
    let Some(choice_v) = body.get("choice") else {
        return json_response(400, json!({ "error": "invalid choice" }));
    };
    let choice = jsval::number(choice_v);
    let Some(choice) = choice.filter(|c| *c == 0.0 || *c == 1.0) else {
        return json_response(400, json!({ "error": "invalid choice" }));
    };
    let file = data_file(state, "pickle_votes.json");
    let mut all = state.store.read_document(&file, json!({}));
    let day = mitch_lib::school::utc_iso_day(mitch_lib::school::now_millis());
    if let Some(map) = all.as_object_mut() {
        // `if (!all[day]) all[day] = {}` — a present-but-falsy slot is replaced.
        if !jsval::truthy(map.get(&day).unwrap_or(&Value::Null)) {
            map.insert(day.clone(), json!({}));
        }
        if let Some(dv) = map.get_mut(&day).and_then(|v| v.as_object_mut()) {
            dv.insert(auth::normalize_email(&email), json!(choice as i64));
        }
        // Keep only the last 30 days of votes.
        let mut days: Vec<String> = map.keys().cloned().collect();
        days.sort();
        while days.len() > 30 {
            let oldest = days.remove(0);
            map.remove(&oldest);
        }
    }
    let _ = state.store.write_document(&file, &all);
    json_response(200, json!({ "ok": true }))
}

/// `/api/pickle-club/crunch` (server.js:16771-16788) — the Crunch-o-meter,
/// one 1-10 rating per member, re-ratable.
fn club_crunch(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let email = match session_email(state, headers) {
        Ok(e) => e,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let Some(rating_v) = body.get("rating") else {
        return json_response(400, json!({ "error": "rating must be 1-10" }));
    };
    let rating = jsval::number(rating_v).map(|r| (r + 0.5).floor() as i64); // Math.round
    let Some(rating) = rating.filter(|r| (1..=10).contains(r)) else {
        return json_response(400, json!({ "error": "rating must be 1-10" }));
    };
    let file = data_file(state, "pickle_crunch.json");
    let mut data = state.store.read_document(&file, json!({}));
    if let Some(map) = data.as_object_mut() {
        map.insert(auth::normalize_email(&email), json!(rating));
    }
    let _ = state.store.write_document(&file, &data);
    json_response(200, json!({ "ok": true }))
}

/// `/api/pickle-club/join` (server.js:16791-16814) — no body parse in the JS.
fn club_join(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let email = match session_email(state, headers) {
        Ok(e) => e,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    let norm = auth::normalize_email(&email);
    if PICKLE_FOUNDERS.contains(&norm.as_str()) {
        return json_response(
            200,
            json!({ "ok": true, "status": "approved", "isFounder": true }),
        );
    }
    let file = data_file(state, "pickle_members.json");
    let mut all = state.store.read_document(&file, json!({}));
    if all
        .get(&norm)
        .and_then(|r| r.get("status"))
        .and_then(|v| v.as_str())
        == Some("approved")
    {
        return json_response(200, json!({ "ok": true, "status": "approved" }));
    }
    // New application, or a rejected one reapplies (fresh timestamp, note cleared).
    if let Some(map) = all.as_object_mut() {
        map.insert(
            norm,
            json!({
                "status": "pending",
                "requestedAt": mitch_lib::school::now_millis(),
                "approvedAt": 0,
                "approvedBy": "",
                "memberNumber": 0,
                "note": "",
            }),
        );
    }
    let _ = state.store.write_document(&file, &all);
    json_response(200, json!({ "ok": true, "status": "pending" }))
}

/// `/api/pickle-club/decide` (server.js:16816-16851) — founders approve/reject.
fn club_decide(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    if !auth::valid_id(&sid, &state.id_secret) {
        return json_response(401, json!({ "error": "unauthorized" }));
    }
    if !is_pickle_founder_id(state, &sid) {
        return json_response(403, json!({ "error": "forbidden" }));
    }
    let actor_email =
        auth::email_from_sid(&state.store, &state.id_secret, &sid).unwrap_or_default();
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let target = auth::normalize_email(&jsval::string(&jsval::or(body.get("email"), json!(""))));
    let decision = jsval::string(&jsval::or(body.get("decision"), json!("")));
    if target.is_empty() || (decision != "approved" && decision != "rejected") {
        return json_response(
            400,
            json!({ "error": "email and decision (approved|rejected) required" }),
        );
    }
    if PICKLE_FOUNDERS.contains(&target.as_str()) {
        return json_response(
            400,
            json!({ "error": "founders and co-owners are always approved" }),
        );
    }
    let file = data_file(state, "pickle_members.json");
    let mut all = state.store.read_document(&file, json!({}));
    let mut rec = all.get(&target).cloned().unwrap_or(json!({
        "status": "pending",
        "requestedAt": mitch_lib::school::now_millis(),
        "approvedAt": 0,
        "approvedBy": "",
        "memberNumber": 0,
        "note": "",
    }));
    if let Some(obj) = rec.as_object_mut() {
        obj.insert("status".to_string(), json!(decision));
        obj.insert(
            "note".to_string(),
            json!(jsval::js_slice_utf16(
                &jsval::string(&jsval::or(body.get("note"), json!(""))),
                300
            )),
        );
        if decision == "approved" {
            let current_number = obj
                .get("memberNumber")
                .and_then(jsval::number)
                .unwrap_or(0.0) as i64;
            if current_number == 0 {
                let mut max = PICKLE_FOUNDERS.len() as i64; // founders hold Jar #1..#N
                if let Some(map) = all.as_object() {
                    for r in map.values() {
                        max = max.max(
                            jsval::number(r.get("memberNumber").unwrap_or(&Value::Null))
                                .unwrap_or(0.0) as i64,
                        );
                    }
                }
                for o in pickle_co_owners(state)
                    .as_object()
                    .map(|m| m.values().cloned().collect::<Vec<_>>())
                    .unwrap_or_default()
                {
                    max = max.max(
                        jsval::number(o.get("memberNumber").unwrap_or(&Value::Null)).unwrap_or(0.0)
                            as i64,
                    );
                }
                obj.insert("memberNumber".to_string(), json!(max + 1));
            }
            obj.insert(
                "approvedAt".to_string(),
                json!(mitch_lib::school::now_millis()),
            );
            obj.insert(
                "approvedBy".to_string(),
                json!(auth::normalize_email(actor_email.trim())),
            );
        }
    }
    if let Some(map) = all.as_object_mut() {
        map.insert(target.clone(), rec.clone());
    }
    let _ = state.store.write_document(&file, &all);
    log_admin_action(
        &state.store,
        state.data_dir(),
        &actor_email,
        &format!("pickle_member_{decision}"),
        json!({ "email": target, "note": rec.get("note").cloned().unwrap_or(json!("")) }),
    );
    json_response(
        200,
        json!({
            "ok": true,
            "status": rec.get("status").cloned().unwrap_or(json!(Value::Null)),
            "memberNumber": jsval::or(rec.get("memberNumber"), json!(Value::Null)),
        }),
    )
}

/// `/api/pickle-club/owners` (server.js:16853-16893) — appoint/remove
/// co-owners. Founders only; works by raw email even before that person has
/// an account.
fn club_owners(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    if !auth::valid_id(&sid, &state.id_secret) {
        return json_response(401, json!({ "error": "unauthorized" }));
    }
    if !is_pickle_founder_id(state, &sid) {
        return json_response(403, json!({ "error": "forbidden" }));
    }
    let actor_email =
        auth::email_from_sid(&state.store, &state.id_secret, &sid).unwrap_or_default();
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let target = auth::normalize_email(&jsval::string(&jsval::or(body.get("email"), json!(""))));
    let action = jsval::string(&jsval::or(body.get("action"), json!("")));
    if target.is_empty() || (action != "appoint" && action != "remove") {
        return json_response(
            400,
            json!({ "error": "email and action (appoint|remove) required" }),
        );
    }
    if PICKLE_FOUNDERS.contains(&target.as_str()) {
        return json_response(
            400,
            json!({ "error": "that account is already a co-founder" }),
        );
    }
    let owners_file = data_file(state, "pickle_owners.json");
    let mut owners = pickle_co_owners(state);
    if action == "appoint" {
        let already = owners
            .as_object()
            .map(|m| m.contains_key(&target))
            .unwrap_or(false);
        if !already {
            // An existing approved member keeps the Jar number they already
            // hold; everyone else takes the next number after all holders.
            let roster = state
                .store
                .read_document(&data_file(state, "pickle_members.json"), json!({}))
                .get(&target)
                .cloned();
            let mut member_number = match roster {
                Some(r) if r.get("status").and_then(|v| v.as_str()) == Some("approved") => {
                    jsval::number(r.get("memberNumber").unwrap_or(&Value::Null)).unwrap_or(0.0)
                        as i64
                }
                _ => 0,
            };
            if member_number == 0 {
                let mut max = PICKLE_FOUNDERS.len() as i64;
                if let Some(map) = state
                    .store
                    .read_document(&data_file(state, "pickle_members.json"), json!({}))
                    .as_object()
                {
                    for r in map.values() {
                        max = max.max(
                            jsval::number(r.get("memberNumber").unwrap_or(&Value::Null))
                                .unwrap_or(0.0) as i64,
                        );
                    }
                }
                if let Some(map) = owners.as_object() {
                    for o in map.values() {
                        max = max.max(
                            jsval::number(o.get("memberNumber").unwrap_or(&Value::Null))
                                .unwrap_or(0.0) as i64,
                        );
                    }
                }
                member_number = max + 1;
            }
            if let Some(map) = owners.as_object_mut() {
                map.insert(
                    target.clone(),
                    json!({
                        "appointedAt": mitch_lib::school::now_millis(),
                        "appointedBy": auth::normalize_email(actor_email.trim()),
                        "memberNumber": member_number,
                    }),
                );
            }
            let _ = state.store.write_document(&owners_file, &owners);
        }
    } else {
        let already = owners
            .as_object()
            .map(|m| m.contains_key(&target))
            .unwrap_or(false);
        if !already {
            return json_response(404, json!({ "error": "not a co-owner" }));
        }
        if let Some(map) = owners.as_object_mut() {
            map.remove(&target);
        }
        let _ = state.store.write_document(&owners_file, &owners);
    }
    log_admin_action(
        &state.store,
        state.data_dir(),
        &actor_email,
        &format!("pickle_owner_{action}"),
        json!({ "email": target }),
    );
    let coowners: Vec<Value> = owners
        .as_object()
        .map(|m| m.keys().map(|k| json!(k)).collect())
        .unwrap_or_default();
    json_response(
        200,
        json!({ "ok": true, "action": action, "coowners": coowners }),
    )
}

/// `/api/pickle-club/membership` GET (server.js:17776-17791).
fn club_membership(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let email = match session_email(state, headers) {
        Ok(e) => e,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    let norm = auth::normalize_email(&email);
    let rec = pickle_membership_for(state, &norm).unwrap_or_else(|| {
        json!({
            "status": "pending",
            "memberNumber": 0,
            "requestedAt": mitch_lib::school::now_millis(),
            "approvedAt": 0,
            "note": "",
        })
    });
    json_response(
        200,
        json!({
            "status": jsval::or(rec.get("status"), json!(Value::Null)),
            "memberNumber": jsval::or(rec.get("memberNumber"), json!(Value::Null)),
            "requestedAt": jsval::number(rec.get("requestedAt").unwrap_or(&Value::Null)).unwrap_or(0.0) as i64,
            "approvedAt": jsval::number(rec.get("approvedAt").unwrap_or(&Value::Null)).unwrap_or(0.0) as i64,
            "note": jsval::string(&jsval::or(rec.get("note"), json!(""))),
            "isFounder": jsval::truthy(rec.get("founder").unwrap_or(&Value::Null)),
        }),
    )
}

/// `/api/pickle-club/applicants` GET (server.js:17795-17832) — founders only.
fn club_applicants(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    if !is_pickle_founder_id(state, &sid) {
        return json_response(403, json!({ "error": "forbidden" }));
    }
    let all = state
        .store
        .read_document(&data_file(state, "pickle_members.json"), json!({}));
    let profiles = mitch_lib::profiles::profile_all(&state.store);
    fn status_rank(status: &str) -> i64 {
        match status {
            "pending" => 0,
            "approved" => 1,
            "rejected" => 2,
            _ => 3,
        }
    }
    let mut applicants: Vec<Value> = all
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(key, rec)| {
                    let status = jsval::string(&jsval::or(rec.get("status"), json!("")));
                    json!({
                        "key": key, // exact normalized email — what /api/pickle-club/decide expects
                        "display": mask_email(key),
                        "name": jsval::str_or(
                            profiles.get(key).and_then(|p| p.get("displayName")).filter(|v| jsval::truthy(v)),
                            key.split('@').next().unwrap_or(""),
                        ),
                        "status": status,
                        "requestedAt": jsval::number(rec.get("requestedAt").unwrap_or(&Value::Null)).unwrap_or(0.0) as i64,
                        "approvedAt": jsval::number(rec.get("approvedAt").unwrap_or(&Value::Null)).unwrap_or(0.0) as i64,
                        "memberNumber": jsval::or(rec.get("memberNumber"), json!(Value::Null)),
                        "note": jsval::string(&jsval::or(rec.get("note"), json!(""))),
                        "_rank": status_rank(&status),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    // `.sort((a, b) => (statusRank[a.status] ?? 3) - (statusRank[b.status] ?? 3)
    //        || b.requestedAt - a.requestedAt)` — stable.
    applicants.sort_by(|a, b| {
        let rank = a
            .get("_rank")
            .and_then(|v| v.as_i64())
            .cmp(&b.get("_rank").and_then(|v| v.as_i64()));
        let req = |v: &Value| v.get("requestedAt").and_then(|x| x.as_i64()).unwrap_or(0);
        rank.then_with(|| req(b).cmp(&req(a)))
    });
    for a in applicants.iter_mut() {
        if let Some(obj) = a.as_object_mut() {
            obj.remove("_rank");
        }
    }
    let mut counts = json!({ "pending": 0, "approved": 0, "rejected": 0 });
    for a in &applicants {
        let status = jsval::string(&jsval::or(a.get("status"), json!("")));
        if let Some(slot) = counts.get_mut(&status) {
            if let Some(n) = slot.as_i64() {
                *slot = json!(n + 1);
            }
        }
    }
    let mut coowners: Vec<Value> = pickle_co_owners(state)
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(email, o)| {
                    json!({
                        "email": email, // exact normalized email — what /api/pickle-club/owners expects
                        "display": mask_email(email),
                        "memberNumber": jsval::or(o.get("memberNumber"), json!(Value::Null)),
                        "appointedAt": jsval::number(o.get("appointedAt").unwrap_or(&Value::Null)).unwrap_or(0.0) as i64,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    coowners.sort_by_key(|c| c.get("memberNumber").and_then(jsval::number).unwrap_or(0.0) as i64);
    let founders: Vec<Value> = PICKLE_FOUNDERS
        .iter()
        .map(|e| json!(mask_email(e)))
        .collect();
    json_response(
        200,
        json!({ "applicants": applicants, "counts": counts, "founders": founders, "coowners": coowners }),
    )
}

// ── Club Bulletin (pickle-bulletin) ─────────────────────────────────────────

/// `/api/pickle-bulletin/post` (server.js:16895-16929) — founders publish.
async fn bulletin_post(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    if !auth::valid_id(&sid, &state.id_secret) {
        return json_response(401, json!({ "error": "unauthorized" }));
    }
    if !is_pickle_founder_id(state, &sid) {
        return json_response(
            403,
            json!({ "error": "only the co-founders publish to the Bulletin" }),
        );
    }
    let email = auth::email_from_sid(&state.store, &state.id_secret, &sid).unwrap_or_default();
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    static WS_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let ws_re = WS_RE.get_or_init(|| {
        regex::Regex::new(r"\s+").unwrap_or_else(|_| {
            regex::Regex::new("$^")
                .unwrap_or_else(|_| regex::Regex::new("$^").unwrap_or_else(|_| unreachable!()))
        })
    });
    let title_raw = jsval::string(&jsval::or(body.get("title"), json!("")));
    let title = jsval::js_slice_utf16(ws_re.replace_all(&title_raw, " ").trim(), 140);
    if title.is_empty() {
        return json_response(400, json!({ "error": "title required" }));
    }
    // `sanitizeBlogHtml(String(body.html || body.text || ''), String(body.text || '').slice(0, 20000))`
    // — the `||` chain runs on the RAW values (a falsy `html` of any JSON
    // type falls through to `text`).
    let truthy_or = |key: &str| -> Value {
        match body.get(key) {
            Some(v) if jsval::truthy(v) => v.clone(),
            _ => json!(""),
        }
    };
    let text = jsval::string(&truthy_or("text"));
    let html_source = jsval::string(&truthy_or("html"));
    let html = blog::sanitize_blog_html(&html_source, &jsval::js_slice_utf16(&text, 20_000));
    if html.trim().is_empty() {
        return json_response(400, json!({ "error": "empty post" }));
    }
    let norm = auth::normalize_email(&email);
    let profiles = mitch_lib::profiles::profile_all(&state.store);
    let author_name = jsval::str_or(
        profiles
            .get(&norm)
            .and_then(|p| p.get("displayName"))
            .filter(|v| jsval::truthy(v)),
        email.split('@').next().unwrap_or(""),
    );
    let post = json!({
        "id": random_bytes_hex(8),
        "title": title,
        "html": html,
        "authorEmail": norm,
        "authorName": author_name,
        "ts": mitch_lib::school::now_millis(),
        "updatedAt": 0,
        "pinned": jsval::truthy(body.get("pinned").unwrap_or(&Value::Null)),
        "reactions": {},
    });
    let file = data_file(state, "pickle_bulletin.json");
    let posts = state.store.read_document(&file, json!([]));
    let mut next_posts = posts.as_array().cloned().unwrap_or_default();
    next_posts.insert(0, post.clone()); // unshift
    let keep_from = next_posts.len().saturating_sub(500);
    let _ = state
        .store
        .write_document(&file, &json!(next_posts[keep_from..].to_vec()));
    log_admin_action(
        &state.store,
        state.data_dir(),
        &email,
        "pickle_bulletin_post",
        json!({
            "id": post.get("id").cloned().unwrap_or(json!(Value::Null)),
            "title": title,
        }),
    );
    // `pickle_bulletin_new` WS fan-out (server.js:16916-16920) — public
    // viewer ('').
    crate::ws::broadcast(
        state,
        crate::ws::WsRecipients::All,
        json!({ "type": "pickle_bulletin_new", "post": pickle_bulletin_public(&post, "") })
            .to_string(),
    );
    json_response(
        200,
        json!({ "ok": true, "post": pickle_bulletin_public(&post, &norm) }),
    )
}

/// `/api/pickle-bulletin/react` (server.js:16931-16966).
fn bulletin_react(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let email = match session_email(state, headers) {
        Ok(e) => e,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let post_id = jsval::string(&jsval::or(body.get("id"), json!("")));
    let emoji = jsval::string(&jsval::or(body.get("emoji"), json!("")));
    if post_id.is_empty() || emoji.is_empty() || emoji.chars().count() > 3 {
        return json_response(400, json!({ "error": "id and emoji required" }));
    }
    let file = data_file(state, "pickle_bulletin.json");
    let mut posts = state.store.read_document(&file, json!([]));
    let idx = posts.as_array().and_then(|arr| {
        arr.iter()
            .position(|p| jsval::string(&jsval::or(p.get("id"), json!(""))) == post_id)
    });
    let Some(idx) = idx else {
        return json_response(404, json!({ "error": "post not found" }));
    };
    let norm_email = auth::normalize_email(&email);
    let tally = toggle_reaction(&mut posts, idx, &emoji, &norm_email);
    let _ = state.store.write_document(&file, &posts);
    // `pickle_bulletin_react` WS fan-out (server.js:16952-16956).
    crate::ws::broadcast(
        state,
        crate::ws::WsRecipients::All,
        json!({ "type": "pickle_bulletin_react", "id": post_id, "reactions": tally }).to_string(),
    );
    let my_reactions = my_reactions_of(posts.as_array().and_then(|a| a.get(idx)), &norm_email);
    json_response(
        200,
        json!({ "ok": true, "reactions": tally, "myReactions": my_reactions }),
    )
}

/// `/api/pickle-bulletin/delete` (server.js:16968-16998) — founder or author.
fn bulletin_delete(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let email = match session_email(state, headers) {
        Ok(e) => e,
        Err((code, msg)) => return json_response(code, json!({ "error": msg })),
    };
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let post_id = jsval::string(&jsval::or(body.get("id"), json!("")));
    let file = data_file(state, "pickle_bulletin.json");
    let mut posts = state.store.read_document(&file, json!([]));
    let idx = posts.as_array().and_then(|arr| {
        arr.iter()
            .position(|p| jsval::string(&jsval::or(p.get("id"), json!(""))) == post_id)
    });
    let Some(idx) = idx else {
        return json_response(404, json!({ "error": "post not found" }));
    };
    let is_founder = is_pickle_founder_id(state, &me_uid(&cookies_of(state, headers)));
    let is_author = auth::normalize_email(&jsval::string(&jsval::or(
        posts
            .as_array()
            .and_then(|a| a.get(idx))
            .and_then(|p| p.get("authorEmail")),
        json!(""),
    ))) == auth::normalize_email(&email);
    if !is_founder && !is_author {
        return json_response(403, json!({ "error": "forbidden" }));
    }
    if let Some(arr) = posts.as_array_mut() {
        arr.remove(idx);
    }
    let _ = state.store.write_document(&file, &posts);
    log_admin_action(
        &state.store,
        state.data_dir(),
        &email,
        "pickle_bulletin_delete",
        json!({ "id": post_id }),
    );
    // `pickle_bulletin_delete` WS fan-out (server.js:16989-16993).
    crate::ws::broadcast(
        state,
        crate::ws::WsRecipients::All,
        json!({ "type": "pickle_bulletin_delete", "id": post_id }).to_string(),
    );
    json_response(200, json!({ "ok": true }))
}

/// `/api/pickle-bulletin/state` GET (server.js:17874-17883) — public; the
/// viewer's slice (myReactions/mine) filled in when signed in.
fn bulletin_state(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    let viewer_norm =
        if !sid.is_empty() && auth::valid_id(&sid, &state.id_secret) && !is_revoked_id(state, &sid)
        {
            auth::email_from_sid(&state.store, &state.id_secret, &sid)
                .map(|e| auth::normalize_email(&e))
                .unwrap_or_default()
        } else {
            String::new()
        };
    let posts = state
        .store
        .read_document(&data_file(state, "pickle_bulletin.json"), json!([]));
    let mut sorted = posts.as_array().cloned().unwrap_or_default();
    // `.sort((a, b) => (b.pinned ? 1 : 0) - (a.pinned ? 1 : 0) || (b.ts || 0) - (a.ts || 0))`
    sorted.sort_by(|a, b| {
        let pinned = |p: &Value| i64::from(jsval::truthy(p.get("pinned").unwrap_or(&Value::Null)));
        let ts =
            |p: &Value| jsval::number(p.get("ts").unwrap_or(&Value::Null)).unwrap_or(0.0) as i64;
        pinned(b).cmp(&pinned(a)).then_with(|| ts(b).cmp(&ts(a)))
    });
    let out: Vec<Value> = sorted
        .iter()
        .map(|p| pickle_bulletin_public(p, &viewer_norm))
        .collect();
    json_response(
        200,
        json!({ "posts": out, "now": mitch_lib::school::now_millis() }),
    )
}

// ── Clubhouse dashboard (features) ──────────────────────────────────────────

/// `/api/pickle-club/features` GET (server.js:23992-24114) — public
/// aggregates with the viewer's personal slice filled in when signed in.
fn club_features(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    let sid_ok =
        !sid.is_empty() && auth::valid_id(&sid, &state.id_secret) && !is_revoked_id(state, &sid);
    let viewer_email = if sid_ok {
        auth::email_from_sid(&state.store, &state.id_secret, &sid).unwrap_or_default()
    } else {
        String::new()
    };
    let viewer_norm = if viewer_email.is_empty() {
        String::new()
    } else {
        auth::normalize_email(&viewer_email)
    };

    // Pickle of the Day: a deterministic matchup per UTC day, one vote each.
    const POTD_PICKLES: [(&str, &str); 12] = [
        ("Gherkin Gandalf", "🧙"),
        ("Dill Diamond", "💎"),
        ("Sir Crunchworthy", "🛡️"),
        ("The Brine Whisperer", "🌊"),
        ("Kosher Kommander", "🫡"),
        ("Bread & Butter Bob", "🥖"),
        ("Half-Sour Hank", "🤠"),
        ("The Full Spear", "🔱"),
        ("Pickleberry Piet", "🍓"),
        ("Madam Cucumberstein", "🎭"),
        ("Fermento", "🦸"),
        ("The Last Dill standing", "🥒"),
    ];
    let day = mitch_lib::school::utc_iso_day(mitch_lib::school::now_millis());
    // `[...day].reduce((h, c) => (h * 31 + c.charCodeAt(0)) >>> 0, 7)` — 32-bit.
    let day_seed: u32 = day
        .chars()
        .fold(7u32, |h, c| h.wrapping_mul(31).wrapping_add(c as u32));
    let idx_a = (day_seed as usize) % POTD_PICKLES.len();
    let mut idx_b = ((day_seed >> 3) as usize) % POTD_PICKLES.len();
    if idx_b == idx_a {
        idx_b = (idx_b + 1) % POTD_PICKLES.len();
    }
    let votes = state
        .store
        .read_document(&data_file(state, "pickle_votes.json"), json!({}))
        .get(&day)
        .cloned()
        .unwrap_or(json!({}));
    let mut votes_a = 0i64;
    let mut votes_b = 0i64;
    if let Some(map) = votes.as_object() {
        for v in map.values() {
            match v {
                Value::Number(n) if n.as_f64() == Some(0.0) => votes_a += 1,
                Value::Number(n) if n.as_f64() == Some(1.0) => votes_b += 1,
                _ => {}
            }
        }
    }

    // Crunch-o-meter: one 1-10 rating per member, re-ratable.
    let crunch_data = state
        .store
        .read_document(&data_file(state, "pickle_crunch.json"), json!({}));
    let mut crunch_sum = 0.0f64;
    let mut crunch_count = 0i64;
    if let Some(map) = crunch_data.as_object() {
        for v in map.values() {
            if let Some(n) = jsval::number(v) {
                if (1.0..=10.0).contains(&n) {
                    crunch_sum += n;
                    crunch_count += 1;
                }
            }
        }
    }

    // Top Briners: most active voices in The Barrel.
    let chat_history = state
        .store
        .read_document(&data_file(state, "pickle_chat.json"), json!([]));
    let mut msg_counts: Vec<(String, i64, i64)> = Vec::new(); // (norm, count, firstSeen) — Map order
    let mut first_seen_index: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    if let Some(arr) = chat_history.as_array() {
        for m in arr {
            let k = auth::normalize_email(&jsval::string(&jsval::or(m.get("email"), json!(""))));
            if k.is_empty() {
                continue;
            }
            match first_seen_index.get(&k) {
                Some(&i) => msg_counts[i].1 += 1,
                None => {
                    let ts =
                        jsval::number(m.get("ts").unwrap_or(&Value::Null)).unwrap_or(0.0) as i64;
                    first_seen_index.insert(k.clone(), msg_counts.len());
                    msg_counts.push((k, 1, ts));
                }
            }
        }
    }
    let profiles = mitch_lib::profiles::profile_all(&state.store);
    let mut top_briners: Vec<&(String, i64, i64)> = msg_counts.iter().collect();
    top_briners.sort_by_key(|e| std::cmp::Reverse(e.1)); // stable, like JS
    let top_briners: Vec<Value> = top_briners
        .iter()
        .take(5)
        .enumerate()
        .map(|(i, (em, count, _))| {
            let name = jsval::str_or(
                profiles
                    .get(em)
                    .and_then(|p| p.get("displayName"))
                    .filter(|v| jsval::truthy(v)),
                em.split('@').next().unwrap_or(""),
            );
            json!({
                "name": name,
                "count": count,
                "trophy": match i { 0 => "🥇", 1 => "🥈", 2 => "🥉", _ => "🥒" },
            })
        })
        .collect();

    // Who's in the barrel right now (heartbeat < 90s old).
    let now = mitch_lib::school::now_millis();
    let mut presence = state
        .pickle_presence
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    presence.retain(|(_, ts)| now - *ts <= 90_000);
    let online_count = presence.len();
    let online_names: Vec<Value> = presence
        .iter()
        .map(|(em, _)| {
            json!(jsval::str_or(
                profiles
                    .get(em)
                    .and_then(|p| p.get("displayName"))
                    .filter(|v| jsval::truthy(v)),
                em.split('@').next().unwrap_or(""),
            ))
        })
        .collect();
    drop(presence);

    // The viewer's badges.
    let mut badges: Vec<Value> = Vec::new();
    let msg_count_of_viewer = msg_counts
        .iter()
        .find(|(em, _, _)| *em == viewer_norm)
        .map(|(_, c, _)| *c)
        .unwrap_or(0);
    if !viewer_norm.is_empty() && is_pickle_founder_email(state, &viewer_norm) {
        badges.push(json!({ "id": "co_owner", "label": "Co-Owner", "emoji": "👑" }));
    }
    if !viewer_norm.is_empty()
        && jsval::truthy(crunch_data.get(&viewer_norm).unwrap_or(&Value::Null))
    {
        badges
            .push(json!({ "id": "crunch_certified", "label": "Certified Crunchy", "emoji": "💥" }));
    }
    if msg_count_of_viewer >= 25 {
        badges.push(json!({ "id": "chatty", "label": "Chatty Brine", "emoji": "💬" }));
    }
    let mut originals: Vec<&(String, i64, i64)> = msg_counts.iter().collect();
    originals.sort_by_key(|e| e.2); // stable, like JS
    let originals: Vec<&str> = originals
        .iter()
        .take(10)
        .map(|(em, _, _)| em.as_str())
        .collect();
    if !viewer_norm.is_empty() && originals.contains(&viewer_norm.as_str()) {
        badges.push(json!({ "id": "original_brine", "label": "Original Brine", "emoji": "🏺" }));
    }

    // Membership status (page-load beacon: first visit registers as pending).
    ensure_pickle_member(state, &sid);
    let membership = if viewer_norm.is_empty() {
        None
    } else {
        pickle_membership_for(state, &viewer_norm)
    };
    let membership_out = match &membership {
        Some(m) => json!({
            "status": jsval::or(m.get("status"), json!(Value::Null)),
            "memberNumber": jsval::or(m.get("memberNumber"), json!(Value::Null)),
            "isFounder": jsval::truthy(m.get("founder").unwrap_or(&Value::Null))
                || jsval::truthy(m.get("coowner").unwrap_or(&Value::Null)),
        }),
        None if !viewer_norm.is_empty() => {
            json!({ "status": "pending", "memberNumber": Value::Null, "isFounder": false })
        }
        _ => Value::Null,
    };

    let potd = |i: usize| json!({ "name": POTD_PICKLES[i].0, "emoji": POTD_PICKLES[i].1 });
    let my_vote = if !viewer_norm.is_empty() && votes.get(&viewer_norm).is_some() {
        votes.get(&viewer_norm).cloned().unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let average = if crunch_count > 0 {
        json!(((crunch_sum / crunch_count as f64 * 10.0 + 0.5).floor()) / 10.0)
    } else {
        Value::Null
    };
    json_response(
        200,
        json!({
            "potd": {
                "day": day,
                "a": potd(idx_a),
                "b": potd(idx_b),
                "votesA": votes_a,
                "votesB": votes_b,
                "total": votes_a + votes_b,
                "myVote": my_vote,
            },
            "crunch": {
                "average": average,
                "count": crunch_count,
                "myRating": if viewer_norm.is_empty() { Value::Null } else {
                    crunch_data.get(&viewer_norm).cloned().unwrap_or(Value::Null)
                },
            },
            "topBriners": top_briners,
            "online": { "count": online_count, "names": online_names.into_iter().take(12).collect::<Vec<_>>() },
            "stats": { "messages": chat_history.as_array().map(|a| a.len()).unwrap_or(0), "briners": msg_counts.len() },
            "badges": badges,
            "membership": membership_out,
        }),
    )
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// Toggle one (emoji, member) reaction pair on `arr[idx].reactions` and
/// return the counts tally (JS reacts to the mutated object).
fn toggle_reaction(doc: &mut Value, idx: usize, emoji: &str, norm_email: &str) -> Value {
    let Some(msg) = doc.as_array_mut().and_then(|arr| arr.get_mut(idx)) else {
        return Value::Object(Map::new()); // unreachable: idx came from this array
    };
    if !msg.get("reactions").map(|r| r.is_object()).unwrap_or(false) {
        if let Some(obj) = msg.as_object_mut() {
            obj.insert("reactions".to_string(), json!({}));
        }
    }
    let list_is_array = msg
        .get("reactions")
        .and_then(|r| r.get(emoji))
        .map(|v| v.is_array())
        .unwrap_or(false);
    let mut list: Vec<Value> = if list_is_array {
        msg.get("reactions")
            .and_then(|r| r.get(emoji))
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let mine_idx = list.iter().position(|v| jsval::string(v) == norm_email);
    match mine_idx {
        Some(i) => {
            list.remove(i);
        }
        None => list.push(json!(norm_email)),
    }
    let Some(msg_obj) = msg.as_object_mut() else {
        return Value::Object(Map::new()); // unreachable: reactions ensured object
    };
    let reactions = msg_obj
        .entry("reactions".to_string())
        .or_insert_with(|| json!({}));
    if let Some(map) = reactions.as_object_mut() {
        if list.is_empty() {
            map.remove(emoji); // `delete msg.reactions[emoji]`
        } else {
            map.insert(emoji.to_string(), Value::Array(list.clone()));
        }
    }
    // Tally over the whole reactions object, insertion-ordered.
    let mut tally = Map::new();
    if let Some(map) = reactions.as_object() {
        for (em, arr) in map {
            if let Some(list) = arr.as_array() {
                if !list.is_empty() {
                    tally.insert(em.clone(), json!(list.len()));
                }
            }
        }
    }
    Value::Object(tally)
}

/// `myReactions` — emoji keys whose list includes the viewer.
fn my_reactions_of(msg: Option<&Value>, norm_email: &str) -> Value {
    let mut mine = Vec::new();
    if let Some(map) = msg
        .and_then(|m| m.get("reactions"))
        .and_then(|r| r.as_object())
    {
        for (em, arr) in map {
            if let Some(list) = arr.as_array() {
                if list.iter().any(|v| jsval::string(v) == norm_email) {
                    mine.push(json!(em));
                }
            }
        }
    }
    Value::Array(mine)
}

/// `picklePresence.set(norm, Date.now())` — Map.set keeps the insertion
/// position of an existing key.
fn touch_presence(state: &Arc<AppState>, norm: &str) {
    let mut presence = state
        .pickle_presence
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    touch_presence_locked(&mut presence, norm, mitch_lib::school::now_millis());
}

fn touch_presence_locked(presence: &mut Vec<(String, i64)>, norm: &str, now: i64) {
    match presence.iter_mut().find(|(k, _)| k == norm) {
        Some(entry) => entry.1 = now,
        None => presence.push((norm.to_string(), now)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `[...day].reduce((h, c) => (h * 31 + c.charCodeAt(0)) >>> 0, 7)` and the
    /// POTD index derivation — golden values from bun for 2026-09-14.
    #[test]
    fn potd_day_seed_matches_js() {
        const POTD_LEN: usize = 12;
        let day = "2026-09-14";
        let day_seed: u32 = day
            .chars()
            .fold(7u32, |h, c| h.wrapping_mul(31).wrapping_add(c as u32));
        assert_eq!(day_seed, 1468146467);
        let idx_a = (day_seed as usize) % POTD_LEN;
        let mut idx_b = ((day_seed >> 3) as usize) % POTD_LEN;
        if idx_b == idx_a {
            idx_b = (idx_b + 1) % POTD_LEN;
        }
        assert_eq!((idx_a, idx_b), (11, 4));
    }

    /// Presence Map.set semantics: update keeps insertion position.
    #[test]
    fn presence_touch_keeps_order() {
        let mut presence = Vec::new();
        touch_presence_locked(&mut presence, "a@x", 100);
        touch_presence_locked(&mut presence, "b@x", 200);
        touch_presence_locked(&mut presence, "a@x", 300); // update in place
        let keys: Vec<&str> = presence.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["a@x", "b@x"]);
        assert_eq!(presence[0].1, 300);
    }
}
