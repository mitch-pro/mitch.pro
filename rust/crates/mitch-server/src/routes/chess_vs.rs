//! `/api/chess-vs/*` — the ten correspondence/live chess endpoints
//! (server.js:20483-20752, plan Step 12, final games batch).
//!
//! Parity quirks preserved (all verified against the JS source):
//! - The `/challenge` handler (server.js:20509-20553) NEVER stores the
//!   challenge: it generates an id, fires the corr email + admin
//!   notification and returns `{ ok: true, id }`, but there is no
//!   `cvChallenges[id] = …` assignment anywhere in the block. So `/respond`
//!   always 404s (`challenge not found`), the heartbeat/`/challenges` lists
//!   (and `/api/ping`'s) are always `[]`, and no game can ever be created
//!   through the API. The port reproduces this exactly: the challenge map
//!   stays empty for the process lifetime, and respond keeps its full shape
//!   over that empty map.
//! - `type: "c".type` (server.js:20593) — `"c".type` evaluates to
//!   `undefined`, so a respond-created game object carries no `type` key at
//!   all, `cvSave`'s `g.type === 'corr'` filter never matches it, and
//!   `move`'s `g.type === 'live'` branch is dead for API-created games.
//!   Only games boot-loaded from data/chess_vs.json (legacy corr saves) are
//!   reachable — game/move/resign/draw/chat are live code for those.
//! - `cvSave` (server.js:857-861) persists ONLY `g.type === 'corr'` games.
//! - `cvChallenges`/`cvChats`/`cvOnline` are in-memory only; `cvGames` is
//!   seeded from data/chess_vs.json at boot (server.js:856).
//! - The JS block has ZERO `checkRateLimit` calls — the RATE_LIMITS entries
//!   for /api/chess-vs/challenge|move (1593-1594) are dead config — so the
//!   handler.rs global gate exempts this prefix.
//! - Emails: corr challenges/moves fire `sendEmailBg` via
//!   `makeChessCorrActionHtml` (server.js:1902), plus an admin notification
//!   and a `refresh_notifications` broadcast.

use axum::http::HeaderMap;
use indexmap::IndexMap;
use serde_json::{json, Value};
use std::sync::Arc;

use mitch_lib::data::js_stringify;
use mitch_lib::jsval;

use crate::errors::json_resp_str;
use crate::state::AppState;

const CHESS_VS_STARTING_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

fn resp(code: u16, body: &Value) -> axum::response::Response {
    json_resp_str(code, js_stringify(body))
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// JS `Math.max` — NaN-propagating (Rust's `f64::max` would return 0.0).
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

/// A JS number as a JSON value — `JSON.stringify(NaN)` is `null`.
fn num_json(x: f64) -> Value {
    if x.is_nan() {
        Value::Null
    } else {
        jsval::num_value(x)
    }
}

/// JS `parseInt(v)` for the tc/bet parsing (radix 10). Numbers are
/// stringified first, so `parseInt(300.7)` is 300 and `parseInt(0.5)` is 0;
/// non-string/non-number inputs are `NaN` (arrays would join in JS — the
/// crafted/board-provided values here are always strings or numbers).
fn js_parse_int(v: &Value) -> f64 {
    let s = match v {
        Value::Number(_) => match jsval::number(v) {
            Some(x) => mitch_lib::data::js_number_string(x),
            None => return f64::NAN,
        },
        Value::String(s) => s.clone(),
        _ => return f64::NAN,
    };
    let t = s.trim_start();
    let bytes = t.as_bytes();
    let mut i = 0usize;
    let neg = match bytes.first() {
        Some(b'-') => {
            i = 1;
            true
        }
        Some(b'+') => {
            i = 1;
            false
        }
        _ => false,
    };
    let start = i;
    let mut acc: i64 = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        acc = acc
            .saturating_mul(10)
            .saturating_add((bytes[i] - b'0') as i64);
        i += 1;
    }
    if i == start {
        return f64::NAN;
    }
    if neg {
        -(acc as f64)
    } else {
        acc as f64
    }
}

/// `Math.min(A, Math.max(B, parseInt(v) || FALLBACK))` — NaN/0 (falsy) fall
/// back, then the clamp applies.
fn clamp_parsed(v: &Value, fallback: f64, lo: f64, hi: f64) -> f64 {
    let p = js_parse_int(v);
    let raw = if p.is_nan() || p == 0.0 { fallback } else { p };
    js_max(lo, raw).min(hi)
}

/// The `tc` literal (server.js:20517-20521).
fn build_tc(body: &Value, is_corr: bool) -> Value {
    if is_corr {
        json!({ "perMove": clamp_parsed(
            body.get("perMove").unwrap_or(&Value::Null),
            86_400_000.0, 3_600_000.0, 7.0 * 86_400_000.0,
        ) })
    } else {
        json!({
            "initial": clamp_parsed(
                body.get("initial").unwrap_or(&Value::Null),
                300_000.0, 30_000.0, 600_000.0,
            ),
            "increment": clamp_parsed(
                body.get("increment").unwrap_or(&Value::Null),
                0.0, 0.0, 30_000.0,
            ),
        })
    }
}

/// `cvSave` (server.js:857-861) — persist only the `type === 'corr'` games.
fn cv_save(state: &Arc<AppState>, games: &IndexMap<String, Value>) {
    let mut out = serde_json::Map::new();
    for (k, g) in games.iter() {
        if g.get("type").and_then(|v| v.as_str()) == Some("corr") {
            out.insert(k.clone(), g.clone());
        }
    }
    let _ = state
        .store
        .write_document(&state.data_dir().join("chess_vs.json"), &Value::Object(out));
}

/// `cvCheckTimeout(g)` (server.js:862-879). Mutates `g` on timeout and pays
/// the winner; returns `true` when a corr game must be cvSave'd (the JS
/// calls cvSave inside).
fn cv_check_timeout(state: &Arc<AppState>, g: &mut Value) -> bool {
    if jsval::string_of(g.get("status")) != "active" {
        return false;
    }
    // `if (!g.tc) return` — null/undefined/0 are all falsy.
    if g.get("tc").is_none_or(|v| v.is_null()) {
        return false;
    }
    let is_corr = g.get("type").and_then(|v| v.as_str()) == Some("corr");
    let started_raw = if is_corr {
        g.get("clockStartedAt")
    } else {
        g.get("lastMoveAt")
    };
    // `if (!startedAt) return` — undefined/null/0/NaN are all falsy.
    let started = started_raw.and_then(jsval::number).unwrap_or(f64::NAN);
    if started == 0.0 || started.is_nan() {
        return false;
    }
    let turn = jsval::string_of(g.get("turn"));
    let clock = g
        .get("clocks")
        .and_then(|c| c.get(&turn))
        .and_then(jsval::number)
        .unwrap_or(f64::NAN);
    let rem = clock - (now_millis() as f64 - started);
    // JS `!(rem <= 0)`: NaN is not <= 0, so NaN means "no timeout".
    if rem.is_nan() || rem > 0.0 {
        return false;
    }
    if let Some(obj) = g.as_object_mut() {
        obj.insert("status".into(), json!("over"));
        obj.insert(
            "result".into(),
            json!(if turn == "w" { "0-1" } else { "1-0" }),
        );
        obj.insert("reason".into(), json!("timeout"));
    }
    let white = jsval::string_of(g.get("white"));
    let black = jsval::string_of(g.get("black"));
    let winner = if turn == "w" { &black } else { &white };
    // cvCheckTimeout pays the win only (no draw branch here).
    let bet = g.get("bet").and_then(jsval::number).unwrap_or(0.0);
    let mut win_bonus = 50.0 + bet * 2.0;
    if are_friends(state, &white, &black) {
        win_bonus = (win_bonus * 1.5).floor();
    }
    mitch_lib::coins::add_coins(
        &state.store,
        &state.cfg.data_dir,
        winner,
        win_bonus,
        state.coin_multiplier(),
        "",
    );
    mitch_lib::achievements::update_stat(
        &state.store,
        &state.cfg.data_dir,
        winner,
        "chess_wins",
        1.0,
        state.coin_multiplier(),
    );
    is_corr
}

/// The heartbeat's `myGames` entry (server.js:20499-20501) — `type`/`result`
/// come straight off the game, so an absent/`undefined` value drops the key
/// exactly like `JSON.stringify`.
fn heartbeat_game_entry(g: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for key in ["id", "white", "black", "status", "type", "result"] {
        if let Some(v) = g.get(key) {
            out.insert(key.to_string(), v.clone());
        }
    }
    Value::Object(out)
}

fn heartbeat(state: &Arc<AppState>, my_email: &str) -> axum::response::Response {
    let now = now_millis();
    state
        .cv_online
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(my_email.to_string(), now);
    let challenges: Vec<Value> = state
        .cv_challenges
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .filter(|c| {
            let to = jsval::string_of(c.get("to"));
            let from = jsval::string_of(c.get("from"));
            to == my_email || from == my_email
        })
        .cloned()
        .collect();
    let mut need_save = false;
    let mut games_list: Vec<Value> = Vec::new();
    {
        let mut games = state.cv_games.lock().unwrap_or_else(|e| e.into_inner());
        let mine: Vec<String> = games
            .iter()
            .filter(|(_, g)| {
                let w = jsval::string_of(g.get("white"));
                let b = jsval::string_of(g.get("black"));
                w == my_email || b == my_email
            })
            .map(|(k, _)| k.clone())
            .collect();
        for id in mine {
            if let Some(g) = games.get_mut(&id) {
                need_save |= cv_check_timeout(state, g);
                games_list.push(heartbeat_game_entry(g));
            }
        }
    }
    if need_save {
        let games = state.cv_games.lock().unwrap_or_else(|e| e.into_inner());
        cv_save(state, &games);
    }
    resp(
        200,
        &json!({ "ok": true, "challenges": challenges, "games": games_list }),
    )
}

fn online(state: &Arc<AppState>) -> axum::response::Response {
    let now = now_millis();
    let online: Vec<String> = state
        .cv_online
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|(_, t)| now - *t < 60_000)
        .map(|(e, _)| e.clone())
        .collect();
    resp(200, &json!({ "online": online }))
}

fn challenges(state: &Arc<AppState>, my_email: &str) -> axum::response::Response {
    let mine: Vec<Value> = state
        .cv_challenges
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .filter(|c| {
            let to = jsval::string_of(c.get("to"));
            let from = jsval::string_of(c.get("from"));
            to == my_email || from == my_email
        })
        .cloned()
        .collect();
    resp(200, &json!({ "challenges": mine }))
}

/// `siteUrl(email)` (admin/data.rs shape) — student addresses get the
/// alternate origin when one is set.
fn site_url_of(state: &AppState, email: &str) -> String {
    let norm = mitch_lib::auth::normalize_email(email);
    let site = state
        .store
        .read_document(&state.data_dir().join("site.json"), json!({}));
    let primary = site
        .get("primary")
        .and_then(|v| v.as_str())
        .unwrap_or("https://mitch.pro");
    let alternate = site.get("alternate").and_then(|v| v.as_str()).unwrap_or("");
    if norm.ends_with("student.rjuhsd.us") && !alternate.is_empty() {
        alternate.to_string()
    } else {
        primary.to_string()
    }
}

/// `makeChessCorrActionHtml(email, title, messageText)`
/// (server.js:1902-1915) — no HTML escaping in the JS template, none here.
fn make_chess_corr_action_html(
    state: &AppState,
    email: &str,
    title: &str,
    message_text: &str,
) -> String {
    let game_url = format!("{}/games/chess-bot/", site_url_of(state, email));
    let content = format!(
        "\n    <h2 style=\"margin: 0 0 16px; font-size: 20px; font-weight: 700; color: #fbbf24; text-align: center;\">♟ Chess Correspondence</h2>\n    <div style=\"background-color: rgba(251, 191, 36, 0.08); border: 1px solid rgba(251, 191, 36, 0.25); border-radius: 12px; padding: 20px; text-align: center; margin-bottom: 24px;\">\n      <p style=\"margin: 0 0 8px; font-size: 16px; font-weight: 700; color: #f4f4f5;\">{title}</p>\n      <p style=\"margin: 0; color: #cbd5e1; line-height: 1.6;\">{message_text}</p>\n    </div>\n    <div style=\"text-align: center; margin-bottom: 8px;\">\n      <a href=\"{game_url}\" style=\"display: inline-block; background: linear-gradient(135deg, #a855f7, #6366f1); color: #ffffff; text-decoration: none; padding: 14px 28px; border-radius: 10px; font-weight: 700; box-shadow: 0 10px 20px rgba(168, 85, 247, 0.3);\">Go to Chess Board</a>\n    </div>\n  "
    );
    crate::routes::admin::legacy::html_base_template(state, email, title, &content)
}

fn notify_target(state: &Arc<AppState>, target: &str, title: &str, message: &str, url: &str) {
    mitch_lib::coins::add_admin_notification(
        &state.store,
        &state.cfg.data_dir,
        target,
        title,
        message,
        "admin",
        "",
        url,
    );
    // `triggerNotificationRefresh` (server.js:3118-3125).
    crate::ws::broadcast(
        state,
        crate::ws::WsRecipients::All,
        json!({ "type": "refresh_notifications" }).to_string(),
    );
}

/// The challenge handler (server.js:20509-20553). NOTE: despite generating
/// an id, the JS never writes `cvChallenges[id]` — the challenge is dropped
/// here exactly as it is in the JS.
fn challenge(state: &Arc<AppState>, body: &Value, my_email: &str) -> axum::response::Response {
    // `(body.to || '')` — a missing key is '' (JS), not "undefined".
    let raw_target = jsval::str_or(body.get("to"), "")
        .to_lowercase()
        .trim()
        .to_string();
    // The corr lobby sends a username (members masks emails), the live
    // lobby a raw email — resolveTargetEmail handles either.
    let resolved = mitch_lib::profile::resolve_target_email(
        &state.store,
        &state.cfg.data_dir,
        &state.id_secret,
        &raw_target,
    );
    let to = resolved
        .as_ref()
        .map(|r| mitch_lib::auth::normalize_email(r))
        .unwrap_or_else(|| raw_target.clone());
    if resolved.is_none() {
        return resp(400, &json!({ "error": "target user not found" }));
    }
    if to == mitch_lib::auth::normalize_email(my_email) {
        return resp(400, &json!({ "error": "cannot challenge yourself" }));
    }
    let is_corr = jsval::string_of(body.get("type")) == "corr";
    let tc = build_tc(body, is_corr);
    let p = js_parse_int(body.get("bet").unwrap_or(&Value::Null));
    let bet = if p.is_nan() { 0.0 } else { p.max(0.0) };
    if bet > 0.0 && mitch_lib::coins::get_coins(&state.store, &state.cfg.data_dir, my_email) < bet {
        return resp(
            400,
            &json!({ "error": "You do not have enough coins for this bet" }),
        );
    }

    // Dedupe: every existing challenge of mine to this target is deleted;
    // `alreadyChallenged` only when the type matches. (The map is provably
    // empty — nothing ever stores challenges.)
    let mut already_challenged = false;
    {
        let mut chals = state
            .cv_challenges
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dupes: Vec<String> = chals
            .iter()
            .filter(|(_, c)| {
                jsval::string_of(c.get("from")) == my_email && jsval::string_of(c.get("to")) == to
            })
            .map(|(k, _)| k.clone())
            .collect();
        for id in dupes {
            if let Some(c) = chals.get(&id) {
                if jsval::string_of(c.get("type")) == if is_corr { "corr" } else { "live" } {
                    already_challenged = true;
                }
            }
            chals.shift_remove(&id);
        }
    }

    let id = mitch_lib::crypto::random_bytes_hex(8);
    let my_canonical = mitch_lib::profile::canonical_delivery_email(
        &state.store,
        &state.cfg.data_dir,
        &state.id_secret,
        my_email,
    );
    let my_norm = mitch_lib::auth::normalize_email(my_email);
    let profiles: Value = state
        .store
        .read_document(&state.data_dir().join("profiles.json"), json!({}));
    let prof = profiles.get(&my_norm).cloned().unwrap_or(json!({}));
    let from_name = ["displayName", "nickname", "username"]
        .iter()
        .find_map(|k| prof.get(*k).filter(|v| jsval::truthy(v)).cloned())
        .map(|v| jsval::string(&v))
        .unwrap_or_else(|| my_canonical.split('@').next().unwrap_or("").to_string());
    let target_to = mitch_lib::profile::canonical_delivery_email(
        &state.store,
        &state.cfg.data_dir,
        &state.id_secret,
        &to,
    );
    if is_corr && !already_challenged {
        let per_move = tc.get("perMove").and_then(jsval::number).unwrap_or(0.0);
        let days = per_move / 86_400_000.0;
        let days_str = if days != 1.0 { "s" } else { "" };
        let subject = format!("Chess challenge from {from_name}");
        let message = format!(
            "{from_name} has challenged you to a correspondence chess game ({} day{days_str}/move) with a bet of {} coins.",
            mitch_lib::data::js_number_string(days),
            mitch_lib::data::js_number_string(bet),
        );
        let html = make_chess_corr_action_html(state, &target_to, &subject, &message);
        crate::routes::push::send_email_bg(state, &target_to, &subject, &html);
    }
    let bet_note = if bet > 0.0 {
        format!(" (Bet: {} coins)", mitch_lib::data::js_number_string(bet))
    } else {
        String::new()
    };
    notify_target(
        state,
        &target_to,
        "New Chess Challenge",
        &format!("{from_name} has challenged you to a Chess game{bet_note}."),
        "/games/chess-bot/",
    );
    resp(200, &json!({ "ok": true, "id": id }))
}

/// The respond handler (server.js:20556-20605) — dead in practice (nothing
/// ever stores a challenge), kept in full shape over the empty map.
fn respond(state: &Arc<AppState>, body: &Value, my_email: &str) -> axum::response::Response {
    let challenge_id = jsval::str_or(body.get("challengeId"), "");
    let accept = jsval::truthy(body.get("accept").unwrap_or(&Value::Null));
    let c = {
        let chals = state
            .cv_challenges
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        chals.get(&challenge_id).cloned()
    };
    let Some(c) = c else {
        return resp(404, &json!({ "error": "challenge not found" }));
    };
    if jsval::string_of(c.get("to")) != my_email {
        return resp(403, &json!({ "error": "not your challenge" }));
    }

    let bet = c.get("bet").and_then(jsval::number).unwrap_or(0.0);
    let is_corr_challenge = c_type_is_corr(&c);
    if accept && bet > 0.0 {
        if mitch_lib::coins::get_coins(&state.store, &state.cfg.data_dir, my_email) < bet {
            return resp(
                400,
                &json!({ "error": "You do not have enough coins to accept this bet" }),
            );
        }
        let from = jsval::string_of(c.get("from"));
        if mitch_lib::coins::get_coins(&state.store, &state.cfg.data_dir, &from) < bet {
            state
                .cv_challenges
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .shift_remove(&challenge_id);
            return resp(
                400,
                &json!({ "error": "The challenger no longer has enough coins for this bet. Challenge cancelled." }),
            );
        }
        mitch_lib::coins::add_coins(
            &state.store,
            &state.cfg.data_dir,
            my_email,
            -bet,
            state.coin_multiplier(),
            "chess-vs: bet deduction on start",
        );
        mitch_lib::coins::add_coins(
            &state.store,
            &state.cfg.data_dir,
            &from,
            -bet,
            state.coin_multiplier(),
            "chess-vs: bet deduction on start",
        );
    }

    state
        .cv_challenges
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .shift_remove(&challenge_id);
    let responder_canonical = mitch_lib::profile::canonical_delivery_email(
        &state.store,
        &state.cfg.data_dir,
        &state.id_secret,
        my_email,
    );
    let my_norm = mitch_lib::auth::normalize_email(my_email);
    let profiles: Value = state
        .store
        .read_document(&state.data_dir().join("profiles.json"), json!({}));
    let prof = profiles.get(&my_norm).cloned().unwrap_or(json!({}));
    let responder_name = ["displayName", "nickname", "username"]
        .iter()
        .find_map(|k| prof.get(*k).filter(|v| jsval::truthy(v)).cloned())
        .map(|v| jsval::string(&v))
        .unwrap_or_else(|| {
            responder_canonical
                .split('@')
                .next()
                .unwrap_or("")
                .to_string()
        });
    let from = jsval::string_of(c.get("from"));
    let challenger_target = mitch_lib::profile::canonical_delivery_email(
        &state.store,
        &state.cfg.data_dir,
        &state.id_secret,
        &from,
    );
    if !accept {
        notify_target(
            state,
            &challenger_target,
            "Chess Challenge Declined",
            &format!("{responder_name} declined your Chess challenge."),
            "/games/chess-bot/",
        );
        return resp(200, &json!({ "ok": true, "declined": true }));
    }
    notify_target(
        state,
        &challenger_target,
        "Chess Challenge Accepted",
        &format!("{responder_name} accepted your Chess challenge!"),
        "/games/chess-bot/",
    );
    // `Math.random() < 0.5` picks the colors; either side is valid.
    let white = if rand_half() {
        from.clone()
    } else {
        my_email.to_string()
    };
    let black = if white == from {
        my_email.to_string()
    } else {
        from.clone()
    };
    let game_id = mitch_lib::crypto::random_bytes_hex(8);
    let now = now_millis();
    let tc = c.get("tc").cloned().unwrap_or(json!({}));
    let clock = if c_type_is_corr(&c) {
        tc.get("perMove")
            .and_then(jsval::number)
            .unwrap_or(f64::NAN)
    } else {
        tc.get("initial")
            .and_then(jsval::number)
            .unwrap_or(f64::NAN)
    };
    // `type: "c".type` evaluates to undefined — the key is DROPPED from the
    // serialized game (server.js:20593). `clockStartedAt` is null for corr
    // challenges and dropped (undefined) for live ones.
    let mut game = json!({
        "id": game_id, "white": white, "black": black,
        "tc": tc, "bet": bet,
        "fen": CHESS_VS_STARTING_FEN, "moves": [], "status": "active",
        "result": Value::Null, "reason": Value::Null, "turn": "w",
        "clocks": {}, // filled below — an undefined clock drops the key
        "lastMoveAt": now, "createdAt": now,
    });
    if !clock.is_nan() {
        let clocks = game
            .get_mut("clocks")
            .and_then(|c| c.as_object_mut())
            .unwrap_or_else(|| unreachable!());
        clocks.insert("w".into(), num_json(clock));
        clocks.insert("b".into(), num_json(clock));
    }
    if is_corr_challenge {
        game.as_object_mut()
            .unwrap_or_else(|| unreachable!())
            .insert("clockStartedAt".into(), Value::Null);
    }
    state
        .cv_games
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(game_id.clone(), game);
    state
        .cv_chats
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(game_id.clone(), json!([]));
    if is_corr_challenge {
        let games = state.cv_games.lock().unwrap_or_else(|e| e.into_inner());
        cv_save(state, &games);
    }
    resp(200, &json!({ "ok": true, "gameId": game_id }))
}

fn c_type_is_corr(c: &Value) -> bool {
    jsval::string_of(c.get("type")) == "corr"
}

fn rand_half() -> bool {
    mitch_lib::crypto::random_bytes_hex(1)
        .chars()
        .next()
        .and_then(|c| c.to_digit(16))
        .is_some_and(|d| d % 2 == 0)
}

// ── Legacy-game handlers (reachable via boot-loaded chess_vs.json) ──────────

fn game_view(state: &Arc<AppState>, search: &str, my_email: &str) -> axum::response::Response {
    let game_id = crate::handler::query(search)
        .get("id")
        .cloned()
        .unwrap_or_default();
    let mut need_save = false;
    let snapshot;
    {
        let mut games = state.cv_games.lock().unwrap_or_else(|e| e.into_inner());
        let Some(g) = games.get_mut(&game_id) else {
            return resp(404, &json!({ "error": "game not found" }));
        };
        if !is_member(g, my_email) {
            return resp(403, &json!({ "error": "not your game" }));
        }
        if jsval::string_of(g.get("status")) == "active" {
            // First GET by the turn holder starts the corr clock.
            let is_corr = g.get("type").and_then(|v| v.as_str()) == Some("corr");
            if is_corr && g.get("clockStartedAt") == Some(&Value::Null) {
                let my_color = if jsval::string_of(g.get("white")) == my_email {
                    "w"
                } else {
                    "b"
                };
                if jsval::string_of(g.get("turn")) == my_color {
                    if let Some(obj) = g.as_object_mut() {
                        obj.insert("clockStartedAt".into(), json!(now_millis()));
                    }
                    need_save = true;
                }
            }
            need_save |= cv_check_timeout(state, g);
        }
        snapshot = g.clone();
    }
    if need_save {
        let games = state.cv_games.lock().unwrap_or_else(|e| e.into_inner());
        cv_save(state, &games);
    }
    let chat = state
        .cv_chats
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&game_id)
        .cloned()
        .unwrap_or(json!([]));
    resp(200, &json!({ "game": snapshot, "chat": chat }))
}

fn is_member(g: &Value, my_email: &str) -> bool {
    jsval::string_of(g.get("white")) == my_email || jsval::string_of(g.get("black")) == my_email
}

fn my_color_of(g: &Value, my_email: &str) -> &'static str {
    if jsval::string_of(g.get("white")) == my_email {
        "w"
    } else {
        "b"
    }
}

fn other_color(c: &str) -> &'static str {
    if c == "w" {
        "b"
    } else {
        "w"
    }
}

/// The over-game payout ladder (server.js:20669-20684; resign 20716-20726;
/// cvCheckTimeout 869-876). `with_reason` selects the move-handler reason
/// strings — the resign/timeout/draw handlers pay without one.
fn apply_payouts(state: &Arc<AppState>, g: &Value, with_reason: bool) {
    let result = jsval::string_of(g.get("result"));
    let white = jsval::string_of(g.get("white"));
    let black = jsval::string_of(g.get("black"));
    let bet = g.get("bet").and_then(jsval::number).unwrap_or(0.0);
    let mut win_bonus = 50.0 + bet * 2.0;
    let mut draw_bonus = 10.0 + bet;
    if are_friends(state, &white, &black) {
        win_bonus = (win_bonus * 1.5).floor();
        draw_bonus = (draw_bonus * 1.5).floor();
    }
    let bet_str = mitch_lib::data::js_number_string(bet);
    let win_reason = format!("chess-vs: win payout (bet={bet_str})");
    let draw_reason = format!("chess-vs: draw payout (bet={bet_str})");
    let empty = String::new();
    let (win_reason, draw_reason) = if with_reason {
        (&win_reason, &draw_reason)
    } else {
        (&empty, &empty)
    };
    let winner = if result == "1-0" {
        Some(&white)
    } else if result == "0-1" {
        Some(&black)
    } else {
        None
    };
    if let Some(winner) = winner {
        mitch_lib::coins::add_coins(
            &state.store,
            &state.cfg.data_dir,
            winner,
            win_bonus,
            state.coin_multiplier(),
            win_reason,
        );
        mitch_lib::achievements::update_stat(
            &state.store,
            &state.cfg.data_dir,
            winner,
            "chess_wins",
            1.0,
            state.coin_multiplier(),
        );
    } else if result == "1/2-1/2" {
        mitch_lib::coins::add_coins(
            &state.store,
            &state.cfg.data_dir,
            &white,
            draw_bonus,
            state.coin_multiplier(),
            draw_reason,
        );
        mitch_lib::coins::add_coins(
            &state.store,
            &state.cfg.data_dir,
            &black,
            draw_bonus,
            state.coin_multiplier(),
            draw_reason,
        );
    }
}

/// `myDisplayName` shape (server.js:20538-20544) — profile displayName ||
/// nickname || username || the canonical local part.
fn display_name_of(state: &AppState, email: &str) -> String {
    let canonical = mitch_lib::profile::canonical_delivery_email(
        &state.store,
        &state.cfg.data_dir,
        &state.id_secret,
        email,
    );
    let norm = mitch_lib::auth::normalize_email(email);
    let profiles: Value = state
        .store
        .read_document(&state.data_dir().join("profiles.json"), json!({}));
    let prof = profiles.get(&norm).cloned().unwrap_or(json!({}));
    ["displayName", "nickname", "username"]
        .iter()
        .find_map(|k| prof.get(*k).filter(|v| jsval::truthy(v)).cloned())
        .map(|v| jsval::string(&v))
        .unwrap_or_else(|| canonical.split('@').next().unwrap_or("").to_string())
}

/// `g.clocks[myColor]` read with JS number coercion (missing → NaN).
fn clocks_get(g: &Value, color: &str) -> f64 {
    g.get("clocks")
        .and_then(|c| c.get(color))
        .and_then(jsval::number)
        .unwrap_or(f64::NAN)
}

fn tc_per_move_opt(g: &Value) -> Option<f64> {
    g.get("tc")
        .and_then(|tc| tc.get("perMove"))
        .and_then(jsval::number)
}

/// The clock update of `/move` (server.js:20647-20659). Returns `true` when
/// the mover's clock hit zero — the caller marks the game over/timeout.
fn apply_clocks(g: &mut Value, my_color: &str, is_live: bool, now: i64) -> bool {
    let last = g
        .get("lastMoveAt")
        .and_then(jsval::number)
        .unwrap_or(f64::NAN);
    let (elapsed, is_corr) = if is_live {
        (now as f64 - last, false)
    } else {
        // `g.clockStartedAt ?? g.lastMoveAt` — ?? only skips null/undefined.
        let started = match g.get("clockStartedAt") {
            Some(v) if !v.is_null() => jsval::number(v).unwrap_or(f64::NAN),
            _ => last,
        };
        (now as f64 - started, true)
    };
    let mut updated = js_max(0.0, clocks_get(g, my_color) - elapsed);
    if is_live {
        // `(g.tc.increment || 0)` — NaN is falsy in JS, so NaN → 0.
        let increment = {
            let inc = g
                .get("tc")
                .and_then(|tc| tc.get("increment"))
                .and_then(jsval::number)
                .unwrap_or(f64::NAN);
            if inc.is_nan() {
                0.0
            } else {
                inc
            }
        };
        updated += increment;
    }
    let timed_out = updated <= 0.0; // false for NaN, like the JS
    let per_move = tc_per_move_opt(g);
    if let Some(clocks) = g.get_mut("clocks").and_then(|c| c.as_object_mut()) {
        // JS assigns the computed number even when it is NaN (undefined clock
        // − elapsed); the key survives in memory but JSON.stringify drops it,
        // so every later read sees undefined → NaN — emulate by removing the
        // key rather than writing null (Number(null) is 0, not NaN).
        if updated.is_nan() {
            clocks.shift_remove(my_color);
        } else {
            clocks.insert(my_color.to_string(), num_json(updated));
        }
        if is_corr && !timed_out {
            // `g.clocks[opp] = g.tc.perMove` — an undefined perMove leaves an
            // own property JSON.stringify drops, so remove the key instead.
            let opp = other_color(my_color).to_string();
            match per_move {
                Some(x) => {
                    clocks.insert(opp, num_json(x));
                }
                None => {
                    clocks.shift_remove(&opp);
                }
            }
        }
    }
    if is_corr {
        if let Some(obj) = g.as_object_mut() {
            obj.insert("clockStartedAt".into(), Value::Null);
        }
    }
    timed_out
}

/// The `/move` handler (server.js:20607-20694) — live code for boot-loaded
/// games.
fn move_piece(state: &Arc<AppState>, body: &Value, my_email: &str) -> axum::response::Response {
    let game_id = jsval::str_or(body.get("gameId"), "");
    let out;
    let mut need_save = false;
    let mut corr_email: Option<(String, String, String, bool, String)> = None;
    {
        let mut games = state.cv_games.lock().unwrap_or_else(|e| e.into_inner());
        let Some(g) = games.get_mut(&game_id) else {
            return resp(404, &json!({ "error": "game not found" }));
        };
        if !is_member(g, my_email) {
            return resp(403, &json!({ "error": "not your game" }));
        }
        if jsval::string_of(g.get("status")) != "active" {
            return resp(400, &json!({ "error": "game over" }));
        }
        need_save |= cv_check_timeout(state, g);
        if jsval::string_of(g.get("status")) != "active" {
            // `return jsonResp(200, { ok: false, game: g })` — 200.
            out = g.clone();
            drop(games);
            if need_save {
                let games = state.cv_games.lock().unwrap_or_else(|e| e.into_inner());
                cv_save(state, &games);
            }
            return resp(200, &json!({ "ok": false, "game": out }));
        }
        let my_color = my_color_of(g, my_email);
        if jsval::string_of(g.get("turn")) != my_color {
            return resp(400, &json!({ "error": "not your turn" }));
        }

        // The boost runs BEFORE the move/fen validation — a boost with a
        // missing move still sinks the 500 coins and adds the 30 seconds.
        if jsval::truthy(body.get("boostClock").unwrap_or(&Value::Null)) {
            if mitch_lib::coins::get_coins(&state.store, &state.cfg.data_dir, my_email) < 500.0 {
                return resp(
                    400,
                    &json!({ "error": "Insufficient coins for clock boost" }),
                );
            }
            mitch_lib::coins::add_coins(
                &state.store,
                &state.cfg.data_dir,
                my_email,
                -500.0,
                state.coin_multiplier(),
                "",
            );
            let clock = clocks_get(g, my_color);
            // `g.clocks[myColor] += 30000` — NaN (undefined clock) drops the
            // key on the next stringify; remove it rather than writing null.
            if let Some(clocks) = g.get_mut("clocks").and_then(|c| c.as_object_mut()) {
                let boosted = clock + 30_000.0;
                if boosted.is_nan() {
                    clocks.shift_remove(my_color);
                } else {
                    clocks.insert(my_color.to_string(), num_json(boosted));
                }
            }
        }

        // `(body.move || '').slice(0, 10)` — string_of alone would yield the
        // string "undefined" for a missing key (it did: a boost-only request
        // once pushed the move "undefined" instead of 400ing).
        let mv = jsval::js_slice_utf16(&jsval::str_or(body.get("move"), ""), 10);
        let new_fen = jsval::js_slice_utf16(&jsval::str_or(body.get("fen"), ""), 200);
        if mv.is_empty() || new_fen.is_empty() {
            return resp(400, &json!({ "error": "missing move/fen" }));
        }
        let now = now_millis();
        let is_corr = g.get("type").and_then(|v| v.as_str()) == Some("corr");
        let is_live = g.get("type").and_then(|v| v.as_str()) == Some("live");
        let timed_out = apply_clocks(g, my_color, is_live, now);
        if timed_out {
            if let Some(obj) = g.as_object_mut() {
                obj.insert("status".into(), json!("over"));
                obj.insert(
                    "result".into(),
                    json!(if my_color == "w" { "0-1" } else { "1-0" }),
                );
                obj.insert("reason".into(), json!("timeout"));
            }
        }
        if jsval::string_of(g.get("status")) == "active" {
            let flipped = if jsval::string_of(g.get("turn")) == "w" {
                "b"
            } else {
                "w"
            };
            if let Some(obj) = g.as_object_mut() {
                if let Some(moves) = obj.get_mut("moves").and_then(|m| m.as_array_mut()) {
                    moves.push(json!(mv));
                }
                obj.insert("fen".into(), json!(new_fen));
                obj.insert("turn".into(), json!(flipped));
                obj.shift_remove("drawOffer");
                if jsval::truthy(body.get("result").unwrap_or(&Value::Null)) {
                    obj.insert("status".into(), json!("over"));
                    obj.insert(
                        "result".into(),
                        body.get("result").cloned().unwrap_or(Value::Null),
                    );
                    let reason = body.get("reason").filter(|v| jsval::truthy(v)).cloned();
                    obj.insert(
                        "reason".into(),
                        reason.unwrap_or_else(|| json!("checkmate")),
                    );
                }
            }
        }
        if let Some(obj) = g.as_object_mut() {
            obj.insert("lastMoveAt".into(), json!(now));
        }
        if jsval::string_of(g.get("status")) == "over" {
            apply_payouts(state, g, true);
        }
        if is_corr {
            need_save = true;
            let opp_email = if my_color == "w" {
                jsval::string_of(g.get("black"))
            } else {
                jsval::string_of(g.get("white"))
            };
            let over = jsval::string_of(g.get("status")) == "over";
            let result_str = jsval::string_of(g.get("result"));
            let from_name = display_name_of(state, my_email);
            corr_email = Some((opp_email, from_name, result_str, over, my_color.to_string()));
        }
        out = g.clone();
    }
    if need_save {
        let games = state.cv_games.lock().unwrap_or_else(|e| e.into_inner());
        cv_save(state, &games);
    }
    if let Some((opp_email, from_name, result_str, over, _)) = corr_email {
        let opp_target = mitch_lib::profile::canonical_delivery_email(
            &state.store,
            &state.cfg.data_dir,
            &state.id_secret,
            &opp_email,
        );
        let subject = if over {
            format!("Chess game over — {from_name} played the final move")
        } else {
            format!("{from_name} played a move in your correspondence game")
        };
        let message = if over {
            format!("Result: {result_str}.")
        } else {
            "It's your turn!".to_string()
        };
        let html = make_chess_corr_action_html(state, &opp_target, &subject, &message);
        crate::routes::push::send_email_bg(state, &opp_target, &subject, &html);
    }
    resp(200, &json!({ "ok": true, "game": out }))
}

/// The `/resign` handler (server.js:20696-20728).
fn resign(state: &Arc<AppState>, body: &Value, my_email: &str) -> axum::response::Response {
    let game_id = jsval::str_or(body.get("gameId"), "");
    let out;
    // No cvCheckTimeout in JS resign — save only when we mutate (corr).
    let need_save;
    {
        let mut games = state.cv_games.lock().unwrap_or_else(|e| e.into_inner());
        let Some(g) = games.get_mut(&game_id) else {
            return resp(403, &json!({}));
        };
        if !is_member(g, my_email) {
            return resp(403, &json!({}));
        }
        if jsval::string_of(g.get("status")) != "active" {
            return resp(400, &json!({ "error": "game already over" }));
        }
        let my_color = my_color_of(g, my_email);
        let result = if my_color == "w" { "0-1" } else { "1-0" };
        if let Some(obj) = g.as_object_mut() {
            obj.insert("status".into(), json!("over"));
            obj.insert("result".into(), json!(result));
            obj.insert("reason".into(), json!("resign"));
        }
        // `winner = g.result === '1-0' ? g.white : g.black` — always one of
        // the two here.
        let winner = if result == "1-0" {
            jsval::string_of(g.get("white"))
        } else {
            jsval::string_of(g.get("black"))
        };
        let bet = g.get("bet").and_then(jsval::number).unwrap_or(0.0);
        let mut win_bonus = 50.0 + bet * 2.0;
        if are_friends(
            state,
            &jsval::string_of(g.get("white")),
            &jsval::string_of(g.get("black")),
        ) {
            win_bonus = (win_bonus * 1.5).floor();
        }
        mitch_lib::coins::add_coins(
            &state.store,
            &state.cfg.data_dir,
            &winner,
            win_bonus,
            state.coin_multiplier(),
            "",
        );
        mitch_lib::achievements::update_stat(
            &state.store,
            &state.cfg.data_dir,
            &winner,
            "chess_wins",
            1.0,
            state.coin_multiplier(),
        );
        need_save = g.get("type").and_then(|v| v.as_str()) == Some("corr");
        out = g.clone();
    }
    if need_save {
        let games = state.cv_games.lock().unwrap_or_else(|e| e.into_inner());
        cv_save(state, &games);
    }
    resp(200, &json!({ "ok": true, "game": out }))
}

/// The `/draw` handler (server.js:20730-20752).
fn draw(state: &Arc<AppState>, body: &Value, my_email: &str) -> axum::response::Response {
    let game_id = jsval::str_or(body.get("gameId"), "");
    let out;
    let mut need_save = false;
    {
        let mut games = state.cv_games.lock().unwrap_or_else(|e| e.into_inner());
        let Some(g) = games.get_mut(&game_id) else {
            return resp(403, &json!({}));
        };
        if !is_member(g, my_email) {
            return resp(403, &json!({}));
        }
        if jsval::string_of(g.get("status")) != "active" {
            return resp(400, &json!({}));
        }
        if jsval::truthy(body.get("offer").unwrap_or(&Value::Null)) {
            if let Some(obj) = g.as_object_mut() {
                obj.insert("drawOffer".into(), json!(my_email));
            }
            // The offer path returns WITHOUT the game object and does not
            // cvSave.
            out = json!({ "ok": true });
        } else {
            if jsval::truthy(body.get("accept").unwrap_or(&Value::Null))
                && draw_offer_truthy(g)
                && jsval::string_of(g.get("drawOffer")) != my_email
            {
                if let Some(obj) = g.as_object_mut() {
                    obj.insert("status".into(), json!("over"));
                    obj.insert("result".into(), json!("1/2-1/2"));
                    obj.insert("reason".into(), json!("draw"));
                    obj.shift_remove("drawOffer");
                }
                apply_payouts(state, g, false);
                need_save = g.get("type").and_then(|v| v.as_str()) == Some("corr");
            } else if let Some(obj) = g.as_object_mut() {
                obj.shift_remove("drawOffer");
            }
            out = json!({ "ok": true, "game": g.clone() });
        }
    }
    if need_save {
        let games = state.cv_games.lock().unwrap_or_else(|e| e.into_inner());
        cv_save(state, &games);
    }
    resp_body(&out)
}

/// `drawOffer` truthiness — `g.drawOffer &&` in the JS accept condition.
fn draw_offer_truthy(g: &Value) -> bool {
    match g.get("drawOffer") {
        Some(v) => jsval::truthy(v),
        None => false,
    }
}

/// The `/chat` handler (server.js:20742-20752) — works on over games too
/// (no status check in the JS).
fn chat(state: &Arc<AppState>, body: &Value, my_email: &str) -> axum::response::Response {
    let game_id = jsval::string_of(body.get("gameId"));
    {
        let games = state.cv_games.lock().unwrap_or_else(|e| e.into_inner());
        let Some(g) = games.get(&game_id) else {
            return resp(403, &json!({}));
        };
        if !is_member(g, my_email) {
            return resp(403, &json!({}));
        }
    }
    // `(body.text || '').trim().slice(0, 500)` — a missing key is ''.
    let text = jsval::js_slice_utf16(jsval::str_or(body.get("text"), "").trim(), 500);
    if text.is_empty() {
        return resp(400, &json!({}));
    }
    {
        let mut chats = state.cv_chats.lock().unwrap_or_else(|e| e.into_inner());
        let entry = chats
            .entry(game_id)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap_or_else(|| unreachable!());
        entry.push(json!({ "from": my_email, "text": text, "ts": now_millis() }));
        if entry.len() > 200 {
            let excess = entry.len() - 200;
            entry.drain(0..excess);
        }
    }
    resp(200, &json!({ "ok": true }))
}

fn resp_body(body: &Value) -> axum::response::Response {
    resp(200, body)
}

fn is_revoked_id(state: &AppState, sid: &str) -> bool {
    state
        .store
        .read_document(&state.data_dir().join("revoked.json"), json!({}))
        .get(sid)
        .is_some()
}

fn are_friends(state: &AppState, a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let norm_a = mitch_lib::auth::normalize_email(a);
    let norm_b = mitch_lib::auth::normalize_email(b);
    let friends: Value = state
        .store
        .read_document(&state.data_dir().join("friends.json"), json!({}));
    let has = |norm: &str, target: &str| {
        friends
            .get(norm)
            .and_then(|v| v.as_array())
            .is_some_and(|arr| {
                arr.iter()
                    .any(|f| mitch_lib::auth::normalize_email(f.as_str().unwrap_or("")) == target)
            })
    };
    has(&norm_a, &norm_b) || has(&norm_b, &norm_a)
}

// ── Shared prelude ───────────────────────────────────────────────────────────

const KNOWN: [&str; 10] = [
    "/api/chess-vs/heartbeat",
    "/api/chess-vs/online",
    "/api/chess-vs/challenges",
    "/api/chess-vs/challenge",
    "/api/chess-vs/respond",
    "/api/chess-vs/game",
    "/api/chess-vs/move",
    "/api/chess-vs/resign",
    "/api/chess-vs/draw",
    "/api/chess-vs/chat",
];

/// Paths that `tryParseJson` (empty body → `{}`, unparseable → 400).
const PARSE_BODY: [&str; 6] = [
    "/api/chess-vs/challenge",
    "/api/chess-vs/respond",
    "/api/chess-vs/move",
    "/api/chess-vs/resign",
    "/api/chess-vs/draw",
    "/api/chess-vs/chat",
];

pub(crate) async fn handle(
    state: &Arc<AppState>,
    _method: &axum::http::Method,
    path: &str,
    headers: &HeaderMap,
    search: &str,
    body_bytes: &[u8],
) -> Option<axum::response::Response> {
    if !path.starts_with("/api/chess-vs/") || !KNOWN.contains(&path) {
        return None;
    }
    // Auth ladder (server.js:20485-20491) — NO rate limiting: the JS block
    // has zero checkRateLimit calls (the RATE_LIMITS entries at 1593-1594
    // are dead config; the handler.rs global gate exempts this prefix).
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cookies.get("id").unwrap_or(""))
        .to_string();
    if sid.is_empty()
        || !mitch_lib::auth::valid_id(&sid, &state.id_secret)
        || is_revoked_id(state, &sid)
    {
        return Some(resp(401, &json!({ "error": "auth required" })));
    }
    let my_email = mitch_lib::auth::names_get_email(&state.store, &sid)
        .unwrap_or_default()
        .to_lowercase();
    if my_email.is_empty() {
        return Some(resp(403, &json!({ "error": "email not found" })));
    }

    // `tryParseJson` (server.js:10514): an EMPTY body parses as `{}` (the
    // handler proceeds), unparseable text is `bad json`; the four GET-shaped
    // paths never parse a body.
    let body = if !PARSE_BODY.contains(&path) || body_bytes.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice(body_bytes) {
            Ok(b) => b,
            Err(_) => return Some(resp(400, &json!({ "error": "bad json" }))),
        }
    };

    Some(match path {
        "/api/chess-vs/heartbeat" => heartbeat(state, &my_email),
        "/api/chess-vs/online" => online(state),
        "/api/chess-vs/challenges" => challenges(state, &my_email),
        "/api/chess-vs/challenge" => challenge(state, &body, &my_email),
        "/api/chess-vs/respond" => respond(state, &body, &my_email),
        "/api/chess-vs/game" => game_view(state, search, &my_email),
        "/api/chess-vs/move" => move_piece(state, &body, &my_email),
        "/api/chess-vs/resign" => resign(state, &body, &my_email),
        "/api/chess-vs/draw" => draw(state, &body, &my_email),
        "/api/chess-vs/chat" => chat(state, &body, &my_email),
        _ => return None,
    })
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_max_propagates_nan() {
        // Rust's f64::max(1.0, NaN) is 1.0; Math.max is NaN.
        assert!(js_max(1.0, f64::NAN).is_nan());
        assert!(js_max(f64::NAN, 1.0).is_nan());
        assert_eq!(js_max(0.0, 3.0), 3.0);
        assert_eq!(js_max(-2.0, -5.0), -2.0);
    }

    #[test]
    fn num_json_nulls_nan() {
        assert_eq!(num_json(f64::NAN), Value::Null);
        assert_eq!(num_json(300.0).as_f64(), Some(300.0));
        assert_eq!(num_json(0.25).as_f64(), Some(0.25));
    }

    #[test]
    fn js_parse_int_matches_js() {
        assert_eq!(js_parse_int(&json!("300")), 300.0);
        assert_eq!(js_parse_int(&json!("300.7")), 300.0); // parseInt("300.7")
        assert_eq!(js_parse_int(&json!(300.7)), 300.0); // stringified first
        assert_eq!(js_parse_int(&json!(0.5)), 0.0);
        assert_eq!(js_parse_int(&json!("  42px")), 42.0); // leading ws + prefix
        assert_eq!(js_parse_int(&json!("-5")), -5.0);
        assert_eq!(js_parse_int(&json!(5)), 5.0);
        assert!(js_parse_int(&json!("")).is_nan());
        assert!(js_parse_int(&json!("nope")).is_nan());
        assert!(js_parse_int(&json!(null)).is_nan()); // objects/arrays → NaN
        assert!(js_parse_int(&json!(true)).is_nan());
        assert!(js_parse_int(&json!({"a": 1})).is_nan());
    }

    #[test]
    fn clamp_parsed_falls_back_and_clamps() {
        // NaN/0 (falsy) → fallback, then the Math.min/max clamp.
        assert_eq!(clamp_parsed(&json!("abc"), 300.0, 30.0, 600.0), 300.0);
        assert_eq!(clamp_parsed(&json!(0), 300.0, 30.0, 600.0), 300.0);
        assert_eq!(clamp_parsed(&json!(0.4), 300.0, 30.0, 600.0), 300.0);
        // In-range value passes through.
        assert_eq!(clamp_parsed(&json!("120"), 300.0, 30.0, 600.0), 120.0);
        // Below the floor → floor; above the ceiling → ceiling.
        assert_eq!(clamp_parsed(&json!("5"), 300.0, 30.0, 600.0), 30.0);
        assert_eq!(clamp_parsed(&json!("5000"), 300.0, 30.0, 600.0), 600.0);
        assert_eq!(
            clamp_parsed(&json!("-50"), 86_400_000.0, 3_600_000.0, 604_800_000.0),
            3_600_000.0
        );
    }

    #[test]
    fn build_tc_shapes() {
        // corr: {perMove} clamped to [3600000, 604800000], default 86400000.
        let tc = build_tc(&json!({ "perMove": "7200000" }), true);
        assert_eq!(tc, json!({ "perMove": 7_200_000.0 }));
        let tc = build_tc(&json!({}), true);
        assert_eq!(tc, json!({ "perMove": 86_400_000.0 }));
        // live: {initial, increment} clamped to [30000,600000] / [0,30000].
        let tc = build_tc(&json!({ "initial": "120000", "increment": "5000" }), false);
        assert_eq!(tc, json!({ "initial": 120_000.0, "increment": 5_000.0 }));
        let tc = build_tc(&json!({}), false);
        assert_eq!(tc, json!({ "initial": 300_000.0, "increment": 0.0 }));
        // increment NaN → fallback 0, initial above ceiling → 600000.
        let tc = build_tc(&json!({ "initial": "999999", "increment": "bogus" }), false);
        assert_eq!(tc, json!({ "initial": 600_000.0, "increment": 0.0 }));
    }

    #[test]
    fn c_type_is_corr_strict() {
        assert!(c_type_is_corr(&json!({ "type": "corr" })));
        assert!(!c_type_is_corr(&json!({})));
        assert!(!c_type_is_corr(&json!({ "type": "live" })));
        assert!(!c_type_is_corr(&json!({ "type": null })));
    }

    #[test]
    fn membership_and_colors() {
        let g = json!({ "white": "a@x.com", "black": "b@x.com" });
        assert!(is_member(&g, "a@x.com"));
        assert!(is_member(&g, "b@x.com"));
        assert!(!is_member(&g, "c@x.com"));
        assert_eq!(my_color_of(&g, "a@x.com"), "w");
        assert_eq!(my_color_of(&g, "b@x.com"), "b");
        assert_eq!(my_color_of(&g, "c@x.com"), "b"); // black default
        assert_eq!(other_color("w"), "b");
        assert_eq!(other_color("b"), "w");
    }

    #[test]
    fn clocks_and_per_move_reads() {
        let g = json!({ "clocks": { "w": 300000, "b": null }, "tc": { "perMove": 86400000 } });
        assert_eq!(clocks_get(&g, "w"), 300_000.0);
        // Number(null) is 0 in JS.
        assert_eq!(clocks_get(&g, "b"), 0.0);
        assert!(clocks_get(&g, "z").is_nan()); // absent key → NaN
        assert_eq!(tc_per_move_opt(&g), Some(86_400_000.0));
        // Missing perMove key (the `?? ` undefined case).
        let g2 = json!({ "tc": {} });
        assert_eq!(tc_per_move_opt(&g2), None);
    }

    #[test]
    fn apply_clocks_live_adds_increment() {
        let mut g = json!({
            "clocks": { "w": 300000, "b": 300000 },
            "tc": { "initial": 300000, "increment": 5000 },
            "lastMoveAt": 1000000,
        });
        let timed_out = apply_clocks(&mut g, "w", true, 1_005_000);
        assert!(!timed_out);
        // max(0, 300000 - 5000) + 5000 = 300000.
        assert_eq!(clocks_get(&g, "w"), 300_000.0);
        // Live games do not touch clocks[opp] or clockStartedAt.
        assert_eq!(clocks_get(&g, "b"), 300_000.0);
        assert!(g.get("clockStartedAt").is_none());
    }

    #[test]
    fn apply_clocks_live_nan_increment_is_falsy() {
        let mut g = json!({
            "clocks": { "w": 100000 },
            "tc": { "initial": 300000 }, // no increment key
            "lastMoveAt": 1000000,
        });
        apply_clocks(&mut g, "w", true, 1_010_000);
        // max(0, 100000-10000) + 0 = 90000.
        assert_eq!(clocks_get(&g, "w"), 90_000.0);
    }

    #[test]
    fn apply_clocks_corr_uses_clock_started_at() {
        let mut g = json!({
            "type": "corr",
            "clocks": { "w": 86400000, "b": 86400000 },
            "tc": { "perMove": 86400000 },
            "clockStartedAt": 1000000,
            "lastMoveAt": 900000,
        });
        let timed_out = apply_clocks(&mut g, "w", false, 2_000_000);
        assert!(!timed_out);
        // max(0, 86400000 - (2000000 - 1000000)) = 85400000.
        assert_eq!(clocks_get(&g, "w"), 85_400_000.0);
        assert_eq!(clocks_get(&g, "b"), 86_400_000.0); // opp = tc.perMove
                                                       // clockStartedAt = null after (JS null, not absent).
        assert_eq!(g.get("clockStartedAt"), Some(&Value::Null));
    }

    #[test]
    fn apply_clocks_corr_no_per_move_removes_opp_key() {
        // undefined perMove → JSON.stringify drops the key.
        let mut g = json!({
            "type": "corr",
            "clocks": { "w": 5_000_000, "b": 5_000_000 },
            "tc": {},
            "clockStartedAt": 1_000_000,
        });
        let timed_out = apply_clocks(&mut g, "w", false, 2_000_000);
        assert!(!timed_out);
        let clocks = g.get("clocks").unwrap();
        assert!(clocks.get("b").is_none());
        assert!(clocks.get("w").is_some());
    }

    #[test]
    fn apply_clocks_timeout_flag() {
        let mut g = json!({
            "clocks": { "w": 3000, "b": 5000 },
            "tc": { "perMove": 5000 },
            "clockStartedAt": 1000000,
        });
        assert!(apply_clocks(&mut g, "w", false, 2_000_000)); // 3000 - 1e6 < 0
                                                              // max(0, negative) is 0, not NaN.
        assert_eq!(clocks_get(&g, "w"), 0.0);
    }

    #[test]
    fn apply_clocks_corr_falls_back_to_last_move_at() {
        // clockStartedAt null → ?? skips it → lastMoveAt.
        let mut g = json!({
            "type": "corr",
            "clocks": { "w": 600000, "b": 600000 },
            "tc": { "perMove": 600000 },
            "clockStartedAt": Value::Null,
            "lastMoveAt": 1_000_000,
        });
        apply_clocks(&mut g, "w", false, 1_100_000);
        assert_eq!(clocks_get(&g, "w"), 500_000.0);
    }

    #[test]
    fn heartbeat_entry_drops_absent_keys() {
        // A respond-created game (no type key) drops type; a legacy game
        // keeps type/result.
        let no_type = json!({ "id": "g1", "white": "a@x", "black": "b@x", "status": "active" });
        let e = heartbeat_game_entry(&no_type);
        assert!(e.get("type").is_none() && e.get("result").is_none());
        assert_eq!(
            e,
            json!({ "id": "g1", "white": "a@x", "black": "b@x", "status": "active" })
        );
        let legacy = json!({
            "id": "g2", "white": "a@x", "black": "b@x", "status": "active",
            "type": "corr", "result": Value::Null,
        });
        let e = heartbeat_game_entry(&legacy);
        assert_eq!(e.get("type"), Some(&json!("corr")));
        // result: null is a present key — kept.
        assert_eq!(e.get("result"), Some(&Value::Null));
    }

    #[test]
    fn draw_offer_truthy_is_js_truthiness() {
        assert!(draw_offer_truthy(&json!({ "drawOffer": "x@y" })));
        assert!(draw_offer_truthy(&json!({ "drawOffer": 1 })));
        assert!(draw_offer_truthy(&json!({ "drawOffer": true })));
        assert!(!draw_offer_truthy(&json!({ "drawOffer": "" })));
        assert!(!draw_offer_truthy(&json!({ "drawOffer": 0 })));
        assert!(!draw_offer_truthy(&json!({ "drawOffer": false })));
        assert!(!draw_offer_truthy(&json!({ "drawOffer": Value::Null })));
        assert!(!draw_offer_truthy(&json!({})));
    }

    #[test]
    fn tc_falsy_guard_matches_js() {
        // cvCheckTimeout's `if (!g.tc)` — null/missing are falsy, an object
        // (even {}) is truthy.
        let g = json!({ "status": "active", "tc": Value::Null });
        let g2 = json!({ "status": "active" });
        let g3 = json!({ "status": "active", "tc": {} });
        assert!(g.get("tc").is_none_or(|v| v.is_null()));
        assert!(g2.get("tc").is_none_or(|v| v.is_null()));
        assert!(!g3.get("tc").is_none_or(|v| v.is_null()));
    }
}
