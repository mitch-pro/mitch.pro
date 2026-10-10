//! The mini-games: Kody's Keyboard typing payouts (server.js:22422-22475),
//! Penny's Piano Keys (22477-22560), Sebastian's Piccolo (22570-22658), and
//! Lillian's Logic (22669-22864: state/validate/next-wordle/solve with the
//! minesweeper board validator).
//!
//! These response bodies carry only small integers/strings, so plain
//! `json_resp` + `jsval::num_value` formatting is exact.

use axum::http::{HeaderMap, Method};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::errors::json_resp;
use crate::state::AppState;
use mitch_lib::jsval;

/// `WORDS` — lillians-logic daily wordle pool (server.js:22709 and 22746,
/// identical 42-word list both places).
const LOGIC_WORDS: [&str; 42] = [
    "APPLE", "BREAD", "CLOUD", "DANCE", "EARTH", "FIELD", "GREEN", "HEART", "IMAGE", "JUICE",
    "LIGHT", "MUSIC", "NIGHT", "OCEAN", "PAPER", "QUEEN", "RIVER", "STONE", "TABLE", "VOICE",
    "WATER", "WORLD", "SPACE", "STARS", "PIXEL", "GAMES", "DREAM", "LEVEL", "BUILD", "ROBOT",
    "POWER", "LUCKY", "CHESS", "BOARD", "CLICK", "WEAVE", "FINAL", "TRUTH", "GALAXY", "SUPER",
    "SMART", "FASTY",
];

pub(crate) fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    _body: &Value,
    body_bytes: &[u8],
) -> Option<axum::response::Response> {
    match path {
        "/api/games/kodys-keyboard/payout" if *method == Method::POST => {
            Some(kodys_payout(state, headers, body_bytes))
        }
        "/api/games/pennys-piano-keys/payout" if *method == Method::POST => {
            Some(piano_payout(state, headers, body_bytes))
        }
        "/api/games/sebastians-piccolo/payout" if *method == Method::POST => {
            Some(piccolo_payout(state, headers, body_bytes))
        }
        "/api/games/lillians-logic/state" => Some(logic_state(state, headers)),
        "/api/games/lillians-logic/validate" if *method == Method::POST => {
            Some(logic_validate(state, body_bytes))
        }
        "/api/games/lillians-logic/next-wordle" if *method == Method::POST => {
            Some(logic_next_wordle(state, headers))
        }
        "/api/games/lillians-logic/solve" if *method == Method::POST => {
            Some(logic_solve(state, headers, body_bytes))
        }
        _ => None,
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `new Date(ms).toDateString()` (local timezone, via mitch-lib jstime).
fn to_date_string(ms: f64) -> String {
    mitch_lib::jstime::js_to_date_string(ms as i64)
}

/// `parseInt(v)` — optional sign, then decimal digits (or a `0x`/`0X` hex
/// run, which parseInt also accepts), NaN when no digits.
fn parse_int_js(s: &str) -> f64 {
    let t = s.trim_start();
    let (sign, t) = match t.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, t.strip_prefix('+').unwrap_or(t)),
    };
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        // parseInt("0x") → NaN; parseInt("0x1G") → 1; the sign applies ("-0x10" → -16).
        let digits: String = hex.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        if digits.is_empty() {
            return f64::NAN;
        }
        return i64::from_str_radix(&digits, 16)
            .map(|v| sign * v as f64)
            .unwrap_or(f64::NAN);
    }
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return f64::NAN;
    }
    sign * digits.parse::<f64>().unwrap_or(f64::NAN)
}

/// `Math.max(0, parseInt(body.score) || 0)` — NaN || 0 → 0, then clamp.
fn parse_score(body: &Value, key: &str) -> f64 {
    let n = body
        .get(key)
        .map(|v| parse_int_js(&jsval::string(v)))
        .unwrap_or(f64::NAN);
    if n.is_nan() || n == 0.0 {
        0.0
    } else {
        n.max(0.0)
    }
}

// ── Kody's Keyboard ──────────────────────────────────────────────────────────

/// `POST /api/games/kodys-keyboard/payout` — server.js:22422-22475.
fn kodys_payout(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> axum::response::Response {
    let Some(body) = crate::routes::me::parse_body_strict(body_bytes) else {
        return json_resp(400, json!({ "success": false }));
    };
    // kodys ladder (server.js:22425-22429): both stages → `401 {success:false}`.
    let email = match crate::routes::games::sid_email(state, headers) {
        Ok(e) => e,
        Err(_) => return json_resp(401, json!({ "success": false })),
    };
    let norm = mitch_lib::auth::normalize_email(&email);

    let wpm = body.get("wpm").and_then(jsval::number).unwrap_or(0.0);
    let time_taken = body.get("ms").and_then(jsval::number).unwrap_or(0.0);

    if wpm > 250.0 || time_taken < 2000.0 {
        return json_resp(400, json!({ "error": "Impossible speed" }));
    }

    let now = now_millis();
    let today = to_date_string(now as f64);
    let mut s = mitch_lib::minigames::mini_session_get(&state.store, &norm, "typing")
        .unwrap_or(json!({"dailyCount": 0, "lastTs": 0}));
    let last_ts = s.get("lastTs").and_then(jsval::number).unwrap_or(0.0);
    if to_date_string(last_ts) != today {
        if let Some(obj) = s.as_object_mut() {
            obj.insert("dailyCount".into(), json!(0));
        }
    }
    if now as f64 - last_ts < 60000.0 {
        return json_resp(429, json!({ "error": "Too many typing payouts" }));
    }
    let daily_count = s.get("dailyCount").and_then(jsval::number).unwrap_or(0.0);
    if daily_count >= 20.0 {
        return json_resp(429, json!({ "error": "Daily typing payout limit reached" }));
    }

    let mut coins: f64 = if wpm >= 120.0 {
        3.0
    } else if wpm >= 80.0 {
        2.0
    } else if wpm >= 40.0 {
        1.0
    } else {
        0.0
    };

    if coins > 0.0 {
        if mitch_lib::auth::is_premium_email(&state.store, &email) {
            coins *= 2.0;
        }
        // 1.5x friend-play bonus: any friend online in the last 120s.
        let mut friend_bonus_active = false;
        let friends = state
            .store
            .read_document(&state.data_dir().join("friends.json"), json!({}));
        let my_friends = friends
            .get(&norm)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let now2 = now_millis();
        let online = my_friends.iter().any(|f| {
            let f_norm = mitch_lib::auth::normalize_email(&jsval::string(f));
            let seen = state
                .cv_online
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&f_norm)
                .copied()
                .unwrap_or(0);
            seen != 0 && now2 - seen < 120000
        });
        if online {
            coins = (coins * 1.5).floor();
            friend_bonus_active = true;
        }
        mitch_lib::coins::add_coins(
            &state.store,
            &state.cfg.data_dir,
            &email,
            coins,
            state.coin_multiplier(),
            "",
        );
        mitch_lib::achievements::update_stat(
            &state.store,
            state.data_dir(),
            &email,
            "typing_coins",
            coins,
            state.coin_multiplier(),
        );
        mitch_lib::achievements::update_stat(
            &state.store,
            state.data_dir(),
            &email,
            "typing_races",
            1.0,
            state.coin_multiplier(),
        );
        if let Some(obj) = s.as_object_mut() {
            obj.insert("dailyCount".into(), jsval::num_value(daily_count + 1.0));
            obj.insert("lastTs".into(), json!(now));
        }
        mitch_lib::minigames::mini_session_set(&state.store, &norm, "typing", &s);
        tracing::info!(
            "[typing] {email} earned {coins} coins at {wpm} WPM{}",
            if friend_bonus_active {
                " (friend bonus)"
            } else {
                ""
            }
        );
        return json_resp(
            200,
            json!({
                "success": true,
                "coinsEarned": jsval::num_value(coins),
                "dailyRemaining": 999,
                "friendBonusActive": friend_bonus_active,
            }),
        );
    }
    // coins === 0: friendBonusActive is scoped inside the JS `if` — the key
    // is undefined and JSON.stringify drops it.
    json_resp(
        200,
        json!({
            "success": true,
            "coinsEarned": 0,
            "dailyRemaining": 999,
        }),
    )
}

// ── Penny's Piano Keys ───────────────────────────────────────────────────────

/// `POST /api/games/pennys-piano-keys/payout` — server.js:22477-22560.
fn piano_payout(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> axum::response::Response {
    let Some(body) = crate::routes::me::parse_body_strict(body_bytes) else {
        return json_resp(400, json!({"success": false, "error": "Invalid JSON body"}));
    };
    // piano ladder (server.js:22480-22484) — distinct body per stage.
    let email = match crate::routes::games::sid_email(state, headers) {
        Ok(e) => e,
        Err(crate::routes::games::SidFail::InvalidId) => {
            return json_resp(
                401,
                json!({"success": false, "error": "Authentication required"}),
            );
        }
        Err(crate::routes::games::SidFail::MissingIdentity) => {
            return json_resp(401, json!({"success": false, "error": "Invalid identity"}));
        }
    };
    let norm = mitch_lib::auth::normalize_email(&email);

    let score = parse_score(&body, "score");
    let ms = parse_score(&body, "ms");

    if score > 500.0 {
        mitch_lib::admin::log_cheat(
            &state.store,
            state.data_dir(),
            &email,
            "Penny's Piano Keys",
            &format!("Suspiciously high score: {score} tiles in {ms}ms"),
            "unknown",
        );
        return json_resp(
            400,
            json!({"success": false, "error": "Legendary performance detected, but score exceeds safety threshold (500 tiles)."}),
        );
    }
    if score > 5.0 {
        let time_per_tile = ms / score;
        if time_per_tile < 135.0 {
            mitch_lib::admin::log_cheat(
                &state.store,
                state.data_dir(),
                &email,
                "Penny's Piano Keys",
                &format!(
                    "Impossible speed: {score} tiles in {ms}ms ({}ms/tile)",
                    time_per_tile.round()
                ),
                "unknown",
            );
            return json_resp(
                400,
                json!({"success": false, "error": "Suspiciously fast tiles! Play like a human."}),
            );
        }
    }

    let now = now_millis();
    let today = to_date_string(now as f64);
    let mut s = mitch_lib::minigames::mini_session_get(&state.store, &norm, "piano")
        .unwrap_or(json!({"dailyCount": 0, "lastTs": 0}));
    let last_ts = s.get("lastTs").and_then(jsval::number).unwrap_or(0.0);
    if to_date_string(last_ts) != today {
        if let Some(obj) = s.as_object_mut() {
            obj.insert("dailyCount".into(), json!(0));
        }
    }
    // Time-travel loop exploit prevention: claimed duration vs real elapsed.
    let actual_elapsed = now as f64 - last_ts;
    if last_ts > 0.0 && ms > 5000.0 && actual_elapsed < ms * 0.8 {
        mitch_lib::admin::log_cheat(
            &state.store,
            state.data_dir(),
            &email,
            "Penny's Piano Keys",
            &format!(
                "Time-travel exploit: Claimed game duration {ms}ms, but only {actual_elapsed}ms elapsed since last submission"
            ),
            "unknown",
        );
        return json_resp(
            400,
            json!({"success": false, "error": "Exploit detected: Play in real-time!"}),
        );
    }

    let daily_cap = 1000.0;
    let daily_count = s.get("dailyCount").and_then(jsval::number).unwrap_or(0.0);
    let mut allowed_tiles = score.max(0.0);
    if daily_count >= daily_cap {
        allowed_tiles = 0.0;
    } else if daily_count + allowed_tiles > daily_cap {
        allowed_tiles = daily_cap - daily_count;
    }

    let coins_awarded = (allowed_tiles * 0.02).floor();

    if coins_awarded > 0.0 {
        let mut final_coins = coins_awarded;
        if mitch_lib::auth::is_premium_email(&state.store, &email) {
            final_coins = coins_awarded * 2.0;
        }
        mitch_lib::coins::add_coins(
            &state.store,
            &state.cfg.data_dir,
            &email,
            final_coins,
            state.coin_multiplier(),
            "",
        );
        mitch_lib::achievements::update_stat(
            &state.store,
            state.data_dir(),
            &email,
            "piano_coins",
            final_coins,
            state.coin_multiplier(),
        );
        mitch_lib::achievements::update_stat(
            &state.store,
            state.data_dir(),
            &email,
            "piano_games",
            1.0,
            state.coin_multiplier(),
        );
        let new_daily = daily_count + allowed_tiles;
        if let Some(obj) = s.as_object_mut() {
            obj.insert("dailyCount".into(), jsval::num_value(new_daily));
            obj.insert("lastTs".into(), json!(now));
        }
        mitch_lib::minigames::mini_session_set(&state.store, &norm, "piano", &s);
        tracing::info!("[piano] {email} earned {final_coins} coins for score {score}");
        return json_resp(
            200,
            json!({
                "success": true,
                "coinsEarned": jsval::num_value(final_coins),
                "dailyRemaining": jsval::num_value(daily_cap - new_daily),
            }),
        );
    }

    // Zero coins: still stamp lastTs and count the game.
    if let Some(obj) = s.as_object_mut() {
        obj.insert("lastTs".into(), json!(now));
    }
    mitch_lib::minigames::mini_session_set(&state.store, &norm, "piano", &s);
    mitch_lib::achievements::update_stat(
        &state.store,
        state.data_dir(),
        &email,
        "piano_games",
        1.0,
        state.coin_multiplier(),
    );
    json_resp(
        200,
        json!({
            "success": true,
            "coinsEarned": 0,
            "dailyRemaining": 0,
            "message": "Daily coin limit reached (1,000). Come back tomorrow!",
        }),
    )
}

// ── Sebastian's Piccolo ──────────────────────────────────────────────────────

/// `POST /api/games/sebastians-piccolo/payout` — server.js:22570-22658.
fn piccolo_payout(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> axum::response::Response {
    let Some(body) = crate::routes::me::parse_body_strict(body_bytes) else {
        return json_resp(400, json!({"success": false, "error": "Invalid JSON body"}));
    };
    // piccolo ladder (server.js:22573-22577) — same bodies as piano.
    let email = match crate::routes::games::sid_email(state, headers) {
        Ok(e) => e,
        Err(crate::routes::games::SidFail::InvalidId) => {
            return json_resp(
                401,
                json!({"success": false, "error": "Authentication required"}),
            );
        }
        Err(crate::routes::games::SidFail::MissingIdentity) => {
            return json_resp(401, json!({"success": false, "error": "Invalid identity"}));
        }
    };
    let norm = mitch_lib::auth::normalize_email(&email);

    let score = parse_score(&body, "score");
    let ms = parse_score(&body, "ms");

    if score > 12000.0 {
        mitch_lib::admin::log_cheat(
            &state.store,
            state.data_dir(),
            &email,
            "Sebastian's Piccolo",
            &format!("Suspiciously high score: {score} points in {ms}ms"),
            "unknown",
        );
        return json_resp(
            400,
            json!({"success": false, "error": "Legendary performance detected, but score exceeds safety threshold (12,000 points)."}),
        );
    }
    // ~12 points/tile average → estimated tiles, 135ms/tile human floor.
    let estimated_tiles = score / 12.0;
    if score > 100.0 && estimated_tiles > 0.0 {
        let time_per_tile = ms / estimated_tiles;
        if time_per_tile < 135.0 {
            mitch_lib::admin::log_cheat(
                &state.store,
                state.data_dir(),
                &email,
                "Sebastian's Piccolo",
                &format!(
                    "Impossible speed: {score} points in {ms}ms ({}ms/tile)",
                    time_per_tile.round()
                ),
                "unknown",
            );
            return json_resp(
                400,
                json!({"success": false, "error": "Suspiciously fast notes! Play like a human."}),
            );
        }
    }

    let now = now_millis();
    let today = to_date_string(now as f64);
    let mut s = mitch_lib::minigames::mini_session_get(&state.store, &norm, "piccolo")
        .unwrap_or(json!({"dailyCoins": 0, "lastTs": 0}));
    let last_ts = s.get("lastTs").and_then(jsval::number).unwrap_or(0.0);
    if to_date_string(last_ts) != today {
        if let Some(obj) = s.as_object_mut() {
            obj.insert("dailyCoins".into(), json!(0));
        }
    }
    let actual_elapsed = now as f64 - last_ts;
    if last_ts > 0.0 && ms > 5000.0 && actual_elapsed < ms * 0.8 {
        mitch_lib::admin::log_cheat(
            &state.store,
            state.data_dir(),
            &email,
            "Sebastian's Piccolo",
            &format!(
                "Time-travel exploit: Claimed game duration {ms}ms, but only {actual_elapsed}ms elapsed since last submission"
            ),
            "unknown",
        );
        return json_resp(
            400,
            json!({"success": false, "error": "Exploit detected: Play in real-time!"}),
        );
    }

    let daily_coin_cap = 1000.0;
    // 0.005 coins per point (0.05 per 10 points).
    let base_coins_awarded = (score * 0.005).floor();
    let daily_coins = s.get("dailyCoins").and_then(jsval::number).unwrap_or(0.0);
    let mut allowed_coins = base_coins_awarded;
    if daily_coins >= daily_coin_cap {
        allowed_coins = 0.0;
    } else if daily_coins + allowed_coins > daily_coin_cap {
        allowed_coins = daily_coin_cap - daily_coins;
    }

    if allowed_coins > 0.0 {
        let mut final_coins = allowed_coins;
        if mitch_lib::auth::is_premium_email(&state.store, &email) {
            final_coins = allowed_coins * 2.0;
        }
        mitch_lib::coins::add_coins(
            &state.store,
            &state.cfg.data_dir,
            &email,
            final_coins,
            state.coin_multiplier(),
            "Sebastian's Piccolo Symphony Performance",
        );
        mitch_lib::achievements::update_stat(
            &state.store,
            state.data_dir(),
            &email,
            "piccolo_coins",
            final_coins,
            state.coin_multiplier(),
        );
        mitch_lib::achievements::update_stat(
            &state.store,
            state.data_dir(),
            &email,
            "piccolo_games",
            1.0,
            state.coin_multiplier(),
        );
        let new_daily = daily_coins + allowed_coins;
        if let Some(obj) = s.as_object_mut() {
            obj.insert("dailyCoins".into(), jsval::num_value(new_daily));
            obj.insert("lastTs".into(), json!(now));
        }
        mitch_lib::minigames::mini_session_set(&state.store, &norm, "piccolo", &s);
        tracing::info!("[piccolo] {email} earned {final_coins} coins for score {score}");
        return json_resp(
            200,
            json!({
                "success": true,
                "coinsEarned": jsval::num_value(final_coins),
                "dailyRemainingCoins": jsval::num_value(daily_coin_cap - new_daily),
            }),
        );
    }

    if let Some(obj) = s.as_object_mut() {
        obj.insert("lastTs".into(), json!(now));
    }
    mitch_lib::minigames::mini_session_set(&state.store, &norm, "piccolo", &s);
    mitch_lib::achievements::update_stat(
        &state.store,
        state.data_dir(),
        &email,
        "piccolo_games",
        1.0,
        state.coin_multiplier(),
    );
    json_resp(
        200,
        json!({
            "success": true,
            "coinsEarned": 0,
            "dailyRemainingCoins": jsval::num_value(daily_coin_cap - daily_coins),
            "message": if daily_coins >= daily_coin_cap {
                "Daily limit reached (1,000)."
            } else {
                "No coins earned."
            },
        }),
    )
}

// ── Lillian's Logic ──────────────────────────────────────────────────────────

/// `GET|POST|PUT… /api/games/lillians-logic/state` — server.js:22669-22682.
/// No method check in the JS: any verb runs the ladder (CSRF applies to
/// non-GET via the global gate).
fn logic_state(state: &Arc<AppState>, headers: &HeaderMap) -> axum::response::Response {
    let email = match crate::routes::games::games_email(state, headers) {
        Ok(e) => e,
        Err(resp) => return *resp,
    };
    let norm = mitch_lib::auth::normalize_email(&email);
    let s = mitch_lib::minigames::mini_session_get(&state.store, &norm, "logic")
        .unwrap_or(json!({"puzzlesDone": 0, "lastSolvedTs": 0}));
    let today = to_date_string(now_millis() as f64);
    let solved_today =
        to_date_string(s.get("lastSolvedTs").and_then(jsval::number).unwrap_or(0.0)) == today;
    let puzzles_done = s.get("puzzlesDone").and_then(jsval::number).unwrap_or(0.0);
    json_resp(
        200,
        json!({
            "success": true,
            "solvedToday": solved_today,
            "puzzlesDone": jsval::num_value(puzzles_done),
        }),
    )
}

/// `POST /api/games/lillians-logic/validate` — server.js:22684-22688. No
/// auth at all: anonymous word checks are allowed.
fn logic_validate(state: &Arc<AppState>, body_bytes: &[u8]) -> axum::response::Response {
    let Some(body) = crate::routes::me::parse_body_strict(body_bytes) else {
        return json_resp(400, json!({ "success": false }));
    };
    let word = jsval::string(&jsval::or(body.get("word"), json!(""))).to_uppercase();
    let valid = state
        .logic_dictionary
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&word.to_lowercase());
    json_resp(200, json!({ "success": true, "valid": valid }))
}

/// `POST /api/games/lillians-logic/next-wordle` — server.js:22690-22717.
/// The explicit `checkRateLimit(req, path)` here is a NO-OP in bun (the
/// global /api/ gate stamps `req._rateLimitChecked` first); the Rust global
/// gate already covers it.
fn logic_next_wordle(state: &Arc<AppState>, headers: &HeaderMap) -> axum::response::Response {
    // ladder (server.js:22693-22697).
    let email = match crate::routes::games::sid_email(state, headers) {
        Ok(e) => e,
        Err(crate::routes::games::SidFail::InvalidId) => {
            return json_resp(
                401,
                json!({"success": false, "error": "Authentication required"}),
            );
        }
        Err(crate::routes::games::SidFail::MissingIdentity) => {
            return json_resp(401, json!({"success": false, "error": "Invalid identity"}));
        }
    };
    let norm = mitch_lib::auth::normalize_email(&email);
    let random_word = LOGIC_WORDS[mitch_lib::crypto::js_random_index(LOGIC_WORDS.len())];
    let mut s = mitch_lib::minigames::mini_session_get(&state.store, &norm, "logic")
        .unwrap_or(json!({"puzzlesDone": 0, "lastSolvedTs": 0}));
    if let Some(obj) = s.as_object_mut() {
        obj.insert("currentWordle".into(), json!(random_word));
    }
    mitch_lib::minigames::mini_session_set(&state.store, &norm, "logic", &s);
    json_resp(200, json!({ "success": true, "word": random_word }))
}

/// `POST /api/games/lillians-logic/solve` — server.js:22719-22863.
fn logic_solve(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> axum::response::Response {
    let Some(body) = crate::routes::me::parse_body_strict(body_bytes) else {
        return json_resp(400, json!({"success": false, "error": "Invalid JSON body"}));
    };
    // ladder (server.js:22718-22722).
    let email = match crate::routes::games::sid_email(state, headers) {
        Ok(e) => e,
        Err(crate::routes::games::SidFail::InvalidId) => {
            return json_resp(
                401,
                json!({"success": false, "error": "Authentication required"}),
            );
        }
        Err(crate::routes::games::SidFail::MissingIdentity) => {
            return json_resp(401, json!({"success": false, "error": "Invalid identity"}));
        }
    };
    let norm = mitch_lib::auth::normalize_email(&email);
    let now = now_millis();
    let today = to_date_string(now as f64);

    let mut s = mitch_lib::minigames::mini_session_get(&state.store, &norm, "logic")
        .unwrap_or(json!({"puzzlesDone": 0, "lastSolvedTs": 0}));
    let first_today =
        to_date_string(s.get("lastSolvedTs").and_then(jsval::number).unwrap_or(0.0)) != today;

    let game_type = jsval::str_or(body.get("type"), "wordle");
    let coins: f64;

    if game_type == "wordle" {
        let guess = jsval::string(&jsval::or(body.get("word"), json!(""))).to_uppercase();
        // getWordForDate: seed = y*10000 + (m+1)*100 + d (local tz).
        let word_for = |ms: i64| -> &'static str {
            let seed = mitch_lib::jstime::js_date_seed(ms);
            LOGIC_WORDS[(seed % LOGIC_WORDS.len() as i64) as usize]
        };
        let target_today = word_for(now);
        let target_yesterday = word_for(now - 24 * 3600 * 1000);
        let target_tomorrow = word_for(now + 24 * 3600 * 1000);

        let current = s
            .get("currentWordle")
            .map(jsval::string)
            .unwrap_or_default();
        let mut is_valid = false;
        if !current.is_empty() && guess == current {
            is_valid = true;
            // delete s.currentWordle — clear so the same word can't re-solve.
            if let Some(obj) = s.as_object_mut() {
                obj.remove("currentWordle");
            }
        } else if guess == target_today || guess == target_yesterday || guess == target_tomorrow {
            is_valid = true;
        }
        if !is_valid {
            return json_resp(
                400,
                json!({"success": false, "error": "Incorrect Wordle solution"}),
            );
        }
        coins = 20.0;
        if let Some(obj) = s.as_object_mut() {
            obj.insert("lastSolvedTs".into(), json!(now));
        }
    } else if game_type == "mines" {
        // 10s cooldown.
        let last_mines_ts = s
            .get("lastMinesSolvedTs")
            .and_then(jsval::number)
            .unwrap_or(0.0);
        if now as f64 - last_mines_ts < 10000.0 {
            return json_resp(
                429,
                json!({"success": false, "error": "Solving Minesweeper too fast! Cooldown is active."}),
            );
        }
        // Daily cap reset + 50/day limit.
        let last_mines_date = s
            .get("lastMinesSolvedDate")
            .map(jsval::string)
            .unwrap_or_default();
        if last_mines_date != today {
            if let Some(obj) = s.as_object_mut() {
                obj.insert("minesSolvedTodayCount".into(), json!(0));
                obj.insert("lastMinesSolvedDate".into(), json!(today.clone()));
            }
        }
        let mines_today = s
            .get("minesSolvedTodayCount")
            .and_then(jsval::number)
            .unwrap_or(0.0);
        if mines_today >= 50.0 {
            return json_resp(
                400,
                json!({"success": false, "error": "Daily Minesweeper limit reached (50/day)"}),
            );
        }
        // Board validation: 100 cells, each {mine: bool, revealed: bool}.
        let Some(board) = body.get("board").and_then(|v| v.as_array()) else {
            return json_resp(
                400,
                json!({"success": false, "error": "Invalid board data uploaded"}),
            );
        };
        if board.len() != 100 {
            return json_resp(
                400,
                json!({"success": false, "error": "Invalid board data uploaded"}),
            );
        }
        let timer = body.get("timer").and_then(jsval::number).unwrap_or(0.0);
        if timer < 5.0 {
            return json_resp(
                400,
                json!({"success": false, "error": "Impossible solve speed!"}),
            );
        }

        // Materialize the JS board shape: cells with mine/revealed/count.
        let mut mines = [false; 100];
        let mut revealed = [false; 100];
        let mut counts = [0.0f64; 100];
        for (i, cell) in board.iter().enumerate() {
            let mine = cell.get("mine");
            let rev = cell.get("revealed");
            let (Some(mine), Some(rev)) = (mine, rev) else {
                return json_resp(
                    400,
                    json!({"success": false, "error": format!("Malformed cell at index {i}")}),
                );
            };
            let (Some(mine), Some(rev)) = (mine.as_bool(), rev.as_bool()) else {
                return json_resp(
                    400,
                    json!({"success": false, "error": format!("Malformed cell at index {i}")}),
                );
            };
            mines[i] = mine;
            revealed[i] = rev;
            counts[i] = cell.get("count").and_then(jsval::number).unwrap_or(0.0);
        }

        let mut mine_count = 0usize;
        let mut rev_count = 0usize;
        for i in 0..100 {
            if mines[i] {
                mine_count += 1;
                if revealed[i] {
                    return json_resp(
                        400,
                        json!({"success": false, "error": "Invalid board: a mine was revealed!"}),
                    );
                }
            } else if revealed[i] {
                rev_count += 1;
            }
        }
        if mine_count != 15 {
            return json_resp(
                400,
                json!({"success": false, "error": "Invalid board: must contain exactly 15 mines"}),
            );
        }
        if rev_count != 85 {
            return json_resp(
                400,
                json!({"success": false, "error": "Invalid board: must reveal all 85 safe cells"}),
            );
        }

        // Neighbor count validation over the 10x10 grid.
        for i in 0..100 {
            if revealed[i] {
                let computed = neighbors(i).filter(|&n| mines[n]).count();
                if counts[i] != computed as f64 {
                    return json_resp(
                        400,
                        json!({"success": false, "error": format!("Invalid neighbor count at cell {i}. Expected {computed}, got {}", cell_count_raw(&board[i]))}),
                    );
                }
            }
        }

        coins = 2.0;
        if let Some(obj) = s.as_object_mut() {
            obj.insert("lastMinesSolvedTs".into(), json!(now));
            obj.insert(
                "minesSolvedTodayCount".into(),
                jsval::num_value(mines_today + 1.0),
            );
            obj.insert("lastMinesSolvedDate".into(), json!(today.clone()));
        }
    } else {
        return json_resp(400, json!({"success": false, "error": "Unknown game type"}));
    }

    let final_coins = if mitch_lib::auth::is_premium_email(&state.store, &email) {
        coins * 2.0
    } else {
        coins
    };
    mitch_lib::coins::add_coins(
        &state.store,
        &state.cfg.data_dir,
        &email,
        final_coins,
        state.coin_multiplier(),
        "",
    );
    mitch_lib::achievements::update_stat(
        &state.store,
        state.data_dir(),
        &email,
        "logic_coins",
        final_coins,
        state.coin_multiplier(),
    );
    mitch_lib::achievements::update_stat(
        &state.store,
        state.data_dir(),
        &email,
        "logic_puzzles",
        1.0,
        state.coin_multiplier(),
    );
    let puzzles_done = s.get("puzzlesDone").and_then(jsval::number).unwrap_or(0.0);
    if let Some(obj) = s.as_object_mut() {
        obj.insert("puzzlesDone".into(), jsval::num_value(puzzles_done + 1.0));
    }
    mitch_lib::minigames::mini_session_set(&state.store, &norm, "logic", &s);
    tracing::info!(
        "[logic] {email} solved {game_type}, earned {final_coins} coins (First today: {first_today})"
    );
    json_resp(
        200,
        json!({
            "success": true,
            "coinsEarned": jsval::num_value(final_coins),
            "firstToday": game_type == "wordle" && first_today,
        }),
    )
}

/// `Number(cell.count || 0)` in the error message — the raw value for the
/// `got ${cell.count}` interpolation (JS template stringifies the original).
fn cell_count_raw(cell: &Value) -> String {
    match cell.get("count") {
        Some(v) => jsval::string(v),
        None => {
            // JS: cell.count is undefined → template prints "undefined".
            "undefined".to_string()
        }
    }
}

/// The 10x10 minesweeper neighborhood (server.js:22819-22831).
fn neighbors(idx: usize) -> impl Iterator<Item = usize> {
    let r = (idx / 10) as i64;
    let c = (idx % 10) as i64;
    let mut out = Vec::new();
    for dr in -1i64..=1 {
        for dc in -1i64..=1 {
            if dr == 0 && dc == 0 {
                continue;
            }
            let nr = r + dr;
            let nc = c + dc;
            if (0..10).contains(&nr) && (0..10).contains(&nc) {
                out.push((nr * 10 + nc) as usize);
            }
        }
    }
    out.into_iter()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_int_semantics() {
        assert_eq!(parse_int_js("12.7"), 12.0);
        assert_eq!(parse_int_js("-3"), -3.0);
        assert!(parse_int_js("").is_nan());
        assert!(parse_int_js("x8").is_nan());
        assert_eq!(parse_score(&json!({"score": "12.9"}), "score"), 12.0);
        assert_eq!(parse_score(&json!({"score": -5}), "score"), 0.0);
        assert_eq!(parse_score(&json!({"ms": 250}), "ms"), 250.0);
        assert_eq!(parse_score(&json!({}), "ms"), 0.0);
    }

    #[test]
    fn neighbors_shape() {
        assert_eq!(neighbors(0).count(), 3); // corner
        assert_eq!(neighbors(5).count(), 5); // edge
        assert_eq!(neighbors(45).count(), 8); // middle
        assert_eq!(neighbors(99).count(), 3); // bottom-right corner
        assert!(neighbors(45).all(|n| n < 100));
    }

    #[test]
    fn word_pool_and_day_seed_are_stable() {
        assert_eq!(LOGIC_WORDS.len(), 42);
        assert_eq!(LOGIC_WORDS[0], "APPLE");
        assert_eq!(LOGIC_WORDS[41], "FASTY");
    }

    #[test]
    fn malformed_cells_rejected_shape() {
        // typeof mine !== 'boolean' → malformed (a 1 or "true" is not a bool)
        let bad = json!({"mine": 1, "revealed": true});
        assert!(bad.get("mine").and_then(|v| v.as_bool()).is_none());
        let ok = json!({"mine": true, "revealed": false});
        assert!(ok.get("mine").and_then(|v| v.as_bool()).is_some());
    }

    #[test]
    fn count_message_uses_raw_value() {
        assert_eq!(cell_count_raw(&json!({"count": 3})), "3");
        assert_eq!(cell_count_raw(&json!({"count": "x"})), "x");
        assert_eq!(cell_count_raw(&json!({})), "undefined");
    }
}
