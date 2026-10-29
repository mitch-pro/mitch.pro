//! `/api/casino/*` — Coinflip, Dice, Slots, Blackjack, and 5-card draw
//! Poker, played for real MitchCoins. See `mitch_lib::casino`'s module doc
//! comment for the money-safety model; this file is just HTTP plumbing
//! around it. Every mutating route here:
//! - requires a valid session (same cookie auth as every other `/api/*`
//!   mutation) and goes through the normal CSRF check (not in
//!   `CSRF_EXEMPT_PATHS`);
//! - rejects outright if `state.casino_enabled()` is false (the admin kill
//!   switch, `/api/admin/casino/toggle` in `admin/economy.rs`);
//! - computes its outcome from `rand::rng()` *before* ever looking at the
//!   bet amount's relationship to the account balance, then calls
//!   `mitch_lib::coins::settle_bet` exactly once to move money.
//!
//! No route here is `async` — none of them need reCAPTCHA or any other
//! await, which matters for Blackjack/Poker: their handlers hold the
//! `state.blackjack_games` / `state.poker_games` `std::sync::Mutex` for the
//! entire operation (lookup, play, settle, remove-if-resolved) with no
//! `.await` in between, so two requests for the same hand (e.g. a retried
//! "stand") can never both observe it as still in progress.

use super::me::{cookies_of, json_response, parse_body_strict};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::auth;
use mitch_lib::casino::{self, BlackjackOutcome, Card, PokerHandRank};
use mitch_lib::coins::{self, BetError};
use serde_json::{json, Value};
use std::sync::Arc;

pub fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Option<Response> {
    if !path.starts_with("/api/casino/") {
        return None;
    }
    let body = match parse_body_strict(body_bytes) {
        Some(b) => b,
        None => return Some(json_response(400, json!({ "error": "bad json" }))),
    };
    match (method.as_str(), path) {
        ("GET", "/api/casino/status") => Some(status(state)),
        ("GET", "/api/casino/blackjack/state") => Some(blackjack_state(state, headers)),
        ("GET", "/api/casino/poker/state") => Some(poker_state(state, headers)),
        ("POST", "/api/casino/coinflip") => Some(coinflip(state, headers, &body)),
        ("POST", "/api/casino/dice") => Some(dice(state, headers, &body)),
        ("POST", "/api/casino/slots") => Some(slots(state, headers, &body)),
        ("POST", "/api/casino/blackjack/start") => Some(blackjack_start(state, headers, &body)),
        ("POST", "/api/casino/blackjack/hit") => Some(blackjack_hit(state, headers)),
        ("POST", "/api/casino/blackjack/stand") => Some(blackjack_stand(state, headers)),
        ("POST", "/api/casino/poker/start") => Some(poker_start(state, headers, &body)),
        ("POST", "/api/casino/poker/draw") => Some(poker_draw(state, headers, &body)),
        _ => None,
    }
}

fn disabled() -> Response {
    json_response(403, json!({ "error": "casino_disabled" }))
}

fn unauthorized() -> Response {
    json_response(401, json!({ "error": "unauthorized" }))
}

/// Shared auth: valid session cookie -> email. `None` means the caller
/// should respond `unauthorized()`.
fn authed_email(state: &Arc<AppState>, headers: &HeaderMap) -> Option<String> {
    let cookies = cookies_of(state, headers);
    let sid = cookies.auth_sid();
    if !auth::valid_id(&sid, &state.id_secret) {
        return None;
    }
    auth::email_from_sid(&state.store, &state.id_secret, &sid)
}

fn bet_from_body(body: &Value) -> Option<f64> {
    let bet = body.get("bet").and_then(|v| v.as_f64())?;
    if casino::bet_in_range(bet) {
        Some(bet)
    } else {
        None
    }
}

fn invalid_bet() -> Response {
    json_response(
        400,
        json!({
            "error": "invalid_bet",
            "minBet": casino::MIN_BET,
            "maxBet": casino::MAX_BET,
        }),
    )
}

fn card_json(c: &Card) -> Value {
    json!({ "rank": c.rank_label(), "suit": c.suit_label() })
}

fn cards_json(cards: &[Card]) -> Value {
    json!(cards.iter().map(card_json).collect::<Vec<_>>())
}

fn blackjack_outcome_label(outcome: BlackjackOutcome) -> &'static str {
    match outcome {
        BlackjackOutcome::PlayerBlackjack => "player_blackjack",
        BlackjackOutcome::PlayerBust => "player_bust",
        BlackjackOutcome::DealerBust => "dealer_bust",
        BlackjackOutcome::PlayerWins => "player_wins",
        BlackjackOutcome::DealerWins => "dealer_wins",
        BlackjackOutcome::Push => "push",
    }
}

fn poker_rank_label(rank: PokerHandRank) -> &'static str {
    match rank {
        PokerHandRank::RoyalFlush => "royal_flush",
        PokerHandRank::StraightFlush => "straight_flush",
        PokerHandRank::FourOfAKind => "four_of_a_kind",
        PokerHandRank::FullHouse => "full_house",
        PokerHandRank::Flush => "flush",
        PokerHandRank::Straight => "straight",
        PokerHandRank::ThreeOfAKind => "three_of_a_kind",
        PokerHandRank::TwoPair => "two_pair",
        PokerHandRank::JacksOrBetter => "jacks_or_better",
        PokerHandRank::HighCard => "high_card",
    }
}

/// Settles one round and builds the common `{won, payout, balance}` tail
/// of a response — the one place every game's route calls
/// `coins::settle_bet`. Returns `Err(Response)` (already a final error
/// response) if the account couldn't cover the bet.
#[allow(clippy::result_large_err)] // the Err is just this handler's own early-return Response
fn settle(
    state: &Arc<AppState>,
    email: &str,
    bet: f64,
    payout: f64,
    reason: &str,
) -> Result<(bool, f64, f64), Response> {
    match coins::settle_bet(&state.store, state.data_dir(), email, bet, payout, reason) {
        Ok((_before, after)) => Ok((payout > 0.0, payout, after)),
        Err(BetError::InsufficientFunds) => {
            Err(json_response(400, json!({ "error": "insufficient_funds" })))
        }
        Err(BetError::InvalidAmount) => Err(invalid_bet()),
    }
}

#[allow(clippy::result_large_err)]
fn settle_escrowed(
    state: &Arc<AppState>,
    email: &str,
    bet: f64,
    payout: f64,
    reason: &str,
) -> Result<(bool, f64, f64), Response> {
    match coins::settle_escrowed_bet(&state.store, state.data_dir(), email, bet, payout, reason) {
        Ok(after) => Ok((payout > 0.0, payout, after)),
        Err(BetError::InsufficientFunds) => {
            Err(json_response(400, json!({ "error": "insufficient_funds" })))
        }
        Err(BetError::InvalidAmount) => Err(invalid_bet()),
    }
}

fn status(state: &Arc<AppState>) -> Response {
    json_response(
        200,
        json!({
            "enabled": state.casino_enabled(),
            "minBet": casino::MIN_BET,
            "maxBet": casino::MAX_BET,
        }),
    )
}

// ---------------------------------------------------------------------
// Coinflip
// ---------------------------------------------------------------------

fn coinflip(state: &Arc<AppState>, headers: &HeaderMap, body: &Value) -> Response {
    if !state.casino_enabled() {
        return disabled();
    }
    let Some(email) = authed_email(state, headers) else {
        return unauthorized();
    };
    let Some(bet) = bet_from_body(body) else {
        return invalid_bet();
    };
    let choice = body.get("choice").and_then(|v| v.as_str()).unwrap_or("");
    let (won, payout) = casino::play_coinflip(bet, choice);
    let reason = format!("casino:coinflip:{}", if won { "win" } else { "loss" });
    match settle(state, &email, bet, payout, &reason) {
        Ok((won, payout, balance)) => json_response(
            200,
            json!({ "won": won, "payout": payout, "balance": balance }),
        ),
        Err(resp) => resp,
    }
}

// ---------------------------------------------------------------------
// Dice
// ---------------------------------------------------------------------

fn dice(state: &Arc<AppState>, headers: &HeaderMap, body: &Value) -> Response {
    if !state.casino_enabled() {
        return disabled();
    }
    let Some(email) = authed_email(state, headers) else {
        return unauthorized();
    };
    let Some(bet) = bet_from_body(body) else {
        return invalid_bet();
    };
    let target = body
        .get("target")
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
        .clamp(
            casino::DICE_MIN_TARGET as u64,
            casino::DICE_MAX_TARGET as u64,
        ) as u8;
    let direction = body
        .get("direction")
        .and_then(|v| v.as_str())
        .unwrap_or("under");
    let (won, payout, roll) = casino::play_dice(bet, target, direction);
    let reason = format!("casino:dice:{}", if won { "win" } else { "loss" });
    match settle(state, &email, bet, payout, &reason) {
        Ok((won, payout, balance)) => json_response(
            200,
            json!({ "won": won, "payout": payout, "roll": roll, "balance": balance }),
        ),
        Err(resp) => resp,
    }
}

// ---------------------------------------------------------------------
// Slots
// ---------------------------------------------------------------------

fn slot_symbol_label(s: casino::SlotSymbol) -> &'static str {
    match s {
        casino::SlotSymbol::Cherry => "cherry",
        casino::SlotSymbol::Lemon => "lemon",
        casino::SlotSymbol::Bell => "bell",
        casino::SlotSymbol::Clover => "clover",
        casino::SlotSymbol::Star => "star",
        casino::SlotSymbol::Seven => "seven",
    }
}

fn slots(state: &Arc<AppState>, headers: &HeaderMap, body: &Value) -> Response {
    if !state.casino_enabled() {
        return disabled();
    }
    let Some(email) = authed_email(state, headers) else {
        return unauthorized();
    };
    let Some(bet) = bet_from_body(body) else {
        return invalid_bet();
    };
    let (reels, payout) = casino::play_slots(bet);
    let reason = format!("casino:slots:{}", if payout > 0.0 { "win" } else { "loss" });
    match settle(state, &email, bet, payout, &reason) {
        Ok((won, payout, balance)) => json_response(
            200,
            json!({
                "won": won,
                "payout": payout,
                "reels": reels.iter().map(|s| slot_symbol_label(*s)).collect::<Vec<_>>(),
                "balance": balance,
            }),
        ),
        Err(resp) => resp,
    }
}

// ---------------------------------------------------------------------
// Blackjack
// ---------------------------------------------------------------------

/// `GET /api/casino/blackjack/state` — lets the UI recover an in-progress
/// hand after a reload or a dropped connection, since there's otherwise no
/// way back in: the bet is already placed, and `blackjack/start` refuses
/// a second hand while one is active.
fn blackjack_state(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let Some(email) = authed_email(state, headers) else {
        return unauthorized();
    };
    let norm = auth::normalize_email(&email);
    let games = state
        .blackjack_games
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    match games.get(&norm) {
        Some(game) => json_response(
            200,
            json!({
                "inProgress": true,
                "player": cards_json(&game.player),
                "dealerUpCard": card_json(&game.dealer[0]),
                "value": casino::blackjack_hand_value(&game.player).0,
                "bet": game.bet,
            }),
        ),
        None => json_response(200, json!({ "inProgress": false })),
    }
}

/// `GET /api/casino/poker/state` — same recovery role as
/// `blackjack_state`, for a dealt-but-not-yet-drawn poker hand.
fn poker_state(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let Some(email) = authed_email(state, headers) else {
        return unauthorized();
    };
    let norm = auth::normalize_email(&email);
    let games = state.poker_games.lock().unwrap_or_else(|e| e.into_inner());
    match games.get(&norm) {
        Some(game) => json_response(
            200,
            json!({ "inProgress": true, "hand": cards_json(&game.hand), "bet": game.bet }),
        ),
        None => json_response(200, json!({ "inProgress": false })),
    }
}

fn blackjack_start(state: &Arc<AppState>, headers: &HeaderMap, body: &Value) -> Response {
    if !state.casino_enabled() {
        return disabled();
    }
    let Some(email) = authed_email(state, headers) else {
        return unauthorized();
    };
    let Some(bet) = bet_from_body(body) else {
        return invalid_bet();
    };
    let norm = auth::normalize_email(&email);

    let mut games = state
        .blackjack_games
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if games.contains_key(&norm) {
        return json_response(409, json!({ "error": "game_in_progress" }));
    }

    let (game, resolved) = casino::blackjack_start(bet);
    let player = game.player.clone();
    let dealer = game.dealer.clone();
    match resolved {
        None => {
            let balance = match coins::escrow_bet(
                &state.store,
                state.data_dir(),
                &email,
                bet,
                "casino:blackjack:bet",
            ) {
                Ok(b) => b,
                Err(BetError::InsufficientFunds) => {
                    return json_response(400, json!({ "error": "insufficient_funds" }));
                }
                Err(BetError::InvalidAmount) => return invalid_bet(),
            };
            games.insert(norm, game);
            drop(games);
            json_response(
                200,
                json!({
                    "resolved": false,
                    "player": cards_json(&player),
                    "dealerUpCard": card_json(&dealer[0]),
                    "value": casino::blackjack_hand_value(&player).0,
                    "bet": bet,
                    "balance": balance,
                }),
            )
        }
        Some((outcome, payout)) => {
            drop(games);
            let reason = format!("casino:blackjack:{}", blackjack_outcome_label(outcome));
            match settle(state, &email, bet, payout, &reason) {
                Ok((_won, payout, balance)) => json_response(
                    200,
                    json!({
                        "resolved": true,
                        "result": blackjack_outcome_label(outcome),
                        "player": cards_json(&player),
                        "dealer": cards_json(&dealer),
                        "payout": payout,
                        "balance": balance,
                    }),
                ),
                Err(resp) => resp,
            }
        }
    }
}

fn blackjack_hit(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    if !state.casino_enabled() {
        return disabled();
    }
    let Some(email) = authed_email(state, headers) else {
        return unauthorized();
    };
    let norm = auth::normalize_email(&email);

    let mut games = state
        .blackjack_games
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(game) = games.get_mut(&norm) else {
        return json_response(404, json!({ "error": "no_active_game" }));
    };
    let resolved = casino::blackjack_hit(game);
    let player = game.player.clone();
    let bet = game.bet;
    match resolved {
        None => {
            drop(games);
            json_response(
                200,
                json!({
                    "resolved": false,
                    "player": cards_json(&player),
                    "value": casino::blackjack_hand_value(&player).0,
                }),
            )
        }
        Some((outcome, payout)) => {
            let dealer = game.dealer.clone();
            games.remove(&norm);
            drop(games);
            let reason = format!("casino:blackjack:{}", blackjack_outcome_label(outcome));
            match settle_escrowed(state, &email, bet, payout, &reason) {
                Ok((_won, payout, balance)) => json_response(
                    200,
                    json!({
                        "resolved": true,
                        "result": blackjack_outcome_label(outcome),
                        "player": cards_json(&player),
                        "dealer": cards_json(&dealer),
                        "payout": payout,
                        "balance": balance,
                    }),
                ),
                Err(resp) => resp,
            }
        }
    }
}

fn blackjack_stand(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    if !state.casino_enabled() {
        return disabled();
    }
    let Some(email) = authed_email(state, headers) else {
        return unauthorized();
    };
    let norm = auth::normalize_email(&email);

    let mut games = state
        .blackjack_games
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(game) = games.get_mut(&norm) else {
        return json_response(404, json!({ "error": "no_active_game" }));
    };
    let (outcome, payout) = casino::blackjack_stand(game);
    let player = game.player.clone();
    let dealer = game.dealer.clone();
    let bet = game.bet;
    games.remove(&norm);
    drop(games);

    let reason = format!("casino:blackjack:{}", blackjack_outcome_label(outcome));
    match settle_escrowed(state, &email, bet, payout, &reason) {
        Ok((_won, payout, balance)) => json_response(
            200,
            json!({
                "resolved": true,
                "result": blackjack_outcome_label(outcome),
                "player": cards_json(&player),
                "dealer": cards_json(&dealer),
                "payout": payout,
                "balance": balance,
            }),
        ),
        Err(resp) => resp,
    }
}

// ---------------------------------------------------------------------
// Poker
// ---------------------------------------------------------------------

fn poker_start(state: &Arc<AppState>, headers: &HeaderMap, body: &Value) -> Response {
    if !state.casino_enabled() {
        return disabled();
    }
    let Some(email) = authed_email(state, headers) else {
        return unauthorized();
    };
    let Some(bet) = bet_from_body(body) else {
        return invalid_bet();
    };
    let norm = auth::normalize_email(&email);

    let mut games = state.poker_games.lock().unwrap_or_else(|e| e.into_inner());
    if games.contains_key(&norm) {
        return json_response(409, json!({ "error": "game_in_progress" }));
    }

    let balance = match coins::escrow_bet(
        &state.store,
        state.data_dir(),
        &email,
        bet,
        "casino:poker:bet",
    ) {
        Ok(b) => b,
        Err(BetError::InsufficientFunds) => {
            return json_response(400, json!({ "error": "insufficient_funds" }));
        }
        Err(BetError::InvalidAmount) => return invalid_bet(),
    };

    let game = casino::poker_deal(bet);
    let hand = game.hand;
    games.insert(norm, game);
    json_response(
        200,
        json!({ "hand": cards_json(&hand), "bet": bet, "balance": balance }),
    )
}

fn poker_draw(state: &Arc<AppState>, headers: &HeaderMap, body: &Value) -> Response {
    if !state.casino_enabled() {
        return disabled();
    }
    let Some(email) = authed_email(state, headers) else {
        return unauthorized();
    };
    let norm = auth::normalize_email(&email);

    let mut games = state.poker_games.lock().unwrap_or_else(|e| e.into_inner());
    let Some(game) = games.get_mut(&norm) else {
        return json_response(404, json!({ "error": "no_active_game" }));
    };
    let hold = body
        .get("hold")
        .and_then(|v| v.as_array())
        .map(|arr| {
            let mut h = [false; 5];
            for (i, slot) in h.iter_mut().enumerate() {
                *slot = arr.get(i).and_then(|v| v.as_bool()).unwrap_or(false);
            }
            h
        })
        .unwrap_or([true; 5]);
    let (rank, payout) = casino::poker_draw(game, hold);
    let hand = game.hand;
    let bet = game.bet;
    games.remove(&norm);
    drop(games);

    let reason = format!("casino:poker:{}", poker_rank_label(rank));
    match settle_escrowed(state, &email, bet, payout, &reason) {
        Ok((_won, payout, balance)) => json_response(
            200,
            json!({
                "hand": cards_json(&hand),
                "rank": poker_rank_label(rank),
                "payout": payout,
                "balance": balance,
            }),
        ),
        Err(resp) => resp,
    }
}
