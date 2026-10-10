//! `/api/jeopardy/*` — the multiplayer trivia game (plan Step 12; the
//! prelude + 11 endpoints at server.js:21010-21560, the clue-cache /
//! board-builder / matcher helpers at server.js:889-1055).
//!
//! State model: `jeopardyLobbies` (server.js:888) is an in-memory
//! insertion-ordered map — a `Vec` here, because `/join` scans it in JS
//! `Object.values` order and `Object.entries` order shapes the score/board
//! output. The clue cache (`jeopardyClueCache` + `jeopardyLastFetch`,
//! server.js:889-892) lives in `AppState.jeopardy_clues` and is lazily
//! refreshed from `data/jeopardy_kids_clean.json` on the JS 24h TTL. The
//! timeout machine (`jeopardyCheckTimeouts`) owns no timer — like JS it is
//! evaluated inside `/api/jeopardy/state` only.
//!
//! The answer matcher is the local fuzzy check only. The JS falls back to a
//! Groq call when the local checks fail; that fallback is deliberately not
//! ported (mitch.pro AI was decommissioned) — where JS would call Groq the
//! port returns false.

use axum::http::{HeaderMap, Method};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::sync::Arc;

use crate::errors::json_resp_str;
use crate::state::AppState;
use mitch_lib::data::js_stringify;
use mitch_lib::jsval;

fn resp(code: u16, body: &Value) -> axum::response::Response {
    json_resp_str(code, js_stringify(body))
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ── Clue cache ───────────────────────────────────────────────────────────────

/// One flat clue from the clean cache (`{ category, clue, answer, value }`,
/// server.js:891).
#[derive(Clone)]
pub struct JeopardyClue {
    pub category: String,
    pub clue: String,
    pub answer: String,
    // Board values always come from the 200-1000 VALUES ladder (the JS board
    // builder never reads the cache entry's value back) — kept for cache
    // shape parity.
    #[allow(dead_code)]
    pub value: i64,
}

/// `jeopardyClueCache` + `jeopardyLastFetch` (server.js:889-892).
#[derive(Default)]
pub struct ClueCache {
    pub clues: Vec<JeopardyClue>,
    pub last_fetch: i64,
}

const JEOPARDY_CACHE_TTL_MS: i64 = 24 * 60 * 60 * 1000;

impl ClueCache {
    /// The clean-cache path of `loadJeopardyClues()` (server.js:894-901), run
    /// lazily under the 24h TTL the JS hourly interval enforces. The
    /// TSV-download bootstrap (fresh deploy without the cache file) is
    /// deliberately not ported — an empty cache surfaces as the JS
    /// `503 clue database not yet loaded` on /start and `clueDbReady: false`
    /// on /state, and prod carries the cache file.
    fn refresh_if_stale(&mut self, state: &AppState) {
        let now = now_millis();
        if self.last_fetch != 0 && now - self.last_fetch <= JEOPARDY_CACHE_TTL_MS {
            return;
        }
        let raw: Value = state.store.read_document(
            &state.data_dir().join("jeopardy_kids_clean.json"),
            json!([]),
        );
        let mut clues = Vec::new();
        if let Some(arr) = raw.as_array() {
            for c in arr {
                let (Some(category), Some(clue), Some(answer)) = (
                    c.get("category").and_then(|v| v.as_str()),
                    c.get("clue").and_then(|v| v.as_str()),
                    c.get("answer").and_then(|v| v.as_str()),
                ) else {
                    continue;
                };
                let value = c.get("value").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64;
                clues.push(JeopardyClue {
                    category: category.to_string(),
                    clue: clue.to_string(),
                    answer: answer.to_string(),
                    value,
                });
            }
        }
        self.clues = clues;
        self.last_fetch = now;
    }
}

/// A snapshot of the live cache (refreshing it first), cloned for the board
/// builder / final-clue picker.
fn load_clues(state: &AppState) -> Vec<JeopardyClue> {
    let mut cache = state
        .jeopardy_clues
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    cache.refresh_if_stale(state);
    cache.clues.clone()
}

/// Round-trip-safe `f64` NaN <-> JSON `null` mapping for `ActiveClue.wager`'s
/// NaN-as-unset sentinel, used only for on-disk persistence.
mod nan_as_null {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &f64, s: S) -> Result<S::Ok, S::Error> {
        if value.is_nan() {
            s.serialize_none()
        } else {
            s.serialize_some(value)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        Ok(Option::<f64>::deserialize(d)?.unwrap_or(f64::NAN))
    }
}

// ── Lobby state model ────────────────────────────────────────────────────────

/// One board cell (server.js:1024-1031).
#[derive(Clone, Serialize, Deserialize)]
pub struct BoardClue {
    pub value: i64,
    pub clue: String,
    pub answer: String,
    pub answered: bool,
    pub daily_double: bool,
    pub answered_by: Option<String>,
}

/// One `activeClue` (server.js:21405-21416). `wager` uses NaN as the JS
/// null/unset sentinel — the only falsy semantics the code relies on is
/// `activeClue.wager || val` (null, 0 and NaN all fall back to the clue's
/// value), so a NaN flag covers every construction site.
#[derive(Clone, Serialize, Deserialize)]
pub struct ActiveClue {
    pub cat: String,
    pub val: f64,
    pub clue: String,
    pub answer: String,
    pub is_daily_double: bool,
    /// buzzing | wagering | answering | reveal
    pub phase: String,
    pub buzz_open_at: Option<i64>,
    pub wager_open_at: Option<i64>,
    pub answer_deadline: Option<i64>,
    pub buzzed_by: Option<String>,
    pub buzzer: Option<i64>,
    // Persisted as JSON null when NaN (see `nan_as_null`) — serde_json's
    // Value conversion already turns a NaN f64 into null on the way out,
    // but deserializing null back into a plain f64 field errors without
    // this, which would otherwise fail every lobby with a daily-double in
    // flight when reloading data/jeopardy_lobbies.json after a restart.
    #[serde(with = "nan_as_null")]
    pub wager: f64,
    pub wager_by: Option<String>,
    pub tab_penalty_for: Vec<String>,
    pub wrong_players: Vec<String>,
    /// JS leaves this undefined until a reveal — serialized by omission.
    pub revealed_correct: Option<bool>,
    pub reveal_close_at: Option<i64>,
}

/// One `finalJeopardy.revealedAnswers[p]` entry (server.js:21025-21031).
#[derive(Clone, Serialize, Deserialize)]
pub struct RevealedAnswer {
    pub answer: String,
    pub correct: bool,
    pub score_change: i64,
    pub wager: i64,
}

/// The final-jeopardy round (server.js:21493-21502).
#[derive(Clone, Serialize, Deserialize)]
pub struct FinalJeopardy {
    pub cat: String,
    pub clue: String,
    pub answer: String,
    pub wagers: IndexMap<String, i64>,
    pub answers: IndexMap<String, String>,
    /// wagering | answering | reveal
    pub phase: String,
    pub wager_deadline: Option<i64>,
    pub answer_deadline: Option<i64>,
    pub reveal_close_at: Option<i64>,
    pub revealed_answers: IndexMap<String, RevealedAnswer>,
}

/// One `jeopardyLobbies[gameId]` (server.js:21252-21259). The keyed maps use
/// serde_json `Map` (insertion-ordered under the workspace's
/// `preserve_order` feature) so `Object.entries` order matches.
///
/// Persisted in full (unmasked) to `data/jeopardy_lobbies.json` after every
/// mutating request — see `persist_lobbies`/`load_lobbies` — so a server
/// restart, including a blue-green deploy swap, resumes in-progress games
/// instead of resetting them.
#[derive(Clone, Serialize, Deserialize)]
pub struct JeopardyLobby {
    pub id: String,
    pub join_code: String,
    pub host: String,
    pub players: Vec<String>,
    pub max_players: i64,
    /// lobby | active | final_jeopardy | over
    pub status: String,
    pub scores: IndexMap<String, i64>,
    pub categories: Option<Vec<String>>,
    pub board: Option<IndexMap<String, Vec<BoardClue>>>,
    pub active_clue: Option<ActiveClue>,
    pub turn: String,
    // bun stores createdAt but nothing reads it back.
    #[allow(dead_code)]
    pub created_at: i64,
    pub tab_hidden: IndexMap<String, bool>,
    pub round_over: bool,
    pub final_score: Option<IndexMap<String, i64>>,
    pub final_jeopardy: Option<FinalJeopardy>,
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
    if !path.starts_with("/api/jeopardy/") {
        return None;
    }
    // Auth ladder (server.js:21012-21019). The JS `checkRateLimit` call runs
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

    // `tryParseJson` (server.js:10514) — empty body → {}, parse failure →
    // 400 'bad json'. /state never parses a body.
    let body = if path == "/api/jeopardy/state" || body_bytes.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice(body_bytes) {
            Ok(b) => b,
            Err(_) => return Some(resp(400, &json!({ "error": "bad json" }))),
        }
    };

    // Unknown /api/jeopardy/* paths pass the auth ladder, then fall through
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
    const KNOWN: [&str; 11] = [
        "/api/jeopardy/state",
        "/api/jeopardy/create",
        "/api/jeopardy/join",
        "/api/jeopardy/start",
        "/api/jeopardy/select",
        "/api/jeopardy/wager",
        "/api/jeopardy/buzz",
        "/api/jeopardy/answer",
        "/api/jeopardy/visibility",
        "/api/jeopardy/final/wager",
        "/api/jeopardy/final/answer",
    ];
    if !KNOWN.contains(&path) {
        return None;
    }
    if path == "/api/jeopardy/state" {
        return Some(state_ep(state, search, my_email, my_norm));
    }
    let mut lobbies = state
        .jeopardy_lobbies
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let out = match path {
        "/api/jeopardy/create" => create(&mut lobbies, body, my_norm),
        "/api/jeopardy/join" => join(&mut lobbies, body, my_norm),
        "/api/jeopardy/start" => start(state, &mut lobbies, body, my_norm),
        "/api/jeopardy/select" => select(&mut lobbies, body, my_norm),
        "/api/jeopardy/wager" => wager(&mut lobbies, body, my_norm),
        "/api/jeopardy/buzz" => buzz(&mut lobbies, body, my_norm),
        "/api/jeopardy/answer" => answer(&mut lobbies, body, my_norm),
        "/api/jeopardy/visibility" => visibility(&mut lobbies, body, my_norm),
        "/api/jeopardy/final/wager" => final_wager(&mut lobbies, body, my_norm),
        "/api/jeopardy/final/answer" => final_answer(&mut lobbies, body, my_norm),
        _ => unreachable!(),
    };
    persist_lobbies(state, &lobbies);
    Some(out)
}

const JEOPARDY_LOBBIES_FILE: &str = "jeopardy_lobbies.json";

/// Snapshots every lobby, full fidelity and unmasked, to disk. Called after
/// every request that can mutate `jeopardy_lobbies` so a server restart —
/// including a blue-green deploy swap — resumes in-progress games instead of
/// silently resetting them (the previous behavior: lobbies lived in-memory
/// only and vanished on every deploy). Cheap: this feature only ever sees a
/// handful of concurrent games, so a full-snapshot write per request is
/// simpler and less error-prone than diffing or hooking every mutation site.
fn persist_lobbies(state: &AppState, lobbies: &[JeopardyLobby]) {
    if let Ok(value) = serde_json::to_value(lobbies) {
        let _ = state
            .store
            .write_document(&state.data_dir().join(JEOPARDY_LOBBIES_FILE), &value);
    }
}

/// Loads lobbies left over from before a restart (see `persist_lobbies`).
/// Finished games are dropped on load — nothing left to resume. Malformed
/// data (e.g. a hand-edited file) falls back to an empty list rather than
/// failing startup.
pub(crate) fn load_lobbies(
    store: &mitch_lib::data::DataStore,
    data_dir: &std::path::Path,
) -> Vec<JeopardyLobby> {
    let raw = store.read_document(&data_dir.join(JEOPARDY_LOBBIES_FILE), json!([]));
    let lobbies: Vec<JeopardyLobby> = serde_json::from_value(raw).unwrap_or_default();
    lobbies.into_iter().filter(|l| l.status != "over").collect()
}

/// `String(body.x || '')` — the JS `|| ''` first, then the String coercion.
fn falsy_string(body: &Value, key: &str) -> String {
    match body.get(key) {
        Some(v) if jsval::truthy(v) => jsval::string(v),
        _ => String::new(),
    }
}

/// `lobby.scores[p] = (lobby.scores[p] || 0) + delta`.
fn add_score(lobby: &mut JeopardyLobby, norm: &str, delta: i64) {
    *lobby.scores.entry(norm.to_string()).or_insert(0) += delta;
}

/// Push `norm` onto `wrongPlayers` if absent (the JS double guard).
fn push_wrong(ac: &mut ActiveClue, norm: &str) {
    if !ac.wrong_players.iter().any(|p| p == norm) {
        ac.wrong_players.push(norm.to_string());
    }
}

/// `lobby.board[cat].find(c => c.value === val)` mutation — the JS guards
/// `if (clue)` after the find.
fn mark_clue_answered(lobby: &mut JeopardyLobby, cat: &str, val: f64, answered_by: Option<String>) {
    if let Some(arr) = lobby.board.as_mut().and_then(|b| b.get_mut(cat)) {
        if let Some(c) = arr.iter_mut().find(|c| c.value as f64 == val) {
            c.answered = true;
            c.answered_by = answered_by;
        }
    }
}

/// The daily-double / open-buzz fallback for `activeClue.wager || val`.
fn effective_wager(ac: &ActiveClue, val: f64) -> f64 {
    if ac.wager.is_nan() || ac.wager == 0.0 {
        val
    } else {
        ac.wager
    }
}

impl JeopardyLobby {
    /// The active clue. Every reader sits behind the endpoint's phase ladder
    /// or the timeout machine's existence check — a missing clue here is a
    /// logic bug, not a runtime condition.
    fn active_clue_ref(&self) -> &ActiveClue {
        match self.active_clue.as_ref() {
            Some(ac) => ac,
            None => unreachable!("active clue guarded by the caller"),
        }
    }

    fn active_clue_mut(&mut self) -> &mut ActiveClue {
        match self.active_clue.as_mut() {
            Some(ac) => ac,
            None => unreachable!("active clue guarded by the caller"),
        }
    }

    /// The final-jeopardy round — `status == 'final_jeopardy'` guards every
    /// reader.
    fn fj_ref(&self) -> &FinalJeopardy {
        match self.final_jeopardy.as_ref() {
            Some(fj) => fj,
            None => unreachable!("final jeopardy guarded by the caller"),
        }
    }

    fn fj_mut(&mut self) -> &mut FinalJeopardy {
        match self.final_jeopardy.as_mut() {
            Some(fj) => fj,
            None => unreachable!("final jeopardy guarded by the caller"),
        }
    }
}

// ── Endpoints ────────────────────────────────────────────────────────────────

/// `POST /api/jeopardy/create` (server.js:21249-21262).
fn create(
    // Pushes a new lobby onto the front of the Vec — JS Object.values order.
    lobbies: &mut Vec<JeopardyLobby>,
    body: &Value,
    my_norm: &str,
) -> axum::response::Response {
    // Math.min(10, Math.max(2, parseInt(body.maxPlayers) || 10))
    let p = js_parse_int(body.get("maxPlayers").unwrap_or(&Value::Null));
    let p = if p.is_nan() { 10.0 } else { p };
    let max_players = 10f64.min(2f64.max(p)) as i64;
    let game_id = mitch_lib::crypto::random_bytes_hex(8);
    let join_code = mitch_lib::crypto::random_bytes_hex(3).to_uppercase();
    let mut scores = IndexMap::new();
    scores.insert(my_norm.to_string(), 0);
    lobbies.push(JeopardyLobby {
        id: game_id.clone(),
        join_code: join_code.clone(),
        host: my_norm.to_string(),
        players: vec![my_norm.to_string()],
        max_players,
        status: "lobby".to_string(),
        scores,
        categories: None,
        board: None,
        active_clue: None,
        turn: my_norm.to_string(),
        created_at: now_millis(),
        tab_hidden: IndexMap::new(),
        round_over: false,
        final_score: None,
        final_jeopardy: None,
    });
    resp(
        200,
        &json!({ "ok": true, "gameId": game_id, "joinCode": join_code }),
    )
}

/// `POST /api/jeopardy/join` (server.js:21263-21275).
fn join(lobbies: &mut [JeopardyLobby], body: &Value, my_norm: &str) -> axum::response::Response {
    let join_code = falsy_string(body, "joinCode")
        .to_uppercase()
        .trim()
        .to_string();
    let Some(idx) = lobbies
        .iter()
        .position(|l| l.join_code == join_code && l.status == "lobby")
    else {
        return resp(
            404,
            &json!({ "error": "Game not found or already started" }),
        );
    };
    let lobby = &mut lobbies[idx];
    if lobby.players.len() as i64 >= lobby.max_players {
        return resp(400, &json!({ "error": "Game is full" }));
    }
    if lobby.players.iter().any(|p| p == my_norm) {
        return resp(400, &json!({ "error": "Already in game" }));
    }
    lobby.players.push(my_norm.to_string());
    lobby.scores.insert(my_norm.to_string(), 0);
    resp(200, &json!({ "ok": true, "gameId": lobby.id }))
}

/// `POST /api/jeopardy/start` (server.js:21276-21298).
fn start(
    state: &AppState,
    lobbies: &mut [JeopardyLobby],
    body: &Value,
    my_norm: &str,
) -> axum::response::Response {
    let game_id = falsy_string(body, "gameId");
    let Some(lobby) = lobbies.iter_mut().find(|l| l.id == game_id) else {
        return resp(404, &json!({ "error": "game not found" }));
    };
    if lobby.host != my_norm {
        return resp(403, &json!({ "error": "only host can start" }));
    }
    if lobby.status != "lobby" {
        return resp(400, &json!({ "error": "game already started" }));
    }
    if lobby.players.len() < 2 {
        return resp(400, &json!({ "error": "need at least 2 players" }));
    }
    let clues = load_clues(state);
    if clues.len() < 100 {
        return resp(
            503,
            &json!({
                "error": "Jeopardy clue database not yet loaded, please try again in a moment"
            }),
        );
    }
    let Some((categories, board, _daily_doubles)) = build_jeopardy_board(&clues) else {
        return resp(503, &json!({ "error": "Could not build board, try again" }));
    };
    lobby.board = Some(board);
    lobby.categories = Some(categories);
    lobby.status = "active".to_string();
    lobby.turn = lobby.players[mitch_lib::crypto::js_random_index(lobby.players.len())].clone();
    resp(200, &json!({ "ok": true }))
}

/// `POST /api/jeopardy/select` (server.js:21299-21325).
fn select(lobbies: &mut [JeopardyLobby], body: &Value, my_norm: &str) -> axum::response::Response {
    let game_id = falsy_string(body, "gameId");
    let Some(lobby) = lobbies.iter_mut().find(|l| l.id == game_id) else {
        return resp(404, &json!({ "error": "game not found" }));
    };
    if !lobby.players.iter().any(|p| p == my_norm) {
        return resp(403, &json!({ "error": "not in game" }));
    }
    if lobby.status != "active" {
        return resp(400, &json!({ "error": "game not active" }));
    }
    if lobby.turn != my_norm {
        return resp(400, &json!({ "error": "not your turn to select" }));
    }
    if lobby.active_clue.is_some() {
        return resp(400, &json!({ "error": "a clue is already active" }));
    }
    let cat = falsy_string(body, "category");
    // Number(body.value) — NaN compares false against every clue value.
    let val = jsval::number(body.get("value").unwrap_or(&Value::Null)).unwrap_or(f64::NAN);
    let cat_ok = lobby
        .categories
        .as_ref()
        .is_some_and(|cs| cs.iter().any(|c| c == &cat));
    if !cat_ok {
        return resp(400, &json!({ "error": "invalid category" }));
    }
    let found = lobby
        .board
        .as_ref()
        .and_then(|b| b.get(&cat))
        .and_then(|arr| {
            arr.iter().position(|c| c.value as f64 == val).map(|i| {
                let c = &arr[i];
                (c.clue.clone(), c.answer.clone(), c.daily_double)
            })
        });
    let Some((clue_text, clue_answer, is_dd)) = found else {
        return resp(400, &json!({ "error": "invalid clue" }));
    };
    if lobby
        .board
        .as_ref()
        .and_then(|b| b.get(&cat))
        .is_some_and(|arr| arr.iter().any(|c| c.value as f64 == val && c.answered))
    {
        return resp(400, &json!({ "error": "already answered" }));
    }
    let now = now_millis();
    lobby.active_clue = Some(ActiveClue {
        cat,
        val,
        clue: clue_text,
        answer: clue_answer,
        is_daily_double: is_dd,
        phase: if is_dd { "wagering" } else { "buzzing" }.to_string(),
        buzz_open_at: if is_dd { None } else { Some(now + 2000) },
        wager_open_at: if is_dd { Some(now) } else { None },
        answer_deadline: None,
        buzzed_by: None,
        buzzer: None,
        wager: f64::NAN,
        wager_by: None,
        tab_penalty_for: Vec::new(),
        wrong_players: Vec::new(),
        revealed_correct: None,
        reveal_close_at: None,
    });
    resp(200, &json!({ "ok": true }))
}

/// `Math.min(maxWager, Math.max(0, Math.floor(Number(body.wager) || 0)))` —
/// shared by /wager and /final/wager (server.js:21349, 21511).
fn clamp_wager(my_score: f64, raw: Option<Value>) -> f64 {
    let max_wager = 1000f64.max(my_score);
    let n = match raw {
        Some(v) => jsval::number(&v).unwrap_or(f64::NAN),
        None => f64::NAN,
    };
    let n = if n.is_nan() { 0.0 } else { n };
    max_wager.min(0f64.max(n.floor()))
}

/// `POST /api/jeopardy/wager` (server.js:21326-21343).
fn wager(lobbies: &mut [JeopardyLobby], body: &Value, my_norm: &str) -> axum::response::Response {
    let game_id = falsy_string(body, "gameId");
    let Some(lobby) = lobbies.iter_mut().find(|l| l.id == game_id) else {
        return resp(403, &json!({}));
    };
    if !lobby.players.iter().any(|p| p == my_norm) {
        return resp(403, &json!({}));
    }
    let in_wagering = lobby
        .active_clue
        .as_ref()
        .is_some_and(|ac| ac.phase == "wagering");
    if !in_wagering {
        return resp(400, &json!({ "error": "not wagering phase" }));
    }
    if lobby.turn != my_norm {
        return resp(403, &json!({ "error": "only active player wagers" }));
    }
    let my_score = lobby.scores.get(my_norm).copied().unwrap_or(0) as f64;
    let w = clamp_wager(my_score, body.get("wager").cloned());
    let ac = lobby.active_clue_mut();
    ac.wager = w;
    ac.wager_by = Some(my_norm.to_string());
    ac.phase = "answering".to_string();
    ac.answer_deadline = Some(now_millis() + 30000);
    resp(200, &json!({ "ok": true, "wager": jsval::num_value(w) }))
}

/// `POST /api/jeopardy/buzz` (server.js:21344-21365).
fn buzz(lobbies: &mut [JeopardyLobby], body: &Value, my_norm: &str) -> axum::response::Response {
    let game_id = falsy_string(body, "gameId");
    let Some(lobby) = lobbies.iter_mut().find(|l| l.id == game_id) else {
        return resp(403, &json!({}));
    };
    if !lobby.players.iter().any(|p| p == my_norm) {
        return resp(403, &json!({}));
    }
    let Some(ac) = lobby.active_clue.as_mut() else {
        return resp(400, &json!({ "error": "not buzzing phase" }));
    };
    if ac.phase != "buzzing" {
        return resp(400, &json!({ "error": "not buzzing phase" }));
    }
    if ac.wrong_players.iter().any(|p| p == my_norm) {
        return resp(400, &json!({ "error": "already guessed incorrectly" }));
    }
    let now = now_millis();
    if let Some(bo) = ac.buzz_open_at {
        if now < bo {
            return resp(400, &json!({ "error": "buzzer not open yet" }));
        }
    }
    if ac.buzzed_by.is_some() {
        return resp(400, &json!({ "error": "someone already buzzed" }));
    }
    ac.buzzed_by = Some(my_norm.to_string());
    ac.buzzer = Some(now);
    ac.phase = "answering".to_string();
    ac.answer_deadline = Some(now + 20000);
    resp(200, &json!({ "ok": true, "buzzedAt": now }))
}

/// `POST /api/jeopardy/answer` (server.js:21366-21421).
fn answer(lobbies: &mut [JeopardyLobby], body: &Value, my_norm: &str) -> axum::response::Response {
    let game_id = falsy_string(body, "gameId");
    let Some(lobby) = lobbies.iter_mut().find(|l| l.id == game_id) else {
        return resp(403, &json!({}));
    };
    if !lobby.players.iter().any(|p| p == my_norm) {
        return resp(403, &json!({}));
    }
    let in_answering = lobby
        .active_clue
        .as_ref()
        .is_some_and(|ac| ac.phase == "answering");
    if !in_answering {
        return resp(400, &json!({ "error": "not answering phase" }));
    }
    let (is_dd, buzzed_by) = {
        let ac = lobby.active_clue_ref();
        (ac.is_daily_double, ac.buzzed_by.clone())
    };
    let expected = if is_dd {
        Some(lobby.turn.clone())
    } else {
        buzzed_by
    };
    if expected.as_deref() != Some(my_norm) {
        return resp(403, &json!({ "error": "not your turn to answer" }));
    }
    // String(body.answer || '').trim().slice(0, 300)
    let given = falsy_string(body, "answer")
        .trim()
        .chars()
        .take(300)
        .collect::<String>();
    let correct_answer = lobby.active_clue_ref().answer.clone();
    let now = now_millis();
    let past_deadline = lobby
        .active_clue_ref()
        .answer_deadline
        .is_some_and(|d| now > d);
    let tab_penalty = lobby
        .active_clue_ref()
        .tab_penalty_for
        .iter()
        .any(|p| p == my_norm);
    let mut correct = false;
    if !past_deadline && !tab_penalty && !given.is_empty() {
        correct = answer_matches(&given, &correct_answer);
    }
    let (cat, val) = {
        let ac = lobby.active_clue_ref();
        (ac.cat.clone(), ac.val)
    };
    let wager = effective_wager(lobby.active_clue_ref(), val);
    let score_change: f64;
    if correct {
        score_change = if is_dd { wager } else { val };
        add_score(lobby, my_norm, score_change as i64);
        mark_clue_answered(lobby, &cat, val, Some(my_norm.to_string()));
        let ac = lobby.active_clue_mut();
        ac.phase = "reveal".to_string();
        ac.revealed_correct = Some(true);
        lobby.turn = my_norm.to_string();
    } else {
        let penalty = if tab_penalty {
            -500.0
        } else if is_dd {
            -wager
        } else {
            -val
        };
        score_change = penalty;
        add_score(lobby, my_norm, penalty as i64);
        {
            let ac = lobby.active_clue_mut();
            push_wrong(ac, my_norm);
        }
        let remaining = {
            let wrong = &lobby.active_clue_ref().wrong_players;
            lobby.players.iter().filter(|p| !wrong.contains(p)).count()
        };
        if is_dd || remaining == 0 {
            let ac = lobby.active_clue_mut();
            ac.phase = "reveal".to_string();
            ac.revealed_correct = Some(false);
            mark_clue_answered(lobby, &cat, val, None);
        } else {
            let ac = lobby.active_clue_mut();
            ac.phase = "buzzing".to_string();
            ac.buzzed_by = None;
            ac.buzzer = None;
            ac.buzz_open_at = Some(now + 1000);
            ac.answer_deadline = None;
        }
    }
    let hide_correct = lobby.active_clue_ref().phase == "buzzing";
    let ac = lobby.active_clue_mut();
    ac.reveal_close_at = Some(now + 5000);
    resp(
        200,
        &json!({
            "ok": true,
            "correct": correct,
            "correctAnswer": if hide_correct { Value::Null } else { json!(correct_answer) },
            "scoreChange": jsval::num_value(score_change),
            "gameOver": false,
            "finalScore": Value::Null,
        }),
    )
}

/// `POST /api/jeopardy/visibility` (server.js:21422-21463).
fn visibility(
    lobbies: &mut [JeopardyLobby],
    body: &Value,
    my_norm: &str,
) -> axum::response::Response {
    let game_id = falsy_string(body, "gameId");
    let Some(lobby) = lobbies.iter_mut().find(|l| l.id == game_id) else {
        return resp(403, &json!({}));
    };
    if !lobby.players.iter().any(|p| p == my_norm) {
        return resp(403, &json!({}));
    }
    let hidden = body.get("hidden").is_some_and(jsval::truthy);
    let now = now_millis();
    if hidden && lobby.active_clue.is_some() {
        let phase = lobby.active_clue_ref().phase.clone();
        if matches!(phase.as_str(), "buzzing" | "answering" | "wagering") {
            let already = lobby
                .active_clue_ref()
                .tab_penalty_for
                .iter()
                .any(|p| p == my_norm);
            if !already {
                {
                    let ac = lobby.active_clue_mut();
                    ac.tab_penalty_for.push(my_norm.to_string());
                }
                // Deduct penalty immediately!
                add_score(lobby, my_norm, -500);
                let turn = lobby.turn.clone();
                let (is_dd, buzzed_by) = {
                    let ac = lobby.active_clue_ref();
                    (ac.is_daily_double, ac.buzzed_by.clone())
                };
                let expected = if is_dd { Some(turn) } else { buzzed_by };
                if expected.as_deref() == Some(my_norm)
                    && (phase == "answering" || phase == "wagering")
                {
                    let (cat, val) = {
                        let ac = lobby.active_clue_ref();
                        (ac.cat.clone(), ac.val)
                    };
                    {
                        let ac = lobby.active_clue_mut();
                        push_wrong(ac, my_norm);
                    }
                    let remaining = {
                        let wrong = &lobby.active_clue_ref().wrong_players;
                        lobby.players.iter().filter(|p| !wrong.contains(p)).count()
                    };
                    if is_dd || remaining == 0 {
                        let ac = lobby.active_clue_mut();
                        ac.phase = "reveal".to_string();
                        ac.revealed_correct = Some(false);
                        ac.reveal_close_at = Some(now + 5000);
                        mark_clue_answered(lobby, &cat, val, None);
                    } else {
                        let ac = lobby.active_clue_mut();
                        ac.phase = "buzzing".to_string();
                        ac.buzzed_by = None;
                        ac.buzzer = None;
                        ac.buzz_open_at = Some(now + 1000);
                        ac.answer_deadline = None;
                    }
                }
            }
        }
    }
    lobby.tab_hidden.insert(my_norm.to_string(), hidden);
    resp(200, &json!({ "ok": true }))
}

/// `POST /api/jeopardy/final/wager` (server.js:21464-21486).
fn final_wager(
    lobbies: &mut [JeopardyLobby],
    body: &Value,
    my_norm: &str,
) -> axum::response::Response {
    let game_id = falsy_string(body, "gameId");
    let Some(lobby) = lobbies.iter_mut().find(|l| l.id == game_id) else {
        return resp(403, &json!({}));
    };
    if !lobby.players.iter().any(|p| p == my_norm) {
        return resp(403, &json!({}));
    }
    let in_wagering = lobby
        .final_jeopardy
        .as_ref()
        .is_some_and(|fj| fj.phase == "wagering");
    if lobby.status != "final_jeopardy" || lobby.final_jeopardy.is_none() || !in_wagering {
        return resp(
            400,
            &json!({ "error": "not final jeopardy wagering phase" }),
        );
    }
    if lobby.fj_ref().wagers.contains_key(my_norm) {
        return resp(400, &json!({ "error": "already wagered" }));
    }
    let my_score = lobby.scores.get(my_norm).copied().unwrap_or(0) as f64;
    let w = clamp_wager(my_score, body.get("wager").cloned()) as i64;
    lobby.fj_mut().wagers.insert(my_norm.to_string(), w);
    let all_wagered = {
        let fj = lobby.fj_ref();
        lobby.players.iter().all(|p| fj.wagers.contains_key(p))
    };
    if all_wagered {
        let fj = lobby.fj_mut();
        fj.phase = "answering".to_string();
        fj.answer_deadline = Some(now_millis() + 30000);
    }
    resp(200, &json!({ "ok": true, "wager": w }))
}

/// `POST /api/jeopardy/final/answer` (server.js:21487-21505).
fn final_answer(
    lobbies: &mut [JeopardyLobby],
    body: &Value,
    my_norm: &str,
) -> axum::response::Response {
    let game_id = falsy_string(body, "gameId");
    let Some(lobby) = lobbies.iter_mut().find(|l| l.id == game_id) else {
        return resp(403, &json!({}));
    };
    if !lobby.players.iter().any(|p| p == my_norm) {
        return resp(403, &json!({}));
    }
    let in_answering = lobby
        .final_jeopardy
        .as_ref()
        .is_some_and(|fj| fj.phase == "answering");
    if lobby.status != "final_jeopardy" || lobby.final_jeopardy.is_none() || !in_answering {
        return resp(
            400,
            &json!({ "error": "not final jeopardy answering phase" }),
        );
    }
    if lobby.fj_ref().answers.contains_key(my_norm) {
        return resp(400, &json!({ "error": "already answered" }));
    }
    let answer = falsy_string(body, "answer")
        .trim()
        .chars()
        .take(300)
        .collect::<String>();
    lobby.fj_mut().answers.insert(my_norm.to_string(), answer);
    let all_answered = {
        let fj = lobby.fj_ref();
        lobby.players.iter().all(|p| fj.answers.contains_key(p))
    };
    if all_answered {
        evaluate_final_jeopardy(lobby);
    }
    resp(200, &json!({ "ok": true }))
}

// ── /state ───────────────────────────────────────────────────────────────────

/// `GET /api/jeopardy/state` (server.js:21176-21247) — runs the lazy timeout
/// machine first, then projects the lobby through the mask.
fn state_ep(
    state: &Arc<AppState>,
    search: &str,
    my_email: &str,
    my_norm: &str,
) -> axum::response::Response {
    let game_id = qs_get(search, "id").unwrap_or_default();
    // clueDbReady reads the live cache length (server.js:21245).
    let clues_len = {
        let mut cache = state
            .jeopardy_clues
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        cache.refresh_if_stale(state);
        cache.clues.len()
    };
    let mut lobbies = state
        .jeopardy_lobbies
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(lobby) = lobbies.iter_mut().find(|l| l.id == game_id) else {
        return resp(404, &json!({ "error": "game not found" }));
    };
    if !lobby.players.iter().any(|p| p == my_norm) {
        return resp(403, &json!({ "error": "not in this game" }));
    }
    check_timeouts(state, lobby);

    let mut safe_board = Map::new();
    if let Some(board) = &lobby.board {
        for (cat, clues) in board {
            let arr: Vec<Value> = clues
                .iter()
                .map(|cl| {
                    let show = lobby
                        .active_clue
                        .as_ref()
                        .is_some_and(|ac| ac.cat == *cat && ac.val == cl.value as f64);
                    json!({
                        "value": cl.value,
                        "answered": cl.answered,
                        "answeredBy": cl.answered_by.as_ref().map(|e| json!(mitch_lib::admin::mask_email(e))).unwrap_or(Value::Null),
                        "dailyDouble": cl.daily_double,
                        "clue": if show { json!(cl.clue) } else { Value::Null },
                    })
                })
                .collect();
            safe_board.insert(cat.clone(), Value::Array(arr));
        }
    }
    let masked_scores: Map<String, Value> = lobby
        .scores
        .iter()
        .map(|(e, s)| (mitch_lib::admin::mask_email(e), json!(s)))
        .collect();

    let active_clue = lobby.active_clue.as_ref().map(|ac| {
        let mut o = json!({
            "cat": ac.cat,
            "val": jsval::num_value(ac.val),
            "clue": ac.clue,
            "phase": ac.phase,
            "buzzer": ac.buzzer.map(|b| json!(b)).unwrap_or(Value::Null),
            "buzzedBy": ac.buzzed_by.as_ref().map(|e| json!(mitch_lib::admin::mask_email(e))).unwrap_or(Value::Null),
            "isDailyDouble": ac.is_daily_double,
            "wager": jsval::num_value(ac.wager),
            "wagerBy": ac.wager_by.as_ref().map(|e| json!(mitch_lib::admin::mask_email(e))).unwrap_or(Value::Null),
            "revealAnswer": if ac.phase == "reveal" { json!(ac.answer) } else { Value::Null },
            "revealedCorrect": ac.revealed_correct.map(|b| json!(b)).unwrap_or(Value::Null),
            "buzzOpenAt": ac.buzz_open_at.map(|b| json!(b)).unwrap_or(Value::Null),
            "answerDeadline": ac.answer_deadline.map(|b| json!(b)).unwrap_or(Value::Null),
            "tabPenaltyApplied": ac.tab_penalty_for.iter().any(|p| p == my_norm),
            "wrongPlayers": Value::Array(
                ac.wrong_players.iter().map(|p| json!(mitch_lib::admin::mask_email(p))).collect(),
            ),
        });
        if ac.revealed_correct.is_none() {
            if let Some(obj) = o.as_object_mut() {
                obj.remove("revealedCorrect");
            }
        }
        o
    });

    let final_jeopardy = lobby.final_jeopardy.as_ref().map(|fj| {
        let revealed: Map<String, Value> = if fj.phase == "reveal" {
            fj.revealed_answers
                .iter()
                .map(|(e, obj)| {
                    (
                        mitch_lib::admin::mask_email(e),
                        json!({
                            "answer": obj.answer,
                            "correct": obj.correct,
                            "scoreChange": obj.score_change,
                            "wager": obj.wager,
                        }),
                    )
                })
                .collect()
            } else {
                Map::new()
            };
        json!({
            "cat": fj.cat,
            "clue": if fj.phase != "wagering" { json!(fj.clue) } else { Value::Null },
            "phase": fj.phase,
            "wagerDeadline": fj.wager_deadline.map(|b| json!(b)).unwrap_or(Value::Null),
            "answerDeadline": fj.answer_deadline.map(|b| json!(b)).unwrap_or(Value::Null),
            "revealCloseAt": fj.reveal_close_at.map(|b| json!(b)).unwrap_or(Value::Null),
            "wagerSubmitted": fj.wagers.contains_key(my_norm),
            "answerSubmitted": fj.answers.contains_key(my_norm),
            "revealAnswer": if fj.phase == "reveal" { json!(fj.answer) } else { Value::Null },
            "revealedAnswers": if fj.phase == "reveal" { Value::Object(revealed) } else { Value::Null },
        })
    });

    let final_score: Option<Map<String, Value>> = lobby.final_score.as_ref().map(|fs| {
        fs.iter()
            .map(|(e, s)| (mitch_lib::admin::mask_email(e), json!(s)))
            .collect()
    });

    let mut out = json!({
        "myEmail": mitch_lib::admin::mask_email(my_email),
        "id": lobby.id,
        "status": lobby.status,
        "host": mitch_lib::admin::mask_email(&lobby.host),
        "players": Value::Array(lobby.players.iter().map(|p| json!(mitch_lib::admin::mask_email(p))).collect()),
        "scores": Value::Object(masked_scores),
        "categories": lobby.categories.as_ref().map(|c| json!(c)).unwrap_or(Value::Null),
        "board": Value::Object(safe_board),
        "activeClue": active_clue.unwrap_or(Value::Null),
        "finalJeopardy": final_jeopardy.unwrap_or(Value::Null),
        "turn": if lobby.turn.is_empty() { Value::Null } else { json!(mitch_lib::admin::mask_email(&lobby.turn)) },
        "roundOver": lobby.round_over,
        "finalScore": final_score.map(Value::Object).unwrap_or(Value::Null),
        "joinCode": json!(lobby.join_code),
        "clueDbReady": clues_len >= 100,
    });
    // joinCode only reaches the host (the JS leaves it undefined otherwise,
    // and JSON.stringify omits undefined keys).
    if lobby.host != my_norm {
        if let Some(obj) = out.as_object_mut() {
            obj.remove("joinCode");
        }
    }
    // check_timeouts() above can mutate lobby state (phase transitions,
    // final-jeopardy evaluation) purely from the passage of time, with no
    // player action — persist here too, not just on the explicit action
    // endpoints below.
    persist_lobbies(state, &lobbies);
    resp(200, &out)
}

// ── Lazy timeout machine (server.js:21053-21175) ─────────────────────────────

/// `jeopardyCheckTimeouts` — no timers; evaluated inside /state only. The JS
/// captures `phase` once and re-reads live fields inside each block, so the
/// port mirrors that shape (a buzzing→reveal transition inside one call does
/// not fall into the reveal block until the next poll).
fn check_timeouts(state: &AppState, lobby: &mut JeopardyLobby) {
    if lobby.status == "final_jeopardy" && lobby.final_jeopardy.is_some() {
        let now = now_millis();
        if lobby.fj_ref().phase == "wagering" {
            let deadline = lobby.fj_ref().wager_deadline;
            if let Some(d) = deadline {
                if now > d {
                    let players = lobby.players.clone();
                    let fj = lobby.fj_mut();
                    for p in players {
                        fj.wagers.entry(p).or_insert(0);
                    }
                    fj.phase = "answering".to_string();
                    fj.answer_deadline = Some(now + 30000);
                }
            }
        }
        if lobby.fj_ref().phase == "answering" {
            let deadline = lobby.fj_ref().answer_deadline;
            if let Some(d) = deadline {
                if now > d {
                    evaluate_final_jeopardy(lobby);
                }
            }
        }
        if lobby.fj_ref().phase == "reveal" {
            let deadline = lobby.fj_ref().reveal_close_at;
            if let Some(d) = deadline {
                if now > d {
                    end_jeopardy_game(state, lobby);
                }
            }
        }
        return;
    }

    let (phase, cat, val) = {
        let Some(ac) = lobby.active_clue.as_ref() else {
            return;
        };
        (ac.phase.clone(), ac.cat.clone(), ac.val)
    };
    let now = now_millis();

    // 1. Buzzing Timeout — 15s to buzz once the buzzer opens.
    if phase == "buzzing" {
        let deadline = {
            let ac = lobby.active_clue_ref();
            ac.buzz_open_at.map(|b| b + 15000)
        };
        if let Some(d) = deadline {
            if now > d {
                mark_clue_answered(lobby, &cat, val, None);
                let ac = lobby.active_clue_mut();
                ac.phase = "reveal".to_string();
                ac.revealed_correct = Some(false);
                ac.reveal_close_at = Some(now + 5000);
            }
        }
    }

    // 2. Answering Timeout — wrong-by-default with the same score math as
    // /answer, then reveal or reopen the buzzer.
    if phase == "answering" {
        let deadline = lobby.active_clue_ref().answer_deadline;
        if let Some(d) = deadline {
            if now > d {
                let (is_dd, buzzed_by, wager) = {
                    let ac = lobby.active_clue_ref();
                    (ac.is_daily_double, ac.buzzed_by.clone(), ac.wager)
                };
                let expected = if is_dd {
                    Some(lobby.turn.clone())
                } else {
                    buzzed_by
                };
                if let Some(exp) = expected {
                    let wager_v = if wager.is_nan() || wager == 0.0 {
                        val
                    } else {
                        wager
                    };
                    let penalty = if is_dd { -wager_v } else { -val };
                    add_score(lobby, &exp, penalty as i64);
                    let ac = lobby.active_clue_mut();
                    push_wrong(ac, &exp);
                }
                let remaining = {
                    let wrong = &lobby.active_clue_ref().wrong_players;
                    lobby.players.iter().filter(|p| !wrong.contains(p)).count()
                };
                if is_dd || remaining == 0 {
                    mark_clue_answered(lobby, &cat, val, None);
                    let ac = lobby.active_clue_mut();
                    ac.phase = "reveal".to_string();
                    ac.revealed_correct = Some(false);
                    ac.reveal_close_at = Some(now + 5000);
                } else {
                    let ac = lobby.active_clue_mut();
                    ac.phase = "buzzing".to_string();
                    ac.buzzed_by = None;
                    ac.buzzer = None;
                    ac.buzz_open_at = Some(now + 1000);
                    ac.answer_deadline = None;
                }
            }
        }
    }

    // 3. Wagering Timeout — 25s to wager, then the 5-coin min wager.
    if phase == "wagering" {
        let expired = {
            let ac = lobby.active_clue_ref();
            ac.wager_open_at.is_some_and(|w| now > w + 25000)
        };
        if expired {
            let turn_email = lobby.turn.clone();
            let ac = lobby.active_clue_mut();
            ac.wager = 5.0;
            ac.wager_by = Some(turn_email);
            ac.phase = "answering".to_string();
            ac.answer_deadline = Some(now + 20000);
        }
    }

    // 4. Reveal Timeout — board completion rolls into final jeopardy.
    if phase == "reveal" {
        let deadline = lobby.active_clue_ref().reveal_close_at;
        if let Some(d) = deadline {
            if now > d {
                let all_done = lobby.categories.as_ref().is_some_and(|cats| {
                    cats.iter().all(|c2| {
                        lobby
                            .board
                            .as_ref()
                            .and_then(|b| b.get(c2))
                            .is_some_and(|arr| arr.iter().all(|cl| cl.answered))
                    })
                });
                if all_done {
                    let clues = load_clues(state);
                    let cats = lobby.categories.clone().unwrap_or_default();
                    match get_final_jeopardy_clue(&clues, &cats) {
                        Some(fc) => {
                            lobby.status = "final_jeopardy".to_string();
                            lobby.final_jeopardy = Some(FinalJeopardy {
                                cat: fc.category.clone(),
                                clue: fc.clue.clone(),
                                answer: fc.answer.clone(),
                                wagers: IndexMap::new(),
                                answers: IndexMap::new(),
                                phase: "wagering".to_string(),
                                wager_deadline: Some(now + 30000),
                                answer_deadline: None,
                                reveal_close_at: None,
                                revealed_answers: IndexMap::new(),
                            });
                        }
                        None => end_jeopardy_game(state, lobby),
                    }
                }
                lobby.active_clue = None;
            }
        }
    }
}

/// `evaluateFinalJeopardy` (server.js:21022-21041).
fn evaluate_final_jeopardy(lobby: &mut JeopardyLobby) {
    let mut fj = match lobby.final_jeopardy.take() {
        Some(fj) => fj,
        None => return,
    };
    fj.phase = "reveal".to_string();
    fj.reveal_close_at = Some(now_millis() + 10000);
    fj.revealed_answers = IndexMap::new();
    let answer = fj.answer.clone();
    let players = lobby.players.clone();
    for p in players {
        let given = fj.answers.get(&p).cloned().unwrap_or_default();
        let wager = fj.wagers.get(&p).copied().unwrap_or(0);
        let correct = if !given.is_empty() {
            answer_matches(&given, &answer)
        } else {
            false
        };
        let score_change = if correct { wager } else { -wager };
        add_score(lobby, &p, score_change);
        fj.revealed_answers.insert(
            p,
            RevealedAnswer {
                answer: given,
                correct,
                score_change,
                wager,
            },
        );
    }
    lobby.final_jeopardy = Some(fj);
}

/// `endJeopardyGame` (server.js:21042-21051) — 120 base coins with the
/// friend ×1.5 and premium ×2 floors (sequential, floored at each step).
fn end_jeopardy_game(state: &AppState, lobby: &mut JeopardyLobby) {
    lobby.status = "over".to_string();
    let top_score = lobby.scores.values().copied().fold(i64::MIN, i64::max);
    let winners: Vec<String> = lobby
        .scores
        .iter()
        .filter(|(_, s)| **s == top_score)
        .map(|(e, _)| e.clone())
        .collect();
    let player_count = lobby.players.len() as i64;
    for w in &winners {
        let mut coins: i64 = 120;
        let friend_bonus = lobby
            .players
            .iter()
            .any(|p| p != w && are_friends(state, w, p));
        if friend_bonus {
            coins = (coins as f64 * 1.5).floor() as i64;
        }
        let premium = mitch_lib::auth::is_premium_email(&state.store, w);
        if premium {
            coins = (coins as f64 * 2.0).floor() as i64;
        }
        let reason = format!(
            "jeopardy: win payout (players={player_count}, score=${top_score}, friend bonus={friend_bonus}, premium={premium})"
        );
        mitch_lib::coins::add_coins(
            &state.store,
            state.data_dir(),
            w,
            coins as f64,
            state.coin_multiplier(),
            &reason,
        );
        mitch_lib::achievements::update_stat(
            &state.store,
            state.data_dir(),
            w,
            "jeopardy_wins",
            1.0,
            state.coin_multiplier(),
        );
    }
    lobby.final_score = Some(lobby.scores.clone());
}

/// `areFriends` (server.js:1169-1176) — FRIENDS_FILE lists both directions.
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

// ── Clue cache helpers (server.js:1004-1055) ─────────────────────────────────

/// `buildJeopardyBoard` — 6 categories × 5 clues, values 200-1000, 1-2 daily
/// doubles. The JS uses `sort(() => Math.random() - 0.5)` shuffles (unbiased
/// ordering is not required — boards are random per side); the port uses the
/// Fisher-Yates shape from the rest of the codebase.
/// `(categories, board, dailyDouble keys)` — `buildJeopardyBoard`'s return.
type BuiltBoard = (Vec<String>, IndexMap<String, Vec<BoardClue>>, Vec<String>);

fn build_jeopardy_board(clues: &[JeopardyClue]) -> Option<BuiltBoard> {
    if clues.len() < 100 {
        return None;
    }
    // Group by category, first-encounter order (the JS Object.keys order).
    let mut order: Vec<String> = Vec::new();
    let mut by_cat: std::collections::HashMap<String, Vec<&JeopardyClue>> =
        std::collections::HashMap::new();
    for c in clues {
        let entry = by_cat.entry(c.category.clone()).or_insert_with(|| {
            order.push(c.category.clone());
            Vec::new()
        });
        entry.push(c);
    }
    let eligible: Vec<String> = order
        .iter()
        .filter(|k| by_cat[k.as_str()].len() >= 5)
        .cloned()
        .collect();
    if eligible.len() < 6 {
        return None;
    }
    let mut shuffled = eligible;
    fisher_yates(&mut shuffled);
    shuffled.truncate(6);

    const VALUES: [i64; 5] = [200, 400, 600, 800, 1000];
    let dd_count = if mitch_lib::crypto::js_random() < 0.5 {
        1
    } else {
        2
    };
    let mut daily_doubles: Vec<String> = Vec::new();
    while daily_doubles.len() < dd_count {
        let cat = &shuffled[mitch_lib::crypto::js_random_index(6)];
        let val = VALUES[mitch_lib::crypto::js_random_index(5)];
        let key = format!("{cat}|{val}");
        if !daily_doubles.contains(&key) {
            daily_doubles.push(key);
        }
    }

    let mut board: IndexMap<String, Vec<BoardClue>> = IndexMap::new();
    for cat in &shuffled {
        let mut cs: Vec<&JeopardyClue> = by_cat[cat.as_str()].clone();
        fisher_yates(&mut cs);
        let arr = VALUES
            .iter()
            .enumerate()
            .map(|(i, val)| {
                let c = cs[i];
                BoardClue {
                    value: *val,
                    clue: c.clue.clone(),
                    answer: c.answer.clone(),
                    answered: false,
                    daily_double: daily_doubles.contains(&format!("{cat}|{val}")),
                    answered_by: None,
                }
            })
            .collect();
        board.insert(cat.clone(), arr);
    }
    Some((shuffled, board, daily_doubles))
}

fn fisher_yates<T>(v: &mut [T]) {
    for i in (1..v.len()).rev() {
        v.swap(i, mitch_lib::crypto::js_random_index(i + 1));
    }
}

/// `getFinalJeopardyClue` — a random clue outside the board's categories,
/// falling back to the whole cache.
fn get_final_jeopardy_clue<'a>(
    clues: &'a [JeopardyClue],
    board_categories: &[String],
) -> Option<&'a JeopardyClue> {
    if clues.is_empty() {
        return None;
    }
    let filtered: Vec<&JeopardyClue> = clues
        .iter()
        .filter(|c| !board_categories.iter().any(|k| k == &c.category))
        .collect();
    if filtered.is_empty() {
        return Some(&clues[mitch_lib::crypto::js_random_index(clues.len())]);
    }
    Some(filtered[mitch_lib::crypto::js_random_index(filtered.len())])
}

// ── Answer matcher (server.js:1032-1054, Groq fallback removed) ──────────────

/// `normalize` — lowercase, strip ONE leading article (with its whitespace),
/// strip `[^a-z0-9\s]`, collapse whitespace runs to single spaces, trim.
fn jeopardy_normalize(s: &str) -> String {
    let lower = s.to_lowercase();
    // ^(the|a|an)\s+/i — alternation order matters: "an x" must not strip as
    // "a" + "n x" (the regex backtracks to the longer alternative). Only
    // strings that START with the article strip it (the ^ anchor precedes the
    // later trim).
    let mut rest = lower.as_str();
    for art in ["the", "an", "a"] {
        if rest.starts_with(art) && rest[art.len()..].starts_with(char::is_whitespace) {
            rest = &rest[art.len()..];
            rest = rest.trim_start_matches(char::is_whitespace);
            break;
        }
    }
    let mut out = String::with_capacity(rest.len());
    let mut pending_ws = false;
    for ch in rest.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            if pending_ws {
                out.push(' ');
                pending_ws = false;
            }
            out.push(ch);
        } else if ch.is_whitespace() {
            pending_ws = true;
        }
    }
    out
}

/// `jeopardyAnswerMatches` — the local checks only. The Groq fallback is not
/// ported (mitch.pro AI was decommissioned): where the JS would call Groq,
/// this returns false.
pub(crate) fn answer_matches(given: &str, correct: &str) -> bool {
    let lower_given = given.to_lowercase();
    let lower_given = lower_given.trim();
    // Local anti-injection and meta-response filters.
    if lower_given.contains('[')
        || lower_given.contains(']')
        || lower_given.contains('{')
        || lower_given.contains('}')
    {
        return false;
    }
    if lower_given.contains("correct answer")
        || lower_given.contains("right answer")
        || lower_given == "yes"
        || lower_given == "no"
        || lower_given == "true"
        || lower_given == "false"
    {
        return false;
    }

    let g = jeopardy_normalize(given);
    let c = jeopardy_normalize(correct);
    if g == c {
        return true;
    }
    // Allow if one contains the other (for short answers).
    if c.len() > 3 && g.contains(&c) {
        return true;
    }
    if g.len() > 3 && c.contains(&g) {
        return true;
    }
    // Positional mismatch count ≤ 2 for close-length answers (the JS comment
    // says Levenshtein but the code compares character positions).
    if (g.len() as i64 - c.len() as i64).abs() <= 3 {
        let gb = g.as_bytes();
        let cb = c.as_bytes();
        let mut dist = 0usize;
        for i in 0..gb.len().max(cb.len()) {
            if gb.get(i) != cb.get(i) {
                dist += 1;
            }
        }
        if dist <= 2 {
            return true;
        }
    }
    // Where the JS would call Groq: return false.
    false
}

// ── Small JS shims ───────────────────────────────────────────────────────────

/// `qs.get(key)` over the raw query string.
fn qs_get(search: &str, key: &str) -> Option<String> {
    for pair in search.trim_start_matches('?').split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if form_urlencoded_decode(k) == key {
            return Some(form_urlencoded_decode(v));
        }
    }
    None
}

fn form_urlencoded_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 2;
                    }
                    Err(_) => out.push(bytes[i]),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `parseInt(value)` — String coercion, optional sign, 0x hex prefix, then
/// the leading digit run (NaN when there is none).
fn js_parse_int(v: &Value) -> f64 {
    let s = jsval::string(v);
    let t = s.trim_start();
    let (neg, rest) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let (radix, digits) = if rest.len() >= 2 && (rest.starts_with("0x") || rest.starts_with("0X")) {
        (16u32, &rest[2..])
    } else {
        (10u32, rest)
    };
    let cleaned: String = digits.chars().take_while(|c| c.is_digit(radix)).collect();
    if cleaned.is_empty() {
        return f64::NAN;
    }
    let mut acc = 0f64;
    for c in cleaned.chars() {
        match c.to_digit(radix) {
            Some(d) => acc = acc * radix as f64 + d as f64,
            None => break, // unreachable — take_while guarded the run
        }
    }
    if neg {
        -acc
    } else {
        acc
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(given: &str, correct: &str) -> bool {
        answer_matches(given, correct)
    }

    #[test]
    fn matcher_normalize_and_exact() {
        assert!(matches("The CAT!", "cat"));
        assert!(matches("cat", "Cat"));
        assert!(matches("  A   BIG   DOG  ", "big dog"));
    }

    #[test]
    fn matcher_meta_filters() {
        assert!(!matches("the correct answer", "cat"));
        assert!(!matches("RIGHT ANSWER is dog", "dog"));
        assert!(!matches("yes", "cat"));
        assert!(!matches("no", "cat"));
        assert!(!matches("true", "cat"));
        assert!(!matches("false", "cat"));
        assert!(!matches("[answer]", "cat"));
        assert!(!matches("bracket ] here", "cat"));
        assert!(!matches("{injection}", "cat"));
    }

    #[test]
    fn matcher_containment_both_ways() {
        assert!(matches("holy roman empire", "roman empire"));
        assert!(matches("roman empire", "the holy roman empire"));
        // The ≤3 containment guard (correct-side): a 3-char answer never
        // matches by containment even when the given contains it.
        assert!(!matches("catalog xyz", "cat"));
        // And the given-side guard: a 3-char given never matches by
        // containment even when the correct answer starts with it.
        assert!(!matches("abc", "abcxyz"));
    }

    #[test]
    fn matcher_positional_mismatches() {
        assert!(matches("france", "franc"));
        assert!(matches("ab", "abc"));
        assert!(matches("abc", "abd"));
        assert!(!matches("abcd", "azzz")); // 3 positional mismatches
        assert!(!matches("paris", "london"));
        assert!(!matches("cat", "dog"));
        // Length gap > 3 skips the positional check entirely.
        assert!(!matches("the the cat", "cat"));
    }

    #[test]
    fn matcher_leading_article_only() {
        // normalize strips ONE leading article; a leading space means the
        // anchor misses, so the article survives on the given side.
        assert!(!matches(" the cat", "the cat"));
        assert!(matches("the cat", "cat"));
        assert!(matches("an apple", "apple"));
        assert!(matches("a bird", "bird"));
        assert!(!matches("ant", "t")); // no article strip, len gap
    }

    #[test]
    fn matcher_groq_path_returns_false() {
        // These pass in bun via the Groq fallback (decommissioned); the port
        // returns false because no local check fires.
        assert!(!matches("fdr", "franklin delano roosevelt"));
        assert!(!matches("potato", "elephant"));
    }

    #[test]
    fn normalize_shape() {
        assert_eq!(
            jeopardy_normalize("The Quick!!  brown-fox "),
            "quick brownfox"
        );
        assert_eq!(jeopardy_normalize("A  B"), "b");
        assert_eq!(jeopardy_normalize(""), "");
    }

    fn clue(cat: &str, n: usize, val: i64) -> JeopardyClue {
        JeopardyClue {
            category: cat.to_string(),
            clue: format!("clue {n}"),
            answer: format!("answer {n}"),
            value: val,
        }
    }

    #[test]
    fn board_builder_shape() {
        // The builder requires ≥100 cached clues total.
        let mut clues = Vec::new();
        for c in 0..8 {
            for n in 0..20 {
                clues.push(clue(&format!("cat{c}"), n, 200 + n as i64));
            }
        }
        let (cats, board, dds) = build_jeopardy_board(&clues).expect("board builds");
        assert_eq!(cats.len(), 6);
        assert_eq!(board.len(), 6);
        assert!((1..=2).contains(&dds.len()));
        for (cat, arr) in &board {
            assert_eq!(arr.len(), 5);
            for (i, cl) in arr.iter().enumerate() {
                assert_eq!(cl.value, [200, 400, 600, 800, 1000][i]);
                assert!(!cl.answered);
                assert!(cl.answered_by.is_none());
                assert_eq!(
                    cl.daily_double,
                    dds.contains(&format!("{cat}|{}", cl.value))
                );
            }
        }
        for cat in &cats {
            assert!(board.contains_key(cat));
        }
        // Every daily double key names a real board cell.
        for dd in &dds {
            let (cat, val) = dd.split_once('|').unwrap();
            let val: i64 = val.parse().unwrap();
            assert!(board[cat].iter().any(|c| c.value == val));
        }
    }

    #[test]
    fn board_builder_guards() {
        // Fewer than 100 total clues.
        assert!(build_jeopardy_board(&[]).is_none());
        // Fewer than 6 eligible categories.
        let mut clues = Vec::new();
        for c in 0..5 {
            for n in 0..6 {
                clues.push(clue(&format!("cat{c}"), n, n as i64));
            }
        }
        clues.push(clue("cat0", 99, 1));
        assert!(build_jeopardy_board(&clues).is_none());
    }

    #[test]
    fn final_clue_picker() {
        let clues: Vec<JeopardyClue> = (0..3)
            .map(|i| clue(&format!("cat{i}"), i as usize, 100))
            .collect();
        let cats = vec!["cat0".to_string(), "cat1".to_string()];
        let fc = get_final_jeopardy_clue(&clues, &cats).unwrap();
        assert_eq!(fc.category, "cat2");
        // No board categories → anything can be picked.
        let fc = get_final_jeopardy_clue(&clues, &[]).unwrap();
        assert!(clues.iter().any(|c| c.category == fc.category));
        // Empty cache → None (endJeopardyGame runs instead).
        assert!(get_final_jeopardy_clue(&[], &[]).is_none());
        // All categories used → falls back to the whole cache.
        let all: Vec<String> = clues.iter().map(|c| c.category.clone()).collect();
        assert!(get_final_jeopardy_clue(&clues, &all).is_some());
    }

    #[test]
    fn wager_clamping_matches_js() {
        let v = |n: Value| Some(n);
        // Math.max(1000, score) floors the max at 1000.
        assert_eq!(clamp_wager(0.0, v(json!(5000))), 1000.0);
        assert_eq!(clamp_wager(-100.0, v(json!("500"))), 500.0);
        assert_eq!(clamp_wager(2000.0, v(json!(9999))), 2000.0);
        // Math.max(0, floor(n)) floors negatives and fractions.
        assert_eq!(clamp_wager(0.0, v(json!(-3))), 0.0);
        assert_eq!(clamp_wager(0.0, v(json!(1.7))), 1.0);
        // NaN (invalid strings / missing key) → || 0 → 0.
        assert_eq!(clamp_wager(0.0, v(json!("abc"))), 0.0);
        assert_eq!(clamp_wager(0.0, None), 0.0);
    }

    #[test]
    fn js_parse_int_semantics() {
        assert_eq!(js_parse_int(&json!(42.9)), 42.0);
        assert_eq!(js_parse_int(&json!("12abc")), 12.0);
        assert!(js_parse_int(&json!("abc")).is_nan());
        assert!(js_parse_int(&Value::Null).is_nan());
        assert_eq!(js_parse_int(&json!(-7)), -7.0);
        assert_eq!(js_parse_int(&json!("  5")), 5.0);
        assert_eq!(js_parse_int(&json!("0x10")), 16.0);
        assert!(js_parse_int(&json!("")).is_nan());
    }

    #[test]
    fn qs_get_parses_query() {
        assert_eq!(qs_get("id=abc&x=1", "id").as_deref(), Some("abc"));
        assert_eq!(qs_get("?id=a%20b", "id").as_deref(), Some("a b"));
        assert_eq!(qs_get("a=1", "id"), None);
        assert_eq!(qs_get("", "id"), None);
        assert_eq!(qs_get("id", "id").as_deref(), Some(""));
    }

    #[test]
    fn effective_wager_falsy_fallback() {
        let mut ac = ActiveClue {
            cat: "C".into(),
            val: 400.0,
            clue: "c".into(),
            answer: "a".into(),
            is_daily_double: false,
            phase: "answering".into(),
            buzz_open_at: None,
            wager_open_at: None,
            answer_deadline: None,
            buzzed_by: None,
            buzzer: None,
            wager: f64::NAN,
            wager_by: None,
            tab_penalty_for: Vec::new(),
            wrong_players: Vec::new(),
            revealed_correct: None,
            reveal_close_at: None,
        };
        // null / 0 wagers fall back to the clue value (JS `wager || val`).
        assert_eq!(effective_wager(&ac, 400.0), 400.0);
        ac.wager = 0.0;
        assert_eq!(effective_wager(&ac, 400.0), 400.0);
        ac.wager = 1200.0;
        assert_eq!(effective_wager(&ac, 400.0), 1200.0);
    }

    fn temp_data_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mitch-test-jeopardy-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap();
        dir
    }

    /// A lobby mid-daily-double (NaN wager, a board, a buzzed-in player) —
    /// the exact shape that previously vanished on every restart/deploy.
    fn sample_lobby() -> JeopardyLobby {
        let mut scores = IndexMap::new();
        scores.insert("alice@school.edu".to_string(), 600);
        scores.insert("bob@school.edu".to_string(), 200);
        let mut board = IndexMap::new();
        board.insert(
            "History".to_string(),
            vec![BoardClue {
                value: 400,
                clue: "c".into(),
                answer: "a".into(),
                answered: false,
                daily_double: true,
                answered_by: None,
            }],
        );
        JeopardyLobby {
            id: "game1".into(),
            join_code: "ABC".into(),
            host: "alice@school.edu".into(),
            players: vec!["alice@school.edu".into(), "bob@school.edu".into()],
            max_players: 10,
            status: "active".into(),
            scores,
            categories: Some(vec!["History".into()]),
            board: Some(board),
            active_clue: Some(ActiveClue {
                cat: "History".into(),
                val: 400.0,
                clue: "c".into(),
                answer: "a".into(),
                is_daily_double: true,
                phase: "wagering".into(),
                buzz_open_at: None,
                wager_open_at: Some(1000),
                answer_deadline: None,
                buzzed_by: Some("alice@school.edu".into()),
                buzzer: Some(1000),
                wager: f64::NAN,
                wager_by: None,
                tab_penalty_for: Vec::new(),
                wrong_players: Vec::new(),
                revealed_correct: None,
                reveal_close_at: None,
            }),
            turn: "alice@school.edu".into(),
            created_at: 1000,
            tab_hidden: IndexMap::new(),
            round_over: false,
            final_score: None,
            final_jeopardy: None,
        }
    }

    #[test]
    fn persisted_lobby_survives_a_restart_including_nan_wager() {
        let dir = temp_data_dir("roundtrip");
        let store = mitch_lib::data::DataStore::open(&dir, &dir.join("data")).unwrap();
        let data_dir = dir.join("data");

        let lobbies = vec![sample_lobby()];
        let value = serde_json::to_value(&lobbies).unwrap();
        store
            .write_document(&data_dir.join(JEOPARDY_LOBBIES_FILE), &value)
            .unwrap();

        let loaded = load_lobbies(&store, &data_dir);
        assert_eq!(loaded.len(), 1);
        let lobby = &loaded[0];
        assert_eq!(lobby.id, "game1");
        assert_eq!(lobby.scores.get("alice@school.edu"), Some(&600));
        assert_eq!(lobby.players, vec!["alice@school.edu", "bob@school.edu"]);
        let ac = lobby.active_clue.as_ref().unwrap();
        assert!(ac.wager.is_nan(), "NaN wager must round-trip as NaN, not fail or become 0");
        assert_eq!(ac.buzzed_by.as_deref(), Some("alice@school.edu"));
        let board = lobby.board.as_ref().unwrap();
        assert_eq!(board.get("History").unwrap()[0].value, 400);
    }

    #[test]
    fn finished_games_are_not_reloaded() {
        let dir = temp_data_dir("finished");
        let store = mitch_lib::data::DataStore::open(&dir, &dir.join("data")).unwrap();
        let data_dir = dir.join("data");

        let mut finished = sample_lobby();
        finished.status = "over".to_string();
        finished.active_clue = None;
        let value = serde_json::to_value(vec![finished]).unwrap();
        store
            .write_document(&data_dir.join(JEOPARDY_LOBBIES_FILE), &value)
            .unwrap();

        assert!(load_lobbies(&store, &data_dir).is_empty());
    }

    #[test]
    fn missing_lobbies_file_loads_empty_without_panicking() {
        let dir = temp_data_dir("missing");
        let store = mitch_lib::data::DataStore::open(&dir, &dir.join("data")).unwrap();
        assert!(load_lobbies(&store, &dir.join("data")).is_empty());
    }
}
