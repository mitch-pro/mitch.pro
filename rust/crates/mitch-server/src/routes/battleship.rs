//! `/api/battleship/*` — the two-player naval game (plan Step 12; the
//! prelude + 8 endpoints at server.js:20754-21007).
//!
//! State model (all in-memory, like JS): `bsChallenges` (server.js:884) and
//! `bsGames` (885) are insertion-ordered maps — challenge expiry sweeps and
//! the heartbeat's challenge/activeGames lists iterate in JS
//! `Object.entries`/`Object.values` order — and `bsOnline` (886) maps email
//! → last-seen ms, scanned in insertion order by `/online`.
//!
//! Auth differs from the casino/jeopardy groups: the JS reads the
//! `studentId`/`id` cookies directly (server.js:20757-20762), which
//! `getCookies` re-derives from a valid `mitch_session` — so a session
//! cookie satisfies it. `names[sid]` is the RAW lowercase email here (the
//! `placedBy.includes(myEmail)` membership check at 20867 compares against
//! the normalized entries literally — quirk preserved).

use axum::http::{HeaderMap, Method};
use indexmap::IndexMap;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;

use mitch_lib::data::js_stringify;
use mitch_lib::jsval;

use crate::errors::json_resp_str;
use crate::state::AppState;

fn resp(code: u16, body: &Value) -> axum::response::Response {
    json_resp_str(code, js_stringify(body))
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// One `bsChallenges[id]` (server.js:884, 20799).
#[derive(Clone)]
pub struct BsChallenge {
    pub id: String,
    pub from: String,
    pub to: String,
    pub bet: f64,
    pub created_at: i64,
}

/// One ship from `/place` — the JS-mutated `ships[i]` object
/// (`{ row, col, dir, size, cells, hits: 0 }`, server.js:20888). `row`/`col`
/// are `Number()`-coerced, so NaN slips through the bounds check (JS
/// `NaN < 0`/`NaN > 9` are both false) and serializes as `null` exactly like
/// `JSON.stringify(NaN)`.
#[derive(Debug)]
pub struct BsShip {
    pub row: f64,
    pub col: f64,
    pub dir: String,
    pub size: i64,
    pub cells: Vec<String>,
    pub hits: i64,
}

/// One `g.boards[email]` (server.js:20845).
pub struct BsBoard {
    pub ships: Option<Vec<BsShip>>,
    pub hits: Vec<String>,
    pub misses: Vec<String>,
}

/// One `bsGames[gameId]` (server.js:20838-20856). `boards` is
/// insertion-ordered (challenger, then accepter) to match the JS object
/// literal.
pub struct BsGame {
    pub id: String,
    pub player1: String,
    pub player2: String,
    pub bet: f64,
    /// placing | active | over
    pub status: String,
    pub boards: IndexMap<String, BsBoard>,
    pub placed_by: Vec<String>,
    pub turn: Option<String>,
    pub winner: Option<String>,
    pub result: Option<String>,
    // bun stores createdAt/lastActionAt but never reads them back.
    #[allow(dead_code)]
    pub created_at: i64,
    #[allow(dead_code)]
    pub last_action_at: i64,
    pub hit_log: Vec<BsHit>,
    pub collusion_flag: bool,
}

/// One `hitLog` entry (`{ by, at }`, server.js:20928) — `by` is stored but
/// never read back (the collusion check only reads `at`).
#[allow(dead_code)]
pub struct BsHit {
    pub by: String,
    pub at: i64,
}

// ── Shared prelude ───────────────────────────────────────────────────────────

pub(crate) async fn handle(
    state: &Arc<AppState>,
    _method: &Method,
    path: &str,
    headers: &HeaderMap,
    search: &str,
    body_bytes: &[u8],
) -> Option<axum::response::Response> {
    if !path.starts_with("/api/battleship/") {
        return None;
    }
    // Auth ladder (server.js:20756-20763). The JS `checkRateLimit` call runs
    // in the global gate (handler.rs) — same limiter, same per-path key.
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
        return Some(resp(403, &json!({ "error": "not found" })));
    }
    let my_norm = mitch_lib::auth::normalize_email(&my_email);

    // `tryParseJson` (server.js:10514) — only the five mutating endpoints
    // parse a body; heartbeat/online/state never touch it.
    let body = if !matches!(
        path,
        "/api/battleship/challenge"
            | "/api/battleship/respond"
            | "/api/battleship/place"
            | "/api/battleship/fire"
            | "/api/battleship/resign"
    ) || body_bytes.is_empty()
    {
        json!({})
    } else {
        match serde_json::from_slice(body_bytes) {
            Ok(b) => b,
            Err(_) => return Some(resp(400, &json!({ "error": "bad json" }))),
        }
    };

    // Unknown /api/battleship/* paths pass the auth ladder, then fall through
    // to the static 404 (no inner `if` matches in the JS group either).
    route(state, path, search, &body, &my_email, &my_norm)
}

fn is_revoked_id(state: &AppState, sid: &str) -> bool {
    state
        .store
        .read_document(&state.data_dir().join("revoked.json"), json!({}))
        .get(sid)
        .is_some()
}

fn route(
    state: &Arc<AppState>,
    path: &str,
    search: &str,
    body: &Value,
    my_email: &str,
    my_norm: &str,
) -> Option<axum::response::Response> {
    const KNOWN: [&str; 8] = [
        "/api/battleship/heartbeat",
        "/api/battleship/online",
        "/api/battleship/challenge",
        "/api/battleship/respond",
        "/api/battleship/place",
        "/api/battleship/fire",
        "/api/battleship/state",
        "/api/battleship/resign",
    ];
    if !KNOWN.contains(&path) {
        return None;
    }
    match path {
        "/api/battleship/heartbeat" => Some(heartbeat(state, my_norm)),
        "/api/battleship/online" => Some(online(state)),
        "/api/battleship/challenge" => Some(challenge(state, body, my_norm)),
        "/api/battleship/respond" => Some(respond(state, body, my_norm)),
        "/api/battleship/place" => Some(place(state, body, my_email, my_norm)),
        "/api/battleship/fire" => Some(fire(state, body, my_norm)),
        "/api/battleship/state" => Some(state_view(state, search, my_email, my_norm)),
        "/api/battleship/resign" => Some(resign(state, body, my_norm)),
        _ => None,
    }
}

// ── `/heartbeat` (server.js:20765-20778) ─────────────────────────────────────

fn heartbeat(state: &Arc<AppState>, my_norm: &str) -> axum::response::Response {
    let now = now_millis();
    state
        .bs_online
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(my_norm.to_string(), now);
    let mut challenges = state
        .bs_challenges
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    challenges.retain(|_, c| now - c.created_at <= 300000);
    let list: Vec<Value> = challenges
        .values()
        .filter(|c| {
            mitch_lib::auth::normalize_email(&c.to) == my_norm
                || mitch_lib::auth::normalize_email(&c.from) == my_norm
        })
        .map(|c| {
            json!({
                "id": c.id,
                "from": mitch_lib::profile::display_email(
                    &state.store, &state.cfg.data_dir, &state.id_secret, &c.from),
                "to": mitch_lib::profile::display_email(
                    &state.store, &state.cfg.data_dir, &state.id_secret, &c.to),
                "bet": jsval::num_value(c.bet),
                "createdAt": c.created_at,
            })
        })
        .collect();
    drop(challenges);
    let games = state.bs_games.lock().unwrap_or_else(|e| e.into_inner());
    let active_games: Vec<String> = games
        .values()
        .filter(|g| {
            (mitch_lib::auth::normalize_email(&g.player1) == my_norm
                || mitch_lib::auth::normalize_email(&g.player2) == my_norm)
                && g.status != "over"
        })
        .map(|g| g.id.clone())
        .collect();
    resp(
        200,
        &json!({ "challenges": list, "activeGames": active_games }),
    )
}

// ── `/online` (server.js:20780-20786) ────────────────────────────────────────

fn online(state: &Arc<AppState>) -> axum::response::Response {
    let now = now_millis();
    let online: Vec<String> = state
        .bs_online
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|(_, t)| now - **t < 60000)
        .map(|(e, _)| mitch_lib::admin::mask_email(e))
        .collect();
    resp(200, &json!({ "online": online }))
}

/// `myDisplayName` (server.js:20801-20802 / 20826-20827) — profile
/// displayName || nickname || username || the canonical local part.
fn display_name(state: &AppState, norm: &str, canonical: &str) -> String {
    let profiles: Value = state
        .store
        .read_document(&state.data_dir().join("profiles.json"), json!({}));
    let prof = profiles.get(norm).cloned().unwrap_or(json!({}));
    let pick = ["displayName", "nickname", "username"]
        .iter()
        .find_map(|k| prof.get(*k).filter(|v| jsval::truthy(v)).cloned())
        .unwrap_or_else(|| json!(canonical.split('@').next().unwrap_or("")));
    jsval::string(&pick)
}

fn notify_challenger(state: &Arc<AppState>, target: &str, title: &str, message: &str) {
    mitch_lib::coins::add_admin_notification(
        &state.store,
        &state.cfg.data_dir,
        target,
        title,
        message,
        "admin",
        "",
        "/games/battleship/",
    );
    // `triggerNotificationRefresh` (server.js:5754-5761) — every broadcast
    // socket.
    crate::ws::broadcast(
        state,
        crate::ws::WsRecipients::All,
        json!({ "type": "refresh_notifications" }).to_string(),
    );
}

// ── `/challenge` (server.js:20788-20807) ─────────────────────────────────────

fn challenge(state: &Arc<AppState>, body: &Value, my_norm: &str) -> axum::response::Response {
    let to = mitch_lib::auth::normalize_email(&jsval::string_of(body.get("to")));
    if to.is_empty() || to == my_norm {
        return resp(400, &json!({ "error": "invalid target" }));
    }
    // `Math.max(0, Math.floor(Number(body.bet) || 0))` — NaN → 0.
    let bet = {
        let n = jsval::number(body.get("bet").unwrap_or(&Value::Null)).unwrap_or(f64::NAN);
        let n = if n.is_nan() { 0.0 } else { n };
        n.floor().max(0.0)
    };
    if bet > 0.0 && mitch_lib::coins::get_coins(&state.store, &state.cfg.data_dir, my_norm) < bet {
        return resp(400, &json!({ "error": "Insufficient coins for bet" }));
    }
    let now = now_millis();
    let id = mitch_lib::crypto::random_bytes_hex(8);
    {
        let mut challenges = state
            .bs_challenges
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        challenges.retain(|_, c| now - c.created_at <= 300000);
        challenges.insert(
            id.clone(),
            BsChallenge {
                id: id.clone(),
                from: my_norm.to_string(),
                to: to.clone(),
                bet,
                created_at: now,
            },
        );
    }
    let my_canonical = mitch_lib::profile::canonical_delivery_email(
        &state.store,
        &state.cfg.data_dir,
        &state.id_secret,
        my_norm,
    );
    let my_display = display_name(state, my_norm, &my_canonical);
    let to_target = mitch_lib::profile::canonical_delivery_email(
        &state.store,
        &state.cfg.data_dir,
        &state.id_secret,
        &to,
    );
    let bet_suffix = if bet > 0.0 {
        format!(" (Bet: {} coins)", js_num_str(bet))
    } else {
        String::new()
    };
    notify_challenger(
        state,
        &to_target,
        "New Battleship Challenge",
        &format!(
            "{} has challenged you to a Battleship game{}.",
            my_display, bet_suffix
        ),
    );
    resp(200, &json!({ "ok": true, "challengeId": id }))
}

// ── `/respond` (server.js:20809-20858) ───────────────────────────────────────

fn respond(state: &Arc<AppState>, body: &Value, my_norm: &str) -> axum::response::Response {
    let challenge_id = jsval::string_of(body.get("challengeId"));
    let accept = jsval::truthy(body.get("accept").unwrap_or(&Value::Null));
    let c = {
        let mut challenges = state
            .bs_challenges
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(c) = challenges.get(&challenge_id).cloned() else {
            return resp(404, &json!({ "error": "challenge not found or expired" }));
        };
        if mitch_lib::auth::normalize_email(&c.to) != my_norm {
            return resp(403, &json!({ "error": "not your challenge" }));
        }
        if c.bet > 0.0 {
            if mitch_lib::coins::get_coins(&state.store, &state.cfg.data_dir, my_norm) < c.bet {
                challenges.shift_remove(&challenge_id);
                return resp(400, &json!({ "error": "Insufficient coins" }));
            }
            if mitch_lib::coins::get_coins(&state.store, &state.cfg.data_dir, &c.from) < c.bet {
                challenges.shift_remove(&challenge_id);
                return resp(
                    400,
                    &json!({
                        "error":
                            "Challenger no longer has enough coins. Challenge cancelled."
                    }),
                );
            }
            if accept {
                mitch_lib::coins::add_coins(
                    &state.store,
                    &state.cfg.data_dir,
                    my_norm,
                    -c.bet,
                    state.coin_multiplier(),
                    "battleship: bet deduction on start",
                );
                mitch_lib::coins::add_coins(
                    &state.store,
                    &state.cfg.data_dir,
                    &c.from,
                    -c.bet,
                    state.coin_multiplier(),
                    "battleship: bet deduction on start",
                );
            }
        }
        challenges.shift_remove(&challenge_id);
        c
    };
    let my_canonical = mitch_lib::profile::canonical_delivery_email(
        &state.store,
        &state.cfg.data_dir,
        &state.id_secret,
        my_norm,
    );
    let my_display = display_name(state, my_norm, &my_canonical);
    let challenger_target = mitch_lib::profile::canonical_delivery_email(
        &state.store,
        &state.cfg.data_dir,
        &state.id_secret,
        &c.from,
    );
    if !accept {
        notify_challenger(
            state,
            &challenger_target,
            "Battleship Challenge Declined",
            &format!("{} declined your Battleship challenge.", my_display),
        );
        return resp(200, &json!({ "ok": true, "declined": true }));
    }
    notify_challenger(
        state,
        &challenger_target,
        "Battleship Challenge Accepted",
        &format!("{} accepted your Battleship challenge!", my_display),
    );
    let game_id = mitch_lib::crypto::random_bytes_hex(8);
    let now = now_millis();
    let mut boards = IndexMap::new();
    boards.insert(
        c.from.clone(),
        BsBoard {
            ships: None,
            hits: Vec::new(),
            misses: Vec::new(),
        },
    );
    boards.insert(
        my_norm.to_string(),
        BsBoard {
            ships: None,
            hits: Vec::new(),
            misses: Vec::new(),
        },
    );
    state
        .bs_games
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            game_id.clone(),
            BsGame {
                id: game_id.clone(),
                player1: c.from.clone(),
                player2: my_norm.to_string(),
                // `c.bet || 0` — bet is never NaN/0-coerced ambiguity here
                // (clamped to ≥0 at creation), so it is the identity.
                bet: c.bet,
                status: "placing".to_string(),
                boards,
                placed_by: Vec::new(),
                turn: None,
                winner: None,
                result: None,
                created_at: now,
                last_action_at: now,
                hit_log: Vec::new(),
                collusion_flag: false,
            },
        );
    resp(200, &json!({ "ok": true, "gameId": game_id }))
}

// ── `/place` (server.js:20860-20897) ─────────────────────────────────────────

const SHIP_SIZES: [i64; 5] = [5, 4, 3, 3, 2];

/// `String(number)` — integer-valued f64 renders without the decimal
/// (`"3"`), NaN renders as `"NaN"` (JS template keys `${r},${c2}`).
fn js_num_str(n: f64) -> String {
    if n.is_nan() {
        "NaN".to_string()
    } else {
        jsval::num_value(n).to_string()
    }
}

/// `Number(body.field)` where a MISSING key is JS `undefined` → NaN
/// (while an explicit `null` is `Number(null)` → 0 — `jsval::number`).
fn js_num_field(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(jsval::number).unwrap_or(f64::NAN)
}

/// The /place ship-validation loop (server.js:20870-20889) — `Number()`
/// coercion, the `'v'` strict-equality dir fallback, NaN-passing bounds,
/// 10×10 tail check and the overlap set. Returns the JS error string.
fn normalize_ships(ships: &Value) -> Result<Vec<BsShip>, &'static str> {
    let Some(arr) = ships.as_array() else {
        return Err("must place exactly 5 ships");
    };
    if arr.len() != 5 {
        return Err("must place exactly 5 ships");
    }
    let mut occupied: HashSet<String> = HashSet::new();
    let mut out: Vec<BsShip> = Vec::new();
    for (i, s) in arr.iter().enumerate() {
        let row = js_num_field(s, "row");
        let col = js_num_field(s, "col");
        let dir = if jsval::string_of(s.get("dir")) == "v" {
            "v"
        } else {
            "h"
        };
        let size = SHIP_SIZES[i];
        // JS `row < 0 || row > 9 || col < 0 || col > 9` — NaN fails both
        // comparisons and passes.
        if row < 0.0 || row > 9.0 || col < 0.0 || col > 9.0 {
            return Err("ship out of bounds");
        }
        let mut cells = Vec::new();
        for j in 0..size {
            let r = if dir == "v" { row + j as f64 } else { row };
            let c2 = if dir == "h" { col + j as f64 } else { col };
            if r > 9.0 || c2 > 9.0 {
                return Err("ship out of bounds");
            }
            let key = format!("{},{}", js_num_str(r), js_num_str(c2));
            if occupied.contains(&key) {
                return Err("ships overlap");
            }
            cells.push(key);
        }
        for k in &cells {
            occupied.insert(k.clone());
        }
        out.push(BsShip {
            row,
            col,
            dir: dir.to_string(),
            size,
            cells,
            hits: 0,
        });
    }
    Ok(out)
}

fn place(
    state: &Arc<AppState>,
    body: &Value,
    my_email: &str,
    my_norm: &str,
) -> axum::response::Response {
    let game_id = jsval::string_of(body.get("gameId"));
    let mut games = state.bs_games.lock().unwrap_or_else(|e| e.into_inner());
    let Some(g) = games.get_mut(&game_id) else {
        return resp(404, &json!({ "error": "game not found" }));
    };
    if mitch_lib::auth::normalize_email(&g.player1) != my_norm
        && mitch_lib::auth::normalize_email(&g.player2) != my_norm
    {
        return resp(403, &json!({ "error": "not your game" }));
    }
    if g.status != "placing" {
        return resp(400, &json!({ "error": "placement phase over" }));
    }
    // JS compares the RAW lowercase email against the normalized entries
    // (server.js:20867) — literal quirk, not normalized here.
    if g.placed_by.iter().any(|e| e == my_email) {
        return resp(400, &json!({ "error": "already placed" }));
    }
    let ships = match normalize_ships(body.get("ships").unwrap_or(&Value::Null)) {
        Ok(s) => s,
        Err(e) => return resp(400, &json!({ "error": e })),
    };
    let Some(board) = g.boards.get_mut(my_norm) else {
        return resp(500, &json!({}));
    };
    board.ships = Some(ships);
    g.placed_by.push(my_norm.to_string());
    if g.placed_by.len() == 2 {
        g.turn = Some(
            if rand::random::<f64>() < 0.5 {
                g.player1.clone()
            } else {
                g.player2.clone()
            }
            .to_string(),
        );
        g.status = "active".to_string();
    }
    let waiting = g.placed_by.len() < 2;
    resp(200, &json!({ "ok": true, "waiting": waiting }))
}

// ── `/fire` (server.js:20899-20964) ──────────────────────────────────────────

fn fire(state: &Arc<AppState>, body: &Value, my_norm: &str) -> axum::response::Response {
    let game_id = jsval::string_of(body.get("gameId"));
    let mut games = state.bs_games.lock().unwrap_or_else(|e| e.into_inner());
    let Some(g) = games.get_mut(&game_id) else {
        return resp(404, &json!({ "error": "game not found" }));
    };
    if mitch_lib::auth::normalize_email(&g.player1) != my_norm
        && mitch_lib::auth::normalize_email(&g.player2) != my_norm
    {
        return resp(403, &json!({ "error": "not your game" }));
    }
    if g.status != "active" {
        return resp(400, &json!({ "error": "game not active" }));
    }
    // `normalizeEmail(g.turn) !== myNorm` — a null turn normalizes to ''.
    let turn_norm = g
        .turn
        .as_deref()
        .map(mitch_lib::auth::normalize_email)
        .unwrap_or_default();
    if turn_norm != my_norm {
        return resp(400, &json!({ "error": "not your turn" }));
    }
    let row = js_num_field(body, "row");
    let col = js_num_field(body, "col");
    if row.is_nan() || col.is_nan() || row < 0.0 || row > 9.0 || col < 0.0 || col > 9.0 {
        return resp(400, &json!({ "error": "invalid coordinate" }));
    }
    let opp_email = if mitch_lib::auth::normalize_email(&g.player1) == my_norm {
        mitch_lib::auth::normalize_email(&g.player2)
    } else {
        mitch_lib::auth::normalize_email(&g.player1)
    };
    let Some(opp_board) = g.boards.get_mut(&opp_email) else {
        return resp(500, &json!({}));
    };
    let coord_key = format!("{},{}", js_num_str(row), js_num_str(col));
    if opp_board.hits.contains(&coord_key) || opp_board.misses.contains(&coord_key) {
        return resp(400, &json!({ "error": "already fired there" }));
    }
    let Some(ships) = opp_board.ships.as_mut() else {
        // JS: `oppBoard.ships.every` on null → uncaught TypeError → 500.
        return resp(500, &json!({}));
    };
    let now = now_millis();
    g.last_action_at = now;
    let mut result = "miss";
    let mut sunk = false;
    let mut sunk_ship: Value = Value::Null;
    let hit_idx = ships.iter().position(|s| s.cells.contains(&coord_key));
    if let Some(idx) = hit_idx {
        opp_board.hits.push(coord_key);
        let ship = &mut ships[idx];
        ship.hits += 1;
        result = "hit";
        g.hit_log.push(BsHit {
            by: my_norm.to_string(),
            at: now,
        });
        if ship.hits >= ship.size {
            sunk = true;
            result = "sunk";
        }
        // Anti-cheat: check collusion (server.js:20931-20937).
        if g.hit_log.len() >= 6 {
            let recent = &g.hit_log[g.hit_log.len() - 6..];
            let span = recent[recent.len() - 1].at - recent[0].at;
            let total_fired = opp_board.hits.len() + opp_board.misses.len();
            let hit_rate = opp_board.hits.len() as f64 / std::cmp::max(total_fired, 1) as f64;
            if (span as f64) < 12000.0 && hit_rate > 0.85 && total_fired <= 15 {
                g.collusion_flag = true;
            }
        }
        if sunk {
            sunk_ship = json!({ "size": ship.size, "cells": ship.cells });
        }
    } else {
        opp_board.misses.push(coord_key);
    }
    let all_sunk = ships.iter().all(|s| s.hits >= s.size);
    if all_sunk {
        g.status = "over".to_string();
        g.winner = Some(my_norm.to_string());
        g.result = Some(format!("{} wins", my_norm));
        if !g.collusion_flag {
            // `g.bet || 0` — bet is never NaN (clamped at creation).
            let bet = if g.bet == 0.0 { 0.0 } else { g.bet };
            let mut win_coins = 40.0 + bet * 2.0;
            let friend = are_friends(state, &g.player1, &g.player2);
            if friend {
                win_coins = (win_coins * 1.5).floor();
            }
            let premium = mitch_lib::auth::is_premium_email(&state.store, my_norm);
            if premium {
                win_coins = (win_coins * 2.0).floor();
            }
            mitch_lib::coins::add_coins(
                &state.store,
                &state.cfg.data_dir,
                my_norm,
                win_coins,
                state.coin_multiplier(),
                &format!(
                    "battleship: win payout (bet={}, friend={}, premium={})",
                    js_num_str(g.bet),
                    friend,
                    premium
                ),
            );
            mitch_lib::achievements::update_stat(
                &state.store,
                &state.cfg.data_dir,
                my_norm,
                "battleship_wins",
                1.0,
                state.coin_multiplier(),
            );
        } else {
            g.result = Some(format!("{} wins (collusion detected — no payout)", my_norm));
        }
    } else if result == "miss" {
        g.turn = Some(opp_email);
    }
    resp(
        200,
        &json!({
            "ok": true,
            "result": result,
            "sunk": sunk,
            "sunkShip": sunk_ship,
            "gameOver": all_sunk,
            "collusionFlag": g.collusion_flag,
        }),
    )
}

// ── `/state` (server.js:20966-20988) ─────────────────────────────────────────

/// Ships → JSON (row/col are JS numbers: integer-valued f64 without the
/// decimal, NaN → `null` like `JSON.stringify`).
fn ships_json(ships: Option<&Vec<BsShip>>) -> Value {
    match ships {
        None => Value::Null,
        Some(list) => Value::Array(
            list.iter()
                .map(|s| {
                    json!({
                        "row": js_num_value_or_null(s.row),
                        "col": js_num_value_or_null(s.col),
                        "dir": s.dir,
                        "size": s.size,
                        "cells": s.cells,
                        "hits": s.hits,
                    })
                })
                .collect(),
        ),
    }
}

/// A finite number → JSON number (integer-valued f64 without the decimal);
/// NaN/Infinity → `null` like `JSON.stringify`.
fn js_num_value_or_null(n: f64) -> Value {
    if n.is_nan() || n.is_infinite() {
        Value::Null
    } else {
        jsval::num_value(n)
    }
}

/// `` g.result.replace(new RegExp(g.player1, 'g'), …) `` — the email is the
/// UNESCAPED pattern (a bare `.` matches any char, quirk preserved); an
/// invalid pattern throws in JS → 500 here.
fn mask_result(result: &str, player1: &str, player2: &str) -> Option<String> {
    let mut out = result.to_string();
    for p in [player1, player2] {
        let re = match regex::Regex::new(p) {
            Ok(re) => re,
            Err(_) => return None,
        };
        out = re
            .replace_all(&out, mitch_lib::admin::mask_email(p).as_str())
            .to_string();
    }
    Some(out)
}

fn state_view(
    state: &Arc<AppState>,
    search: &str,
    my_email: &str,
    my_norm: &str,
) -> axum::response::Response {
    let game_id = crate::handler::query(search)
        .get("id")
        .cloned()
        .unwrap_or_default();
    let games = state.bs_games.lock().unwrap_or_else(|e| e.into_inner());
    let Some(g) = games.get(&game_id) else {
        return resp(404, &json!({ "error": "game not found" }));
    };
    if mitch_lib::auth::normalize_email(&g.player1) != my_norm
        && mitch_lib::auth::normalize_email(&g.player2) != my_norm
    {
        return resp(403, &json!({ "error": "not your game" }));
    }
    let opp_email = if my_norm == mitch_lib::auth::normalize_email(&g.player1) {
        mitch_lib::auth::normalize_email(&g.player2)
    } else {
        mitch_lib::auth::normalize_email(&g.player1)
    };
    let Some(my_board) = g.boards.get(my_norm) else {
        return resp(500, &json!({}));
    };
    let Some(opp_board) = g.boards.get(&opp_email) else {
        return resp(500, &json!({}));
    };
    let result = match g.result.as_ref() {
        None => Value::Null,
        Some(r) => match mask_result(r, &g.player1, &g.player2) {
            Some(m) => Value::String(m),
            None => return resp(500, &json!({})),
        },
    };
    resp(
        200,
        &json!({
            "myEmail": mitch_lib::admin::mask_email(my_email),
            "game": {
                "id": g.id,
                "status": g.status,
                "turn": g.turn.as_ref().map(|t| json!(mitch_lib::admin::mask_email(t))).unwrap_or(Value::Null),
                "winner": g.winner.as_ref().map(|w| json!(mitch_lib::admin::mask_email(w))).unwrap_or(Value::Null),
                "result": result,
                "collusionFlag": g.collusion_flag,
                "placedBy": Value::Array(g.placed_by.iter().map(|p| json!(mitch_lib::admin::mask_email(p))).collect()),
                "myShips": ships_json(my_board.ships.as_ref()),
                "myHits": my_board.hits,
                "myMisses": my_board.misses,
                "oppHits": opp_board.hits,
                "oppMisses": opp_board.misses,
                "oppShips": if g.status == "over" { ships_json(opp_board.ships.as_ref()) } else { Value::Null },
                "bet": jsval::num_value(g.bet),
                "player1": mitch_lib::admin::mask_email(&g.player1),
                "player2": mitch_lib::admin::mask_email(&g.player2),
                "myBoardPlaced": my_board.ships.is_some(),
                "oppBoardPlaced": opp_board.ships.is_some(),
            },
        }),
    )
}

// ── `/resign` (server.js:20990-21006) ────────────────────────────────────────

fn resign(state: &Arc<AppState>, body: &Value, my_norm: &str) -> axum::response::Response {
    let game_id = jsval::string_of(body.get("gameId"));
    let mut games = state.bs_games.lock().unwrap_or_else(|e| e.into_inner());
    let Some(g) = games.get_mut(&game_id) else {
        return resp(403, &json!({}));
    };
    if mitch_lib::auth::normalize_email(&g.player1) != my_norm
        && mitch_lib::auth::normalize_email(&g.player2) != my_norm
    {
        return resp(403, &json!({}));
    }
    if g.status != "active" && g.status != "placing" {
        return resp(400, &json!({ "error": "game already over" }));
    }
    let opp_email = if my_norm == mitch_lib::auth::normalize_email(&g.player1) {
        mitch_lib::auth::normalize_email(&g.player2)
    } else {
        mitch_lib::auth::normalize_email(&g.player1)
    };
    g.status = "over".to_string();
    g.winner = Some(opp_email.clone());
    g.result = Some(format!("{} resigned", my_norm));
    if !g.collusion_flag {
        // `(g.bet || 0) * 2` — bet is never NaN (clamped at creation).
        let mut win_coins = 40.0 + g.bet * 2.0;
        let friend = are_friends(state, &g.player1, &g.player2);
        if friend {
            win_coins = (win_coins * 1.5).floor();
        }
        let premium = mitch_lib::auth::is_premium_email(&state.store, &opp_email);
        if premium {
            win_coins = (win_coins * 2.0).floor();
        }
        mitch_lib::coins::add_coins(
            &state.store,
            &state.cfg.data_dir,
            &opp_email,
            win_coins,
            state.coin_multiplier(),
            &format!("battleship: win on resign (opponent={})", my_norm),
        );
        mitch_lib::achievements::update_stat(
            &state.store,
            &state.cfg.data_dir,
            &opp_email,
            "battleship_wins",
            1.0,
            state.coin_multiplier(),
        );
    }
    resp(200, &json!({ "ok": true }))
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

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_ships_accepts_and_normalizes() {
        let ships = json!([
            { "row": 0, "col": 0, "dir": "h" },
            { "row": "1", "col": 0, "dir": "v" }, // string coords coerce
            { "row": 2, "col": 4, "dir": "H" },   // strict 'v' check → 'h'
            { "row": 9, "col": 0, "dir": "l" },   // unknown dir → 'h'
            { "row": 8, "col": 5, "dir": "v" },
        ]);
        let out = normalize_ships(&ships).unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(out.len(), 5);
        assert_eq!(out[0].size, 5);
        assert_eq!(out[0].cells, vec!["0,0", "0,1", "0,2", "0,3", "0,4"]);
        assert_eq!(out[1].size, 4);
        assert_eq!(out[1].row, 1.0);
        assert_eq!(out[1].cells, vec!["1,0", "2,0", "3,0", "4,0"]);
        assert_eq!(out[2].dir, "h");
        assert_eq!(out[3].dir, "h");
        assert_eq!(out[4].cells, vec!["8,5", "9,5"]);
        assert!(out.iter().all(|s| s.hits == 0));
    }

    #[test]
    fn normalize_ships_rejects() {
        // Wrong count / non-array.
        assert_eq!(
            normalize_ships(&json!([1, 2, 3, 4])).unwrap_err(),
            "must place exactly 5 ships"
        );
        assert_eq!(
            normalize_ships(&json!("nope")).unwrap_err(),
            "must place exactly 5 ships"
        );
        // Out of bounds at the head.
        assert_eq!(
            normalize_ships(&json!([
                { "row": 10, "col": 0, "dir": "h" }, { "row": 0, "col": 6, "dir": "h" },
                { "row": 2, "col": 0, "dir": "h" }, { "row": 2, "col": 5, "dir": "h" },
                { "row": 4, "col": 7, "dir": "v" },
            ]))
            .unwrap_err(),
            "ship out of bounds"
        );
        // Out of bounds at the tail (row 7 + 4 vertical cells > 9).
        assert_eq!(
            normalize_ships(&json!([
                { "row": 0, "col": 0, "dir": "h" }, { "row": 7, "col": 0, "dir": "v" },
                { "row": 0, "col": 5, "dir": "h" }, { "row": 2, "col": 0, "dir": "h" },
                { "row": 2, "col": 5, "dir": "h" },
            ]))
            .unwrap_err(),
            "ship out of bounds"
        );
        // Overlap.
        assert_eq!(
            normalize_ships(&json!([
                { "row": 0, "col": 0, "dir": "h" }, { "row": 0, "col": 2, "dir": "h" },
                { "row": 2, "col": 0, "dir": "h" }, { "row": 2, "col": 5, "dir": "h" },
                { "row": 4, "col": 7, "dir": "v" },
            ]))
            .unwrap_err(),
            "ships overlap"
        );
    }

    #[test]
    fn nan_coords_pass_the_js_bounds_check() {
        // JS `NaN < 0 || NaN > 9` is false — NaN slips through and lands in
        // the cells as "NaN,NaN" (unfireable, serialized as null).
        let ships = json!([
            { "row": "abc", "col": 0, "dir": "h" }, { "row": 0, "col": 6, "dir": "h" },
            { "row": 2, "col": 0, "dir": "h" }, { "row": 2, "col": 5, "dir": "h" },
            { "row": 4, "col": 7, "dir": "v" },
        ]);
        let out = normalize_ships(&ships).unwrap_or_else(|e| panic!("{}", e));
        assert!(out[0].row.is_nan());
        assert_eq!(
            out[0].cells,
            vec!["NaN,0", "NaN,1", "NaN,2", "NaN,3", "NaN,4"]
        );
        // An explicit null is Number(null) → 0, NOT NaN.
        let null_row = normalize_ships(&json!([
            { "row": null, "col": 0, "dir": "h" }, { "row": 0, "col": 6, "dir": "h" },
            { "row": 2, "col": 0, "dir": "h" }, { "row": 2, "col": 5, "dir": "h" },
            { "row": 4, "col": 7, "dir": "v" },
        ]))
        .unwrap_or_else(|e| panic!("{}", e));
        assert_eq!(null_row[0].row, 0.0);
    }

    #[test]
    fn js_num_str_matches_js_string() {
        assert_eq!(js_num_str(3.0), "3");
        assert_eq!(js_num_str(2.5), "2.5");
        assert_eq!(js_num_str(f64::NAN), "NaN");
        assert_eq!(js_num_str(0.0), "0");
        assert_eq!(js_num_str(-1.0), "-1");
    }

    #[test]
    fn mask_result_replaces_both_players() {
        // maskEmail is a passthrough; the replace uses an UNESCAPED regex
        // (the bare `.` wildcard, quirk preserved).
        let out = mask_result("a@x.rjuhsd.us resigned", "a@x.rjuhsd.us", "b@x.rjuhsd.us")
            .unwrap_or_default();
        assert_eq!(out, "a@x.rjuhsd.us resigned");
        // The wildcard quirk: '.' matches any single char, so the stored
        // email also replaces lookalike text.
        let out = mask_result("aZx wins", "a.x", "b@x.rjuhsd.us").unwrap_or_default();
        assert_eq!(out, "a.x wins");
        // Both emails in one result string.
        let out = mask_result("a.x wins", "a.x", "b.x").unwrap_or_default();
        assert_eq!(out, "a.x wins");
    }

    #[test]
    fn mask_result_invalid_pattern_is_none() {
        // A `(` in an email would throw in JS (RegExp compile) → the 500
        // path here.
        assert!(mask_result("x wins", "(bad", "b@x.rjuhsd.us").is_none());
    }
}
