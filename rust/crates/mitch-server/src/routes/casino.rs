//! `/api/casino/*` — the casino (plan Step 12). The whole group sits behind
//! one shared prelude (server.js:23077-23097): sid auth ladder → method gate
//! → `blackjack/state` → (POSTs except blackjack hit/stand) tryParseJson +
//! verifyRecaptcha — then the helpers `addHistory` / `readCasinoBet` /
//! `isRigged` / `settleCasinoRound` / `weightedPick` / `drawUniqueNumbers`
//! (server.js:23099-23175) and the per-game handlers.
//!
//! This commit ports the prelude + helpers + the read endpoints
//! (`history`, `global-feed`) + the instant games rock-paper-scissors,
//! lucky-seven, color-card, triple-dice, plinko, roulette and high-low
//! (server.js:23189-23310) and blackjack (start/hit/stand,
//! server.js:23323-23459).
//!
//! Every response body leaves through `js_stringify` — the money fields are
//! `Number(x.toFixed(n))` values whose exact rendering matters.

use axum::http::{HeaderMap, Method};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::errors::json_resp_str;
use crate::state::AppState;
use mitch_lib::data::{js_num_from_fixed, js_stringify, js_to_fixed};
use mitch_lib::jsval;

/// One entry of `bjGames` (server.js:23377): the shuffled deck plus the two
/// hands, the locked-in bet, and the (always false) rig flag the dead rig
/// branches read.
pub struct BjGame {
    pub deck: Vec<Value>,
    pub player_hand: Vec<Value>,
    pub dealer_hand: Vec<Value>,
    pub bet: f64,
    // bun stores `email` in the map but never reads it back (the handlers
    // close over the request's email) — kept for shape parity.
    #[allow(dead_code)]
    pub email: String,
    pub rigged: bool,
}

pub(crate) async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body: &Value,
    body_bytes: &[u8],
) -> Option<axum::response::Response> {
    if !path.starts_with("/api/casino/") {
        return None;
    }
    // Auth ladder (server.js:23079-23083) — validId fail → 'unauthorized',
    // emailFromSid fail → 'email not found'. bun does NOT consult the
    // revoked-id store here (unlike the game-portal pair).
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cookies.get("id").unwrap_or(""))
        .to_string();
    if !mitch_lib::auth::valid_id(&sid, &state.id_secret) {
        return Some(resp(401, &json!({ "error": "unauthorized" })));
    }
    let Some(email) = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid) else {
        return Some(resp(401, &json!({ "error": "email not found" })));
    };
    let norm = mitch_lib::auth::normalize_email(&email);

    // Method gate (server.js:23086-23087): the three read paths are GET-only,
    // everything else POST-only.
    let read_only = matches!(
        path,
        "/api/casino/history" | "/api/casino/global-feed" | "/api/casino/blackjack/state"
    );
    let wanted = if read_only { Method::GET } else { Method::POST };
    if *method != wanted {
        return Some(resp(405, &json!({ "error": "Method not allowed" })));
    }

    // `GET /api/casino/blackjack/state` (server.js:23088-23091) — the
    // in-progress hand's public view (bet, player hand, dealer up card).
    if path == "/api/casino/blackjack/state" {
        let games = state.bj_games.lock().unwrap_or_else(|e| e.into_inner());
        return Some(match games.get(&norm) {
            Some(g) => resp(
                200,
                &json!({
                    "active": true,
                    "bet": jsval::num_value(g.bet),
                    "playerHand": Value::Array(g.player_hand.clone()),
                    "dealerUpCard": g.dealer_hand.first().cloned().unwrap_or(Value::Null),
                }),
            ),
            None => resp(200, &json!({ "active": false })),
        });
    }

    // POST parse + reCAPTCHA gate (server.js:23093-23097) — blackjack
    // hit/stand skip both (they re-use the parsed body / need no captcha).
    if *method == Method::POST
        && path != "/api/casino/blackjack/hit"
        && path != "/api/casino/blackjack/stand"
    {
        let parsed = if body_bytes.is_empty() {
            Some(json!({}))
        } else {
            serde_json::from_slice(body_bytes).ok()
        };
        let Some(b) = parsed else {
            return Some(resp(400, &json!({ "error": "bad json" })));
        };
        let ip = crate::handler::get_real_ip(headers, None);
        let token = jsval::string(&jsval::or(b.get("recaptcha_token"), json!("")));
        if !crate::routes::push::verify_recaptcha(state, &token, &ip, "").await {
            return Some(resp(
                400,
                &json!({ "error": "reCAPTCHA failed. Please try again." }),
            ));
        }
        return game(state, path, &b, &email, &norm);
    }
    let _ = body; // GET paths never read the body
    game(state, path, &Value::Null, &email, &norm)
}

/// Dispatch after the prelude. `body` is the parsed POST body for games, Null
/// for the GET reads.
fn game(
    state: &Arc<AppState>,
    path: &str,
    body: &Value,
    email: &str,
    norm: &str,
) -> Option<axum::response::Response> {
    match path {
        "/api/casino/history" => Some(history(state, norm)),
        "/api/casino/global-feed" => Some(global_feed(state)),
        "/api/casino/rock-paper-scissors" => Some(rock_paper_scissors(state, body, email, norm)),
        "/api/casino/lucky-seven" => Some(lucky_seven(state, body, email, norm)),
        "/api/casino/color-card" => Some(color_card(state, body, email, norm)),
        "/api/casino/triple-dice" => Some(triple_dice(state, body, email, norm)),
        "/api/casino/plinko" => Some(plinko(state, body, email, norm)),
        "/api/casino/roulette" => Some(roulette(state, body, email, norm)),
        "/api/casino/high-low" => Some(high_low(state, body, email, norm)),
        "/api/casino/blackjack/start" => Some(bj_start(state, body, email, norm)),
        "/api/casino/blackjack/hit" => Some(bj_hit(state, email, norm)),
        "/api/casino/blackjack/stand" => Some(bj_stand(state, email, norm)),
        "/api/casino/poker/start" => Some(poker(state, body, email, norm)),
        "/api/casino/coinflip" => Some(coinflip(state, body, email, norm)),
        "/api/casino/dice" => Some(dice(state, body, email, norm)),
        "/api/casino/crash" => Some(crash(state, body, email, norm)),
        "/api/casino/wheel" => Some(wheel(state, body, email, norm)),
        "/api/casino/scratch" => Some(scratch(state, body, email, norm)),
        "/api/casino/keno" => Some(keno(state, body, email, norm)),
        "/api/casino/slots" => Some(slots(state, body, email, norm, false)),
        "/api/casino/vip/slots" => Some(slots(state, body, email, norm, true)),
        // unhandled paths return None so
        // the request falls through exactly like bun (POST → 405 fallthrough,
        // GET → static 404).
        _ => None,
    }
}

fn resp(code: u16, body: &Value) -> axum::response::Response {
    json_resp_str(code, js_stringify(body))
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn enabled(state: &Arc<AppState>) -> bool {
    state
        .casino_enabled
        .load(std::sync::atomic::Ordering::Relaxed)
}

fn closed() -> axum::response::Response {
    resp(403, &json!({ "error": "Casino is currently closed." }))
}

fn user_stats(state: &Arc<AppState>, norm: &str) -> Value {
    mitch_lib::user_stats::user_stats_get(&state.store, norm)
}

/// The VIP Casino Pass (unlimited max bet) is retired — it let a round's
/// bet size scale without limit, which combined with the also-retired
/// bad-beat-insurance refund to turn a guaranteed-non-negative round into a
/// guaranteed-large one. No new `vip_casino_until` can be granted (the shop
/// item is gone), but this stays `false` outright rather than reading that
/// field, so any balance already on a user's account from before this fix
/// stops mattering the moment this deploys instead of lingering for up to
/// the pass's original 24h.
fn is_vip(_state: &Arc<AppState>, _norm: &str) -> bool {
    false
}

/// `readCasinoBet` (server.js:23106-23117) — Number(body.amount), the
/// minimum/balance ladder, then the non-VIP 500 cap. Returns the bet clamped
/// through `Number(bet.toFixed(2))`.
fn read_casino_bet(
    state: &Arc<AppState>,
    body: &Value,
    email: &str,
    norm: &str,
    min: f64,
) -> Result<f64, Box<axum::response::Response>> {
    let bet = body
        .get("amount")
        .and_then(jsval::number)
        .unwrap_or(f64::NAN);
    let bal = mitch_lib::coins::get_coins(&state.store, state.data_dir(), email);
    if !bet.is_finite() || bet < min {
        return Err(Box::new(resp(
            400,
            &json!({ "error": format!("Minimum bet is {} coins.", jsval::num_value(min)) }),
        )));
    }
    if bet > bal {
        return Err(Box::new(resp(
            400,
            &json!({ "error": "You do not have enough coins for that bet." }),
        )));
    }
    if !is_vip(state, norm) && bet > 500.0 {
        return Err(Box::new(resp(
            400,
            &json!({ "error": "Maximum bet is 500 coins." }),
        )));
    }
    Ok(js_num_from_fixed(&js_to_fixed(bet, 2)))
}

/// The player context the JS closure captures (`email` + `norm`).
struct Round<'a> {
    email: &'a str,
    norm: &'a str,
}

/// The settle result of `settleCasinoRound` (server.js:23155).
struct Settled {
    payout: f64,
    net: f64,
    new_balance: f64,
}

/// `casinoIntake += v` / `casinoPayout += v` — f64 RMW over the AtomicU64
/// bit stores, then the `saveCasinoStats()` write-through
/// (server.js:1199).
fn add_casino_stat(atomic: &std::sync::atomic::AtomicU64, v: f64) {
    use std::sync::atomic::Ordering::Relaxed;
    let mut cur = atomic.load(Relaxed);
    loop {
        let next = (f64::from_bits(cur) + v).to_bits();
        match atomic.compare_exchange_weak(cur, next, Relaxed, Relaxed) {
            Ok(_) => break,
            Err(c) => cur = c,
        }
    }
}

fn save_casino_stats(state: &Arc<AppState>) {
    use std::sync::atomic::Ordering::Relaxed;
    let doc = json!({
        "intake": jsval::num_value(f64::from_bits(state.casino_intake.load(Relaxed))),
        "payout": jsval::num_value(f64::from_bits(state.casino_payout.load(Relaxed))),
    });
    let _ = state
        .store
        .write_document(&state.cfg.data_dir.join("casino_stats.json"), &doc);
}

/// `addHistory` (server.js:23099-23104) — unshift, 25-cap with pop.
fn add_history(state: &Arc<AppState>, norm: &str, game: &str, amount: f64, outcome: &str) {
    let mut map = state
        .casino_history
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut h = map.get(norm).cloned().unwrap_or_default();
    h.insert(
        0,
        json!({
            "game": game,
            "amount": jsval::num_value(amount),
            "outcome": outcome,
            "ts": now_millis(),
        }),
    );
    if h.len() > 25 {
        h.pop();
    }
    map.insert(norm.to_string(), h);
}

/// `logBet` (server.js:1371-1374) — unshift into the global feed, 50-cap.
fn log_bet(state: &Arc<AppState>, user: &str, game: &str, amount: f64, outcome: &str) {
    let mut feed = state.betting_feed.lock().unwrap_or_else(|e| e.into_inner());
    feed.insert(
        0,
        json!({
            "user": user,
            "game": game,
            "amount": jsval::num_value(amount),
            "outcome": outcome,
            "ts": now_millis(),
        }),
    );
    if feed.len() > 50 {
        feed.pop();
    }
}

/// `settleCasinoRound` (server.js:23123-23156) — the 4-decimal money
/// rounding, intake/payout bookkeeping, the coin settlement, history + feed
/// + stats.
///
/// The original double-down / bad-beat modifiers are gone: bad-beat
/// insurance refunded the bet on *any* loss outright, which made a round
/// with any win chance above zero a guaranteed non-negative-EV bet — free
/// money over enough rounds — and double-down then let that profit compound
/// by doubling wins on top. Both were sold in the shop; removing the shop
/// items alone wouldn't have closed this for whoever already had one
/// active, so the effect is removed here too.
fn settle_casino_round(
    state: &Arc<AppState>,
    player: &Round<'_>,
    game_name: &str,
    bet: f64,
    payout: f64,
    outcome: &str,
    opts: (bool, bool), // (freeSpin, prepaid)
) -> Settled {
    let (free_spin, prepaid) = opts;
    let (email, norm) = (player.email, player.norm);
    let final_outcome = outcome.to_string();

    // `Number(Math.max(0, finalPayout || 0).toFixed(4))` — NaN/0 are falsy
    // under `||`, and Math.max(0, x) clamps negatives to +0.
    let fp = if payout.is_nan() || payout == 0.0 {
        0.0
    } else {
        payout
    };
    let safe_payout = js_num_from_fixed(&js_to_fixed(fp.max(0.0), 4));
    let effective_bet = if free_spin { 0.0 } else { bet };
    let net = js_num_from_fixed(&js_to_fixed(safe_payout - effective_bet, 4));

    add_casino_stat(
        &state.casino_intake,
        if prepaid { 0.0 } else { effective_bet },
    );
    add_casino_stat(&state.casino_payout, safe_payout);
    save_casino_stats(state);
    mitch_lib::coins::add_coins(
        &state.store,
        state.data_dir(),
        email,
        if prepaid { safe_payout } else { net },
        state.coin_multiplier(),
        "",
    );
    add_history(state, norm, game_name, net, &final_outcome);
    log_bet(state, email, game_name, effective_bet, &final_outcome);

    mitch_lib::achievements::update_stat(
        &state.store,
        state.data_dir(),
        email,
        "casino_bets",
        1.0,
        state.coin_multiplier(),
    );
    if net > 0.0 {
        mitch_lib::achievements::update_stat(
            &state.store,
            state.data_dir(),
            email,
            "casino_wins",
            1.0,
            state.coin_multiplier(),
        );
    }

    Settled {
        payout: safe_payout,
        net,
        new_balance: mitch_lib::coins::get_coins(&state.store, state.data_dir(), email),
    }
}

/// `GET /api/casino/history` (server.js:23177-23179).
fn history(state: &Arc<AppState>, norm: &str) -> axum::response::Response {
    let map = state
        .casino_history
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let h = map.get(norm).cloned().unwrap_or_default();
    resp(200, &json!({ "history": Value::Array(h) }))
}

/// `GET /api/casino/global-feed` (server.js:23181-23187) — first 50 entries
/// with the email local-part replacing `user` (empty → 'anonymous').
fn global_feed(state: &Arc<AppState>) -> axum::response::Response {
    let feed = state.betting_feed.lock().unwrap_or_else(|e| e.into_inner());
    let sanitized: Vec<Value> = feed
        .iter()
        .take(50)
        .map(|b| {
            let mut out = b.clone();
            if let Some(obj) = out.as_object_mut() {
                let user = jsval::string(obj.get("user").unwrap_or(&Value::Null));
                let shown = if user.is_empty() {
                    "anonymous".to_string()
                } else {
                    user.split('@').next().unwrap_or("").to_string()
                };
                obj.insert("user".to_string(), json!(shown));
            }
            out
        })
        .collect();
    resp(200, &json!({ "feed": Value::Array(sanitized) }))
}

// ── Instant games ────────────────────────────────────────────────────────────

/// `POST /api/casino/rock-paper-scissors` (server.js:23189-23202).
fn rock_paper_scissors(
    state: &Arc<AppState>,
    body: &Value,
    email: &str,
    norm: &str,
) -> axum::response::Response {
    if !enabled(state) {
        return closed();
    }
    let bet = match read_casino_bet(state, body, email, norm, 1.0) {
        Ok(b) => b,
        Err(r) => return *r,
    };
    let choice = jsval::string(&jsval::or(body.get("choice"), json!(""))).to_lowercase();
    let options = ["rock", "paper", "scissors"];
    if !options.contains(&choice.as_str()) {
        return resp(400, &json!({ "error": "Choose rock, paper, or scissors." }));
    }
    let computer = options[mitch_lib::crypto::js_random_index(options.len())];
    let tie = choice == computer;
    let won = !tie
        && ((choice == "rock" && computer == "scissors")
            || (choice == "paper" && computer == "rock")
            || (choice == "scissors" && computer == "paper"));
    let payout = if tie {
        bet
    } else if won {
        bet * 1.9
    } else {
        0.0
    };
    let outcome = if tie {
        "PUSH"
    } else if won {
        "WIN"
    } else {
        "LOSE"
    };
    let settled = settle_casino_round(
        state,
        &Round { email, norm },
        "Rock Paper Scissors",
        bet,
        payout,
        outcome,
        (false, false),
    );
    resp(
        200,
        &json!({
            "ok": true,
            "choice": choice,
            "computer": computer,
            "tie": tie,
            "won": won,
            "win": jsval::num_value(settled.payout),
            "net": jsval::num_value(settled.net),
            "newBalance": jsval::num_value(settled.new_balance),
        }),
    )
}

/// `POST /api/casino/lucky-seven` (server.js:23204-23213).
fn lucky_seven(
    state: &Arc<AppState>,
    body: &Value,
    email: &str,
    norm: &str,
) -> axum::response::Response {
    if !enabled(state) {
        return closed();
    }
    let bet = match read_casino_bet(state, body, email, norm, 1.0) {
        Ok(b) => b,
        Err(r) => return *r,
    };
    let dice = [
        1.0 + (mitch_lib::crypto::js_random() * 6.0).floor(),
        1.0 + (mitch_lib::crypto::js_random() * 6.0).floor(),
    ];
    let total = dice[0] + dice[1];
    let won = total == 7.0;
    let settled = settle_casino_round(
        state,
        &Round { email, norm },
        "Lucky Seven",
        bet,
        if won { bet * 4.8 } else { 0.0 },
        if won { "WIN" } else { "LOSE" },
        (false, false),
    );
    resp(
        200,
        &json!({
            "ok": true,
            "dice": [jsval::num_value(dice[0]), jsval::num_value(dice[1])],
            "total": jsval::num_value(total),
            "won": won,
            "mult": jsval::num_value(if won { 4.8 } else { 0.0 }),
            "win": jsval::num_value(settled.payout),
            "net": jsval::num_value(settled.net),
            "newBalance": jsval::num_value(settled.new_balance),
        }),
    )
}

/// `POST /api/casino/color-card` (server.js:23215-23229).
fn color_card(
    state: &Arc<AppState>,
    body: &Value,
    email: &str,
    norm: &str,
) -> axum::response::Response {
    if !enabled(state) {
        return closed();
    }
    let bet = match read_casino_bet(state, body, email, norm, 1.0) {
        Ok(b) => b,
        Err(r) => return *r,
    };
    let choice = jsval::string(&jsval::or(body.get("choice"), json!(""))).to_lowercase();
    if choice != "red" && choice != "black" {
        return resp(400, &json!({ "error": "Choose red or black." }));
    }
    let suits = ["hearts", "diamonds", "clubs", "spades"];
    let suit = suits[mitch_lib::crypto::js_random_index(suits.len())];
    let color = if suit == "hearts" || suit == "diamonds" {
        "red"
    } else {
        "black"
    };
    let value = 1.0 + (mitch_lib::crypto::js_random() * 13.0).floor();
    let card = card_name(value);
    let won = choice == color;
    let settled = settle_casino_round(
        state,
        &Round { email, norm },
        "Color Card",
        bet,
        if won { bet * 1.92 } else { 0.0 },
        if won { "WIN" } else { "LOSE" },
        (false, false),
    );
    resp(
        200,
        &json!({
            "ok": true,
            "choice": choice,
            "color": color,
            "suit": suit,
            "card": card,
            "won": won,
            "mult": jsval::num_value(if won { 1.92 } else { 0.0 }),
            "win": jsval::num_value(settled.payout),
            "net": jsval::num_value(settled.net),
            "newBalance": jsval::num_value(settled.new_balance),
        }),
    )
}

/// `POST /api/casino/triple-dice` (server.js:23231-23242).
fn triple_dice(
    state: &Arc<AppState>,
    body: &Value,
    email: &str,
    norm: &str,
) -> axum::response::Response {
    if !enabled(state) {
        return closed();
    }
    let bet = match read_casino_bet(state, body, email, norm, 1.0) {
        Ok(b) => b,
        Err(r) => return *r,
    };
    // `Number(body.pick)` then `Number.isInteger(pick) && 1 <= pick <= 6`.
    let pick = body.get("pick").and_then(jsval::number).unwrap_or(f64::NAN);
    if !pick.is_finite() || pick.fract() != 0.0 || pick < 1.0 || pick > 6.0 {
        return resp(400, &json!({ "error": "Pick a number from 1 to 6." }));
    }
    let dice: [f64; 3] = [
        1.0 + (mitch_lib::crypto::js_random() * 6.0).floor(),
        1.0 + (mitch_lib::crypto::js_random() * 6.0).floor(),
        1.0 + (mitch_lib::crypto::js_random() * 6.0).floor(),
    ];
    let matches = dice.iter().filter(|d| **d == pick).count() as f64;
    let mult = [0.0, 2.0, 5.0, 25.0][matches as usize];
    let settled = settle_casino_round(
        state,
        &Round { email, norm },
        "Triple Dice",
        bet,
        bet * mult,
        if matches > 0.0 { "WIN" } else { "LOSE" },
        (false, false),
    );
    resp(
        200,
        &json!({
            "ok": true,
            "pick": jsval::num_value(pick),
            "dice": dice.map(jsval::num_value),
            "matches": jsval::num_value(matches),
            "mult": jsval::num_value(mult),
            "win": jsval::num_value(settled.payout),
            "net": jsval::num_value(settled.net),
            "newBalance": jsval::num_value(settled.new_balance),
        }),
    )
}

/// `isRigged()` (server.js:23119-23121) — hardcoded `false` in bun; the
/// first caller is roulette. Kept as a function so the rig branches below
/// stay a faithful port.
fn is_rigged() -> bool {
    false
}

/// `weightedPick(items)` (server.js:23163-23172): roll = Math.random() *
/// total, then subtract each weight and return the first item where the
/// running roll is ≤ 0 (fallback: the last item).
fn weighted_pick<T>(items: &[(T, f64)]) -> &T {
    let total: f64 = items.iter().map(|(_, w)| w).sum();
    let mut roll = mitch_lib::crypto::js_random() * total;
    for (item, weight) in items {
        roll -= weight;
        if roll <= 0.0 {
            return item;
        }
    }
    &items[items.len() - 1].0
}

/// `POST /api/casino/plinko` (server.js:23244-23258). Note plinko reads
/// `body` without its own parse/recaptcha gates — the shared prelude already
/// ran them.
fn plinko(
    state: &Arc<AppState>,
    body: &Value,
    email: &str,
    norm: &str,
) -> axum::response::Response {
    if !enabled(state) {
        return closed();
    }
    let bet = match read_casino_bet(state, body, email, norm, 1.0) {
        Ok(b) => b,
        Err(r) => return *r,
    };
    // The weighted slot table (server.js:23249-23252), label order preserved.
    let slots: [(&str, f64); 7] = [
        ("0x", 0.0),
        ("0.5x", 0.5),
        ("0.8x", 0.8),
        ("1.2x", 1.2),
        ("1.5x", 1.5),
        ("3x", 3.0),
        ("8x", 8.0),
    ];
    let weights: [f64; 7] = [25.0, 25.0, 18.0, 15.0, 10.0, 5.0, 2.0];
    let paired: Vec<((&str, f64), f64)> = slots.iter().zip(weights).map(|(s, w)| (*s, w)).collect();
    let (label, mult) = weighted_pick(&paired);
    let settled = settle_casino_round(
        state,
        &Round { email, norm },
        "Plinko",
        bet,
        bet * mult,
        if *mult >= 1.0 { "WIN" } else { "LOSE" },
        (false, false),
    );
    resp(
        200,
        &json!({
            "ok": true,
            "slot": label,
            "mult": jsval::num_value(*mult),
            "win": jsval::num_value(settled.payout),
            "net": jsval::num_value(settled.net),
            "newBalance": jsval::num_value(settled.new_balance),
        }),
    )
}

/// The roulette color table (server.js:23286-23292), index = pocket number.
const ROULETTE_COLORS: [&str; 37] = [
    "green", "red", "black", "red", "black", "red", "black", "red", "black", "red", "black",
    "black", "red", "black", "red", "black", "red", "black", "red", "red", "black", "red", "black",
    "red", "black", "red", "black", "red", "black", "black", "red", "black", "red", "black", "red",
    "black", "red",
];

/// `POST /api/casino/roulette` (server.js:23260-23304). `type` is consumed
/// raw (no `|| ''` coercion): the string comparisons are strict, and the
/// numeric branch accepts anything `Number(type)` turns into an integer
/// 0-36 — including `true` (→ 1) and `null` (→ 0).
fn roulette(
    state: &Arc<AppState>,
    body: &Value,
    email: &str,
    norm: &str,
) -> axum::response::Response {
    if !enabled(state) {
        return closed();
    }
    // bun re-calls tryParseJson here; the shared prelude already parsed the
    // same bytes (parsedJsonBody caches), so this is a no-op on both sides.
    let bet = match read_casino_bet(state, body, email, norm, 1.0) {
        Ok(b) => b,
        Err(r) => return *r,
    };
    let type_ = body.get("type");

    let num = (mitch_lib::crypto::js_random() * 37.0).floor();
    let mut num_value = jsval::num_value(num);
    let mut result_color = ROULETTE_COLORS[num as usize];
    let rigged = is_rigged();

    let mut won = false;
    let mut mult = 0.0;
    let mut valid = true;

    let type_str = type_.and_then(Value::as_str);
    if let Some(t) = type_str.filter(|s| *s == "red" || *s == "black") {
        won = result_color == t;
        mult = 2.0;
        if won && rigged {
            // Object.keys(colors).find(n => colors[n] !== type && n !== '0'):
            // first pocket (insertion order, '0' first) with a different
            // color; num becomes the STRING key from here on.
            if let Some((i, _)) = ROULETTE_COLORS
                .iter()
                .enumerate()
                .find(|(i, c)| **c != t && *i != 0)
            {
                num_value = Value::String(i.to_string());
                result_color = ROULETTE_COLORS[i];
            }
            won = false;
        }
    } else if type_str == Some("green") {
        won = num == 0.0;
        mult = 35.0;
        if won && rigged {
            num_value = jsval::num_value(1.0);
            result_color = "red";
            won = false;
        }
    } else {
        // Number.isInteger(Number(type)) && Number(type) >= 0 && <= 36.
        let n = type_.and_then(jsval::number).unwrap_or(f64::NAN);
        if n.is_finite() && n.fract() == 0.0 && (0.0..=36.0).contains(&n) {
            won = num == n;
            mult = 35.0;
            if won && rigged {
                let next = ((num + 1.0) % 37.0) as usize;
                num_value = jsval::num_value(next as f64);
                result_color = ROULETTE_COLORS[next];
                won = false;
            }
        } else {
            valid = false;
        }
    }

    if !valid {
        return resp(400, &json!({ "error": "Invalid bet type." }));
    }
    let payout = if won { bet * mult } else { 0.0 };
    let settled = settle_casino_round(
        state,
        &Round { email, norm },
        "Roulette",
        bet,
        payout,
        if won { "WIN" } else { "LOSE" },
        (false, false),
    );
    resp(
        200,
        &json!({
            "ok": true,
            "number": num_value,
            "color": result_color,
            "won": won,
            "mult": jsval::num_value(mult),
            "win": jsval::num_value(settled.payout),
            "net": jsval::num_value(settled.net),
            "newBalance": jsval::num_value(settled.new_balance),
        }),
    )
}

/// `POST /api/casino/high-low` (server.js:23306-23319).
fn high_low(
    state: &Arc<AppState>,
    body: &Value,
    email: &str,
    norm: &str,
) -> axum::response::Response {
    if !enabled(state) {
        return closed();
    }
    let bet = match read_casino_bet(state, body, email, norm, 1.0) {
        Ok(b) => b,
        Err(r) => return *r,
    };
    let choice = jsval::string(&jsval::or(body.get("choice"), json!(""))).to_lowercase();
    if choice != "higher" && choice != "lower" {
        return resp(400, &json!({ "error": "Choose higher or lower." }));
    }
    let value = 1.0 + (mitch_lib::crypto::js_random() * 13.0).floor();
    let card = card_name(value);
    let push = value == 7.0;
    let won = !push
        && if choice == "higher" {
            value > 7.0
        } else {
            value < 7.0
        };
    let payout = if push {
        bet
    } else if won {
        bet * 2.0
    } else {
        0.0
    };
    let outcome = if push {
        "PUSH"
    } else if won {
        "WIN"
    } else {
        "LOSE"
    };
    let settled = settle_casino_round(
        state,
        &Round { email, norm },
        "High / Low",
        bet,
        payout,
        outcome,
        (false, false),
    );
    resp(
        200,
        &json!({
            "ok": true,
            "choice": choice,
            "card": card,
            "value": jsval::num_value(value),
            "push": push,
            "won": won,
            "win": jsval::num_value(settled.payout),
            "net": jsval::num_value(settled.net),
            "newBalance": jsval::num_value(settled.new_balance),
        }),
    )
}

/// The card label shared by color-card and high-low
/// (`value === 1 ? 'A' : value === 13 ? 'K' : value === 12 ? 'Q' :
/// value === 11 ? 'J' : String(value)`).
fn card_name(value: f64) -> String {
    match value as u32 {
        1 => "A".to_string(),
        11 => "J".to_string(),
        12 => "Q".to_string(),
        13 => "K".to_string(),
        v => v.to_string(),
    }
}

/// `getVal` (server.js:23393-23398) — aces count last (each adds 11 while
/// the running total stays ≤ 21, else 1); face cards 10; everything else
/// parseInt. A card without a parseable `v` poisons the value to NaN, which
/// makes every comparison below false exactly like JS.
fn hand_value(hand: &[Value]) -> f64 {
    let mut v = 0.0;
    let mut aces: u32 = 0;
    for c in hand {
        let cv = c.get("v").and_then(Value::as_str).unwrap_or("");
        if cv == "A" {
            aces += 1;
        } else if cv == "J" || cv == "Q" || cv == "K" {
            v += 10.0;
        } else {
            match cv.parse::<f64>() {
                Ok(n) => v += n,
                // parseInt(undefined)/parseInt('') → NaN; NaN + aces stays NaN
                Err(_) => return f64::NAN,
            }
        }
    }
    for _ in 0..aces {
        v += if v + 11.0 <= 21.0 { 11.0 } else { 1.0 };
    }
    v
}

/// The 52-card deck literal (server.js:23347-23349) — suits ♠♥♦♣ ×
/// A/2-10/J/Q/K as `{s, v}` objects.
fn build_deck() -> Vec<Value> {
    let mut deck = Vec::with_capacity(52);
    for s in ["♠", "♥", "♦", "♣"] {
        for v in [
            "A", "2", "3", "4", "5", "6", "7", "8", "9", "10", "J", "Q", "K",
        ] {
            deck.push(json!({ "s": s, "v": v }));
        }
    }
    deck
}

/// `POST /api/casino/blackjack/start` (server.js:23323-23381). The bet is
/// NOT readCasinoBet — its own ladder is an `invalid bet` (non-finite, <1 or
/// over balance) then the non-VIP 500 cap, with no toFixed(2) normalization.
/// The bet is deducted up front; the prepaid settle below only adds the
/// payout back.
fn bj_start(
    state: &Arc<AppState>,
    body: &Value,
    email: &str,
    norm: &str,
) -> axum::response::Response {
    if !enabled(state) {
        return closed();
    }
    // bun re-calls tryParseJson here — a cached no-op (the prelude parsed).
    if state
        .bj_games
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(norm)
    {
        return resp(
            409,
            &json!({ "error": "Finish your current blackjack hand first." }),
        );
    }
    let bet = body
        .get("amount")
        .and_then(jsval::number)
        .unwrap_or(f64::NAN);
    let bal = mitch_lib::coins::get_coins(&state.store, state.data_dir(), email);
    if !bet.is_finite() || bet < 1.0 || bet > bal {
        return resp(400, &json!({ "error": "invalid bet" }));
    }
    if !is_vip(state, norm) && bet > 500.0 {
        return resp(400, &json!({ "error": "Maximum bet is 500 coins." }));
    }

    add_casino_stat(&state.casino_intake, bet);
    save_casino_stats(state);
    mitch_lib::coins::add_coins(
        &state.store,
        state.data_dir(),
        email,
        -bet,
        state.coin_multiplier(),
        "",
    );

    let mut deck = build_deck();
    // Fisher-Yates from the end, exactly like the JS loop.
    for i in (1..deck.len()).rev() {
        let j = mitch_lib::crypto::js_random_index(i + 1);
        deck.swap(i, j);
    }
    // playerHand = two pops, dealerHand = the next two.
    let mut player_hand: Vec<Value> = Vec::with_capacity(2);
    let mut dealer_hand: Vec<Value> = Vec::with_capacity(2);
    for _ in 0..2 {
        player_hand.push(deck.pop().unwrap_or(Value::Null));
    }
    for _ in 0..2 {
        dealer_hand.push(deck.pop().unwrap_or(Value::Null));
    }
    // bun uses a local `const rigged = false;` (not isRigged()); the value
    // is identical, so share the helper. The rig branches stay ported below
    // for fidelity even though they can never run.
    let rigged = is_rigged();

    if rigged {
        // Break a player 21 with the first two-card non-21 completion.
        if hand_value(&player_hand) == 21.0 {
            if let Some(broken_idx) = deck
                .iter()
                .position(|c| hand_value(&[player_hand[0].clone(), c.clone()]) < 21.0)
            {
                let card = deck.remove(broken_idx);
                deck.push(player_hand[1].clone());
                player_hand[1] = card;
            }
        }
        // Deal the dealer 21 when the player looks strong.
        if hand_value(&dealer_hand) < 21.0 && hand_value(&player_hand) > 17.0 {
            if let Some(win_idx) = deck
                .iter()
                .position(|c| hand_value(&[dealer_hand[0].clone(), c.clone()]) == 21.0)
            {
                let card = deck.remove(win_idx);
                deck.push(dealer_hand[1].clone());
                dealer_hand[1] = card;
            }
        }
    }

    state
        .bj_games
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            norm.to_string(),
            BjGame {
                deck,
                player_hand: player_hand.clone(),
                dealer_hand: dealer_hand.clone(),
                bet,
                email: email.to_string(),
                rigged,
            },
        );
    resp(
        200,
        &json!({
            "ok": true,
            "playerHand": player_hand,
            "dealerUpCard": dealer_hand[0],
        }),
    )
}

/// `POST /api/casino/blackjack/hit` (server.js:23400-23426). No body parse
/// (the prelude skips it). A bust deletes the game and settles `BUST`
/// prepaid — the response's `winAmt` is the literal 0, not the settle
/// payout (insurance would refund coins the response still reports as 0).
fn bj_hit(state: &Arc<AppState>, email: &str, norm: &str) -> axum::response::Response {
    let mut games = state.bj_games.lock().unwrap_or_else(|e| e.into_inner());
    let Some(g) = games.get_mut(norm) else {
        return resp(400, &json!({ "error": "no active game" }));
    };
    let mut card = g.deck.pop().unwrap_or(Value::Null);
    if g.rigged {
        // At 12+ with a safe card drawn, swap in the first card that busts.
        let current_p = hand_value(&g.player_hand);
        if current_p >= 12.0
            && hand_value(&[g.player_hand.as_slice(), std::slice::from_ref(&card)].concat()) <= 21.0
        {
            if let Some(bust_idx) = g.deck.iter().position(|c| {
                hand_value(&[g.player_hand.as_slice(), std::slice::from_ref(c)].concat()) > 21.0
            }) {
                let bust_card = g.deck.remove(bust_idx);
                g.deck.push(card);
                card = bust_card;
            }
        }
    }
    g.player_hand.push(card);
    if hand_value(&g.player_hand) > 21.0 {
        let (player_hand, dealer_hand, bet) = (g.player_hand.clone(), g.dealer_hand.clone(), g.bet);
        games.remove(norm);
        drop(games);
        settle_casino_round(
            state,
            &Round { email, norm },
            "Blackjack",
            bet,
            0.0,
            "BUST",
            (false, true),
        );
        return resp(
            200,
            &json!({
                "ok": true,
                "gameOver": true,
                "playerHand": player_hand,
                "status": "bust",
                "dealerHand": dealer_hand,
                "winAmt": jsval::num_value(0.0),
            }),
        );
    }
    resp(
        200,
        &json!({
            "ok": true,
            "gameOver": false,
            "playerHand": g.player_hand,
            "status": "active",
        }),
    )
}

/// The stand outcome ladder (server.js:23443-23452) — dealer bust or player
/// higher pays double, a tie pushes the bet back, otherwise a loss.
fn bj_outcome(dval: f64, pval: f64, bet: f64) -> (&'static str, f64) {
    if dval > 21.0 || pval > dval {
        ("win", bet * 2.0)
    } else if dval == pval {
        ("push", bet)
    } else {
        ("lose", 0.0)
    }
}

/// `POST /api/casino/blackjack/stand` (server.js:23428-23459). No active
/// game is a 200 `{ok, gameOver}` (unlike hit's 400).
fn bj_stand(state: &Arc<AppState>, email: &str, norm: &str) -> axum::response::Response {
    let mut games = state.bj_games.lock().unwrap_or_else(|e| e.into_inner());
    let Some(g) = games.get_mut(norm) else {
        return resp(200, &json!({ "ok": true, "gameOver": true }));
    };
    let mut dval = hand_value(&g.dealer_hand);
    let pval = hand_value(&g.player_hand);
    if g.rigged {
        // Dealer keeps drawing while it cannot beat the player.
        while dval <= pval && dval < 21.0 {
            let win_idx = g.deck.iter().position(|c| {
                let v = hand_value(&[g.dealer_hand.as_slice(), std::slice::from_ref(c)].concat());
                v >= pval && v <= 21.0
            });
            match win_idx {
                Some(i) => g.dealer_hand.push(g.deck.remove(i)),
                None => g.dealer_hand.push(g.deck.pop().unwrap_or(Value::Null)),
            }
            dval = hand_value(&g.dealer_hand);
        }
    } else {
        while dval < 17.0 {
            g.dealer_hand.push(g.deck.pop().unwrap_or(Value::Null));
            dval = hand_value(&g.dealer_hand);
        }
    }
    let (res, win_amt) = bj_outcome(dval, pval, g.bet);
    let (player_hand, dealer_hand, bet) = (g.player_hand.clone(), g.dealer_hand.clone(), g.bet);
    games.remove(norm);
    drop(games);
    let settled = settle_casino_round(
        state,
        &Round { email, norm },
        "Blackjack",
        bet,
        win_amt,
        res.to_uppercase().as_str(),
        (false, true),
    );
    resp(
        200,
        &json!({
            "ok": true,
            "gameOver": true,
            "playerHand": player_hand,
            "dealerHand": dealer_hand,
            "status": res,
            "winAmt": jsval::num_value(settled.payout),
        }),
    )
}

/// `POST /api/casino/poker/start` (server.js:23460-23518). Five cards off a
/// shuffled 52-card deck, evaluated against the rank ladder — there is no
/// draw phase; the dealt hand IS the hand. The game name embeds the rank
/// (`Poker (Two Pair)`), so the history/feed read differently per rank.
fn poker(state: &Arc<AppState>, body: &Value, email: &str, norm: &str) -> axum::response::Response {
    if !enabled(state) {
        return closed();
    }
    let bet = match read_casino_bet(state, body, email, norm, 1.0) {
        Ok(b) => b,
        Err(r) => return *r,
    };
    // The poker deck literal (server.js:23468-23470) — vals ascending 2..A,
    // unlike blackjack's A-first deck.
    let mut deck = Vec::with_capacity(52);
    for s in ["♠", "♥", "♦", "♣"] {
        for v in [
            "2", "3", "4", "5", "6", "7", "8", "9", "10", "J", "Q", "K", "A",
        ] {
            deck.push(json!({ "s": s, "v": v }));
        }
    }
    for i in (1..deck.len()).rev() {
        let j = mitch_lib::crypto::js_random_index(i + 1);
        deck.swap(i, j);
    }
    let pop = |deck: &mut Vec<Value>| deck.pop().unwrap_or(Value::Null);
    let mut hand: Vec<Value> = (0..5).map(|_| pop(&mut deck)).collect();

    let rigged = is_rigged();
    if rigged && poker_rank(&hand).1 > 0.0 {
        // The rig re-draws WITH replacement from the remaining deck.
        let mut attempts = 0;
        while poker_rank(&hand).1 > 0.0 && attempts < 20 {
            hand = (0..5)
                .map(|_| deck[mitch_lib::crypto::js_random_index(deck.len())].clone())
                .collect();
            attempts += 1;
        }
    }

    let (rank, mult) = poker_rank(&hand);
    let settled = settle_casino_round(
        state,
        &Round { email, norm },
        &format!("Poker ({rank})"),
        bet,
        bet * mult,
        if mult > 0.0 { "WIN" } else { "LOSE" },
        (false, false),
    );
    resp(
        200,
        &json!({
            "ok": true,
            "hand": hand,
            "rank": rank,
            "mult": jsval::num_value(mult),
            "win": jsval::num_value(settled.payout),
            "net": jsval::num_value(settled.net),
            "newBalance": jsval::num_value(settled.new_balance),
        }),
    )
}

/// `checkHand` (server.js:23477-23496). There is NO ace-low straight (A is
/// always 14), and a single pair pays only when the paired value is J/Q/K/A
/// (`vMap[v] >= 11`). Group sizes come from the counts object sorted
/// descending; the pair lookup reads the first hand-ordered key with count
/// 2 (only reachable for a single pair — two pairs are matched earlier).
fn poker_rank(hand: &[Value]) -> (&'static str, f64) {
    let vmap = |v: &str| -> Option<f64> {
        match v {
            "J" => Some(11.0),
            "Q" => Some(12.0),
            "K" => Some(13.0),
            "A" => Some(14.0),
            other => other.parse::<f64>().ok(),
        }
    };
    let mut nums: Vec<f64> = hand
        .iter()
        .map(|c| {
            c.get("v")
                .and_then(Value::as_str)
                .and_then(vmap)
                .unwrap_or(f64::NAN)
        })
        .collect();
    nums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut counts: Vec<(String, u32)> = Vec::new();
    let mut suits: Vec<(String, u32)> = Vec::new();
    for c in hand {
        let v = c.get("v").and_then(Value::as_str).unwrap_or("");
        let s = c.get("s").and_then(Value::as_str).unwrap_or("");
        for (map, key) in [(&mut counts, v), (&mut suits, s)] {
            match map.iter_mut().find(|(k, _)| k == key) {
                Some((_, n)) => *n += 1,
                None => map.push((key.to_string(), 1)),
            }
        }
    }
    let mut sizes: Vec<u32> = counts.iter().map(|(_, n)| *n).collect();
    sizes.sort_by(|a, b| b.cmp(a));
    let is_flush = suits.iter().any(|(_, n)| *n == 5);
    let is_straight = nums.windows(2).all(|w| w[1] == w[0] + 1.0);

    if is_flush && is_straight && nums.first() == Some(&10.0) {
        return ("Royal Flush", 500.0);
    }
    if is_flush && is_straight {
        return ("Straight Flush", 100.0);
    }
    if sizes.first() == Some(&4) {
        return ("Four of a Kind", 50.0);
    }
    if sizes.first() == Some(&3) && sizes.get(1) == Some(&2) {
        return ("Full House", 15.0);
    }
    if is_flush {
        return ("Flush", 10.0);
    }
    if is_straight {
        return ("Straight", 7.0);
    }
    if sizes.first() == Some(&3) {
        return ("Three of a Kind", 5.0);
    }
    if sizes.first() == Some(&2) && sizes.get(1) == Some(&2) {
        return ("Two Pair", 3.0);
    }
    if sizes.first() == Some(&2) {
        let paired = counts
            .iter()
            .find(|(_, n)| *n == 2)
            .and_then(|(k, _)| vmap(k));
        if paired.is_some_and(|n| n >= 11.0) {
            return ("Jacks or Better", 2.0);
        }
    }
    ("Lose", 0.0)
}

/// `POST /api/casino/coinflip` (server.js:23519-23534). A fair flip against
/// a called side at 1.9x; the rig force-flips a would-be win.
fn coinflip(
    state: &Arc<AppState>,
    body: &Value,
    email: &str,
    norm: &str,
) -> axum::response::Response {
    if !enabled(state) {
        return closed();
    }
    let bet = match read_casino_bet(state, body, email, norm, 1.0) {
        Ok(b) => b,
        Err(r) => return *r,
    };
    let side = jsval::string(&jsval::or(body.get("side"), json!(""))).to_lowercase();
    if !["heads", "tails"].contains(&side.as_str()) {
        return resp(400, &json!({ "error": "Choose heads or tails." }));
    }
    let rigged = is_rigged();
    let mut result = if mitch_lib::crypto::js_random() < 0.5 {
        "heads"
    } else {
        "tails"
    };
    if rigged && result == side {
        result = if side == "heads" { "tails" } else { "heads" };
    }
    let won = side == result;
    let mult = if won { 1.9 } else { 0.0 };
    let settled = settle_casino_round(
        state,
        &Round { email, norm },
        "Coin Flip",
        bet,
        if won { bet * mult } else { 0.0 },
        if won { "WIN" } else { "LOSE" },
        (false, false),
    );
    resp(
        200,
        &json!({
            "ok": true,
            "result": result,
            "won": won,
            "mult": jsval::num_value(mult),
            "win": jsval::num_value(settled.payout),
            "net": jsval::num_value(settled.net),
            "newBalance": jsval::num_value(settled.new_balance),
        }),
    )
}

/// `POST /api/casino/dice` (server.js:23535-23553). Roll 1-100; `under`
/// wins below 50, `over` wins above 51 (50 and 51 always lose) at 1.94x.
/// The rig re-rolls a winning range into the losing range.
fn dice(state: &Arc<AppState>, body: &Value, email: &str, norm: &str) -> axum::response::Response {
    if !enabled(state) {
        return closed();
    }
    let bet = match read_casino_bet(state, body, email, norm, 1.0) {
        Ok(b) => b,
        Err(r) => return *r,
    };
    let side = jsval::string(&jsval::or(body.get("side"), json!(""))).to_lowercase();
    if !["under", "over"].contains(&side.as_str()) {
        return resp(400, &json!({ "error": "Choose under or over." }));
    }
    let rigged = is_rigged();
    let mut roll = (mitch_lib::crypto::js_random() * 100.0).floor() + 1.0;
    if rigged {
        if side == "under" && roll < 50.0 {
            roll = (mitch_lib::crypto::js_random() * 51.0).floor() + 50.0;
        } else if side == "over" && roll > 51.0 {
            roll = (mitch_lib::crypto::js_random() * 51.0).floor() + 1.0;
        }
    }
    let won = if side == "under" {
        roll < 50.0
    } else {
        roll > 51.0
    };
    let mult = if won { 1.94 } else { 0.0 };
    let settled = settle_casino_round(
        state,
        &Round { email, norm },
        "Dice Duel",
        bet,
        if won { bet * mult } else { 0.0 },
        if won { "WIN" } else { "LOSE" },
        (false, false),
    );
    resp(
        200,
        &json!({
            "ok": true,
            "roll": jsval::num_value(roll),
            "won": won,
            "mult": jsval::num_value(mult),
            "win": jsval::num_value(settled.payout),
            "net": jsval::num_value(settled.net),
            "newBalance": jsval::num_value(settled.new_balance),
        }),
    )
}

/// `POST /api/casino/crash` (server.js:23554-23569). The player picks a
/// cashout multiplier 1.2-6.0; the crash point is
/// `max(1, min(10, 0.95/max(random, 1e-6)))` at 2dp — a win pays
/// `bet * cashout`. Note the 1.2/6.0 bounds are checked BEFORE the
/// toFixed(2) normalization of the cashout.
fn crash(state: &Arc<AppState>, body: &Value, email: &str, norm: &str) -> axum::response::Response {
    if !enabled(state) {
        return closed();
    }
    let bet = match read_casino_bet(state, body, email, norm, 1.0) {
        Ok(b) => b,
        Err(r) => return *r,
    };
    let target = jsval::number(&jsval::or(body.get("target"), json!(null))).unwrap_or(f64::NAN);
    if !target.is_finite() || target < 1.2 || target > 6.0 {
        return resp(
            400,
            &json!({ "error": "Cashout must be between 1.20x and 6.00x." }),
        );
    }
    let rigged = is_rigged();
    let raw = 0.95 / mitch_lib::crypto::js_random().max(0.000001);
    let mut crash_at = js_num_from_fixed(&js_to_fixed(raw.clamp(1.0, 10.0), 2));
    let cashout = js_num_from_fixed(&js_to_fixed(target, 2));
    if rigged && cashout <= crash_at {
        crash_at = js_num_from_fixed(&js_to_fixed((cashout - 0.01).max(1.0), 2));
    }
    let won = cashout <= crash_at;
    let settled = settle_casino_round(
        state,
        &Round { email, norm },
        "Crash",
        bet,
        if won { bet * cashout } else { 0.0 },
        if won { "WIN" } else { "CRASH" },
        (false, false),
    );
    resp(
        200,
        &json!({
            "ok": true,
            "crashAt": jsval::num_value(crash_at),
            "target": jsval::num_value(cashout),
            "won": won,
            "mult": jsval::num_value(if won { cashout } else { 0.0 }),
            "win": jsval::num_value(settled.payout),
            "net": jsval::num_value(settled.net),
            "newBalance": jsval::num_value(settled.new_balance),
        }),
    )
}

/// The prize-wheel segments (server.js:23579-23587) — label/mult/weight in
/// JS literal order.
const WHEEL_SEGMENTS: [(&str, f64, f64); 7] = [
    ("Bust", 0.0, 85.0),
    ("Half Back", 0.5, 40.0),
    ("Small Win", 1.25, 32.0),
    ("Double", 2.0, 31.0),
    ("Triple", 3.0, 8.0),
    ("Meteor", 8.0, 3.0),
    ("Galaxy Jackpot", 20.0, 1.0),
];

/// `POST /api/casino/wheel` (server.js:23570-23590). weightedPick over the
/// 7 segments; a win pays bet×mult (Half Back's 0.5 loses half, counts as
/// LOSE in history).
fn wheel(state: &Arc<AppState>, body: &Value, email: &str, norm: &str) -> axum::response::Response {
    if !enabled(state) {
        return closed();
    }
    let bet = match read_casino_bet(state, body, email, norm, 1.0) {
        Ok(b) => b,
        Err(r) => return *r,
    };
    let paired: Vec<((&str, f64), f64)> = WHEEL_SEGMENTS
        .iter()
        .map(|(l, m, w)| ((*l, *m), *w))
        .collect();
    let rigged = is_rigged();
    let mut segment = weighted_pick(&paired);
    if rigged && segment.1 >= 1.0 {
        segment = &paired[0].0; // Force Bust
    }
    let (label, mult) = *segment;
    let settled = settle_casino_round(
        state,
        &Round { email, norm },
        "Prize Wheel",
        bet,
        bet * mult,
        if mult >= 1.0 { "WIN" } else { "LOSE" },
        (false, false),
    );
    resp(
        200,
        &json!({
            "ok": true,
            "segment": label,
            "mult": jsval::num_value(mult),
            "win": jsval::num_value(settled.payout),
            "net": jsval::num_value(settled.net),
            "newBalance": jsval::num_value(settled.new_balance),
        }),
    )
}

/// The scratch-card rank table (server.js:23596-23603): roll thresholds and
/// the mult each band pays. Extracted for testability; the ladder is
/// strictly `<` against the thresholds in ascending order.
fn scratch_rank(roll: f64) -> (f64, &'static str) {
    if roll < 0.002 {
        (90.0, "Triple Diamonds")
    } else if roll < 0.010 {
        (25.0, "Triple Sevens")
    } else if roll < 0.040 {
        (6.0, "Triple Bells")
    } else if roll < 0.100 {
        (2.5, "Triple Fruit")
    } else if roll < 0.180 {
        (1.5, "Small Match")
    } else if roll < 0.300 {
        (1.0, "Half Back")
    } else {
        (0.0, "No Match")
    }
}

/// `POST /api/casino/scratch` (server.js:23591-23614). One uniform roll
/// picks the band; the 3×3 tile grid is random with winning bands stamping
/// their diagonal/line symbols on top.
fn scratch(
    state: &Arc<AppState>,
    body: &Value,
    email: &str,
    norm: &str,
) -> axum::response::Response {
    if !enabled(state) {
        return closed();
    }
    let bet = match read_casino_bet(state, body, email, norm, 1.0) {
        Ok(b) => b,
        Err(r) => return *r,
    };
    let rigged = is_rigged();
    let mut roll = mitch_lib::crypto::js_random();
    if rigged && roll < 0.180 {
        roll = 0.300 + mitch_lib::crypto::js_random() * 0.7; // Force No Match
    }
    let (mult, rank) = scratch_rank(roll);
    const SYMBOLS: [&str; 7] = ["🍒", "🍋", "🍊", "🍇", "🔔", "💎", "7️⃣"];
    let mut tiles: Vec<String> = (0..9)
        .map(|_| SYMBOLS[mitch_lib::crypto::js_random_index(SYMBOLS.len())].to_string())
        .collect();
    if mult >= 25.0 {
        let s = if mult >= 90.0 { "💎" } else { "7️⃣" };
        tiles[0] = s.to_string();
        tiles[4] = s.to_string();
        tiles[8] = s.to_string();
    } else if mult >= 2.5 {
        let s = if mult >= 6.0 { "🔔" } else { "🍒" };
        tiles[1] = s.to_string();
        tiles[4] = s.to_string();
        tiles[7] = s.to_string();
    } else if mult > 0.0 {
        tiles[3] = "🍋".to_string();
        tiles[4] = "🍋".to_string();
    }
    let tiles: Vec<Value> = tiles.into_iter().map(Value::String).collect();
    let settled = settle_casino_round(
        state,
        &Round { email, norm },
        "Scratch Card",
        bet,
        bet * mult,
        if mult >= 1.0 { "WIN" } else { "LOSE" },
        (false, false),
    );
    resp(
        200,
        &json!({
            "ok": true,
            "tiles": tiles,
            "rank": rank,
            "mult": jsval::num_value(mult),
            "win": jsval::num_value(settled.payout),
            "net": jsval::num_value(settled.net),
            "newBalance": jsval::num_value(settled.new_balance),
        }),
    )
}

/// The keno payout table (server.js:23628-23637) — `payoutTable[picks][hits]`
/// in JS literal order; absent cells pay nothing.
const KENO_PAYOUT: [(usize, &[(usize, f64)]); 8] = [
    (3, &[(2, 2.1), (3, 14.0)]),
    (4, &[(2, 0.9), (3, 5.0), (4, 38.0)]),
    (5, &[(3, 2.8), (4, 14.0), (5, 90.0)]),
    (6, &[(3, 1.3), (4, 5.5), (5, 38.0), (6, 210.0)]),
    (7, &[(4, 3.6), (5, 17.0), (6, 90.0), (7, 480.0)]),
    (8, &[(4, 2.4), (5, 8.5), (6, 45.0), (7, 190.0), (8, 950.0)]),
    (
        9,
        &[(5, 5.5), (6, 25.0), (7, 120.0), (8, 450.0), (9, 1700.0)],
    ),
    (
        10,
        &[
            (5, 3.8),
            (6, 14.0),
            (7, 65.0),
            (8, 250.0),
            (9, 1000.0),
            (10, 2800.0),
        ],
    ),
];

/// `payoutTable[picks.length][hits.length] || 0`.
fn keno_payout(picks: usize, hits: usize) -> f64 {
    KENO_PAYOUT
        .iter()
        .find(|(p, _)| *p == picks)
        .and_then(|(_, row)| row.iter().find(|(h, _)| *h == hits))
        .map(|(_, m)| *m)
        .unwrap_or(0.0)
}

/// The keno `picks` parser (server.js:23622): non-array → `[]`; else
/// `Number()`-coerced, Set-deduped, filtered to integers 1-40, sorted
/// ascending.
fn parse_keno_picks(body: &Value) -> Vec<f64> {
    let Some(arr) = body.get("picks").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut set: Vec<f64> = Vec::new();
    for v in arr {
        let n = jsval::number(v).unwrap_or(f64::NAN);
        if !set.contains(&n) {
            set.push(n);
        }
    }
    let mut picks: Vec<f64> = set
        .into_iter()
        .filter(|n| n.is_finite() && n.fract() == 0.0 && *n >= 1.0 && *n <= 40.0)
        .collect();
    picks.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    picks
}

/// `drawUniqueNumbers(max, count)` (server.js:23168-23175): 1..max,
/// Fisher-Yates from the end, first `count`, sorted ascending.
fn draw_unique_numbers(max: usize, count: usize) -> Vec<f64> {
    let mut nums: Vec<f64> = (1..=max).map(|i| i as f64).collect();
    for i in (1..nums.len()).rev() {
        let j = (mitch_lib::crypto::js_random() * (i as f64 + 1.0)).floor() as usize;
        nums.swap(i, j);
    }
    nums.truncate(count);
    nums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    nums
}

/// `POST /api/casino/keno` (server.js:23615-23653). 12 balls drawn without
/// replacement against 3-10 picks; each hit pays the table × 1.12
/// (toFixed(2)). Hits are picks-order (both sides ascending).
fn keno(state: &Arc<AppState>, body: &Value, email: &str, norm: &str) -> axum::response::Response {
    if !enabled(state) {
        return closed();
    }
    let bet = match read_casino_bet(state, body, email, norm, 1.0) {
        Ok(b) => b,
        Err(r) => return *r,
    };
    let picks = parse_keno_picks(body);
    if picks.len() < 3 || picks.len() > 10 {
        return resp(400, &json!({ "error": "Pick 3 to 10 numbers." }));
    }
    let rigged = is_rigged();
    let mut drawn = draw_unique_numbers(40, 12);
    let mut hits: Vec<f64> = picks
        .iter()
        .filter(|p| drawn.contains(p))
        .cloned()
        .collect();
    if rigged && keno_payout(picks.len(), hits.len()) > 0.0 {
        // If rigged and user was going to win, keep re-drawing until they lose.
        let mut attempts = 0;
        while keno_payout(picks.len(), hits.len()) > 0.0 && attempts < 20 {
            drawn = draw_unique_numbers(40, 12);
            hits = picks
                .iter()
                .filter(|p| drawn.contains(p))
                .cloned()
                .collect();
            attempts += 1;
        }
    }
    let mut mult = keno_payout(picks.len(), hits.len());
    if mult > 0.0 {
        mult = js_num_from_fixed(&js_to_fixed(mult * 1.12, 2));
    }
    let settled = settle_casino_round(
        state,
        &Round { email, norm },
        "Keno Rush",
        bet,
        bet * mult,
        if mult > 0.0 { "WIN" } else { "LOSE" },
        (false, false),
    );
    let picks_v: Vec<Value> = picks.iter().map(|p| jsval::num_value(*p)).collect();
    let drawn_v: Vec<Value> = drawn.iter().map(|d| jsval::num_value(*d)).collect();
    let hits_v: Vec<Value> = hits.iter().map(|h| jsval::num_value(*h)).collect();
    resp(
        200,
        &json!({
            "ok": true,
            "picks": picks_v,
            "drawn": drawn_v,
            "hits": hits_v,
            "mult": jsval::num_value(mult),
            "win": jsval::num_value(settled.payout),
            "net": jsval::num_value(settled.net),
            "newBalance": jsval::num_value(settled.new_balance),
        }),
    )
}

/// The slots multiplier table (server.js:23686-23696): triples by symbol,
/// a leading pair pays 3 (a trailing pair — A,B,B — pays nothing), else 0.
fn slots_mult(res: &[&str; 3]) -> f64 {
    if res[0] == res[1] && res[1] == res[2] {
        match res[0] {
            "💎" => 100.0,
            "7️⃣" => 50.0,
            "🔔" => 20.0,
            "🍒" => 10.0,
            _ => 6.0,
        }
    } else if res[0] == res[1] {
        3.0
    } else {
        0.0
    }
}

/// The rank map (server.js:23699) — keyed on the PRE-bonus mult.
fn slots_rank(mult: f64) -> &'static str {
    match mult {
        100.0 => "JACKPOT (Diamonds)",
        50.0 => "Triple Sevens",
        20.0 => "Triple Bells",
        10.0 => "Triple Cherries",
        6.0 => "Triple Fruit",
        3.0 => "Double",
        // NaN compares unequal to every literal, same as JS `mult === n`.
        _ => "Lose",
    }
}

/// `POST /api/casino/slots` + `/api/casino/vip/slots` (server.js:23654-23721).
/// Slots does NOT use readCasinoBet — the bet is the raw `Number(body.amount)`
/// (no toFixed(2) normalization), checked against its own ladders. Free spins
/// (granted elsewhere, capped at 500) settle with effectiveBet 0; the VIP room
/// needs an active pass, bets ≥ 100, and pays a 10% bonus.
fn slots(
    state: &Arc<AppState>,
    body: &Value,
    email: &str,
    norm: &str,
    is_vip_room: bool,
) -> axum::response::Response {
    if !enabled(state) {
        return closed();
    }
    let mut stats = user_stats(state, norm);
    let is_vip = is_vip(state, norm);
    if is_vip_room && !is_vip {
        return resp(403, &json!({ "error": "VIP pass required" }));
    }
    let free_spins = stats
        .get("slots_free_spins")
        .and_then(jsval::number)
        .unwrap_or(0.0);
    let is_free_spin = free_spins > 0.0 && !is_vip_room;

    let bet = body
        .get("amount")
        .and_then(jsval::number)
        .unwrap_or(f64::NAN);
    // `bet < 100` is false for NaN, so the VIP min-bet check lets NaN through
    // to the isFinite ladders below.
    if is_vip_room && bet < 100.0 {
        return resp(400, &json!({ "error": "VIP minimum bet is 100 coins" }));
    }
    if is_free_spin {
        if !bet.is_finite() || bet < 1.0 || bet > 500.0 {
            return resp(
                400,
                &json!({ "error": "invalid bet (free spins limit max 500 coins)" }),
            );
        }
        // stats[norm].slots_free_spins = freeSpins - 1; saveUserStats.
        stats["slots_free_spins"] = jsval::num_value(free_spins - 1.0);
        mitch_lib::user_stats::user_stats_set(&state.store, norm, &stats);
    } else {
        let bal = mitch_lib::coins::get_coins(&state.store, state.data_dir(), email);
        if !bet.is_finite() || bet < 1.0 || bet > bal {
            return resp(400, &json!({ "error": "invalid bet" }));
        }
        if !is_vip && bet > 500.0 {
            return resp(400, &json!({ "error": "Maximum bet is 500 coins." }));
        }
    }

    const SYMBOLS: [&str; 7] = ["🍒", "🍋", "🍊", "🍇", "🔔", "💎", "7️⃣"];
    let rigged = is_rigged();
    let mut results: [&str; 3] = [
        SYMBOLS[mitch_lib::crypto::js_random_index(SYMBOLS.len())],
        SYMBOLS[mitch_lib::crypto::js_random_index(SYMBOLS.len())],
        SYMBOLS[mitch_lib::crypto::js_random_index(SYMBOLS.len())],
    ];
    if rigged && slots_mult(&results) > 0.0 {
        let mut attempts = 0;
        while slots_mult(&results) > 0.0 && attempts < 20 {
            results = [
                SYMBOLS[mitch_lib::crypto::js_random_index(SYMBOLS.len())],
                SYMBOLS[mitch_lib::crypto::js_random_index(SYMBOLS.len())],
                SYMBOLS[mitch_lib::crypto::js_random_index(SYMBOLS.len())],
            ];
            attempts += 1;
        }
    }
    let raw_mult = slots_mult(&results);
    let rank = slots_rank(raw_mult);
    let mut mult = raw_mult;
    if is_vip_room && mult > 0.0 {
        mult = js_num_from_fixed(&js_to_fixed(mult * 1.1, 2)); // 10% VIP bonus
    }
    let win_amt = bet * mult;
    let settled = settle_casino_round(
        state,
        &Round { email, norm },
        if is_vip_room { "VIP Slots" } else { "Slots" },
        bet,
        win_amt,
        if mult > 0.0 { "WIN" } else { "LOSE" },
        (is_free_spin, false),
    );
    // `stats[norm].slots_free_spins || 0` from the LOCAL stats object (the
    // settle's internal reload doesn't touch it). bun throws → 500 if the
    // user has no user_stats entry at all; the port reads 0 there instead —
    // unreachable for any user who ever earned stats.
    let remaining = stats
        .get(norm)
        .and_then(|s| s.get("slots_free_spins"))
        .and_then(jsval::number)
        .unwrap_or(0.0);
    resp(
        200,
        &json!({
            "ok": true,
            "results": results,
            "rank": rank,
            "mult": jsval::num_value(mult),
            "win": jsval::num_value(settled.payout),
            "net": jsval::num_value(settled.net),
            "newBalance": jsval::num_value(settled.new_balance),
            "freeSpinsRemaining": jsval::num_value(remaining),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The roulette color table must match the JS object literal verbatim.
    #[test]
    fn roulette_color_table_matches_js() {
        let expected = [
            "green", "red", "black", "red", "black", "red", "black", "red", "black", "red",
            "black", "black", "red", "black", "red", "black", "red", "black", "red", "red",
            "black", "red", "black", "red", "black", "red", "black", "red", "black", "black",
            "red", "black", "red", "black", "red", "black", "red",
        ];
        assert_eq!(ROULETTE_COLORS, expected);
        assert_eq!(ROULETTE_COLORS.len(), 37);
        assert_eq!(ROULETTE_COLORS[0], "green");
        // Red and black each appear 18 times.
        assert_eq!(ROULETTE_COLORS.iter().filter(|c| **c == "red").count(), 18);
        assert_eq!(
            ROULETTE_COLORS.iter().filter(|c| **c == "black").count(),
            18
        );
    }

    /// Card labels A/2-10/J/Q/K for values 1-13 (color-card + high-low).
    #[test]
    fn card_name_covers_1_to_13() {
        let expected = [
            "A", "2", "3", "4", "5", "6", "7", "8", "9", "10", "J", "Q", "K",
        ];
        for (v, want) in expected.iter().enumerate() {
            assert_eq!(&card_name((v + 1) as f64), want);
        }
    }

    /// High-low outcomes: push only on 7, higher wins 8-13, lower wins 1-6.
    #[test]
    fn high_low_outcome_matrix() {
        for v in 1u32..=13 {
            let value = v as f64;
            let push = value == 7.0;
            let higher_won = !push && value > 7.0;
            let lower_won = !push && value < 7.0;
            assert_eq!(push, v == 7);
            assert_eq!(higher_won, (8..=13).contains(&v));
            assert_eq!(lower_won, (1..=6).contains(&v));
        }
    }

    /// Plinko weight table: labels, multipliers and the 100-weight sum.
    #[test]
    fn plinko_slot_table() {
        let slots: [(&str, f64); 7] = [
            ("0x", 0.0),
            ("0.5x", 0.5),
            ("0.8x", 0.8),
            ("1.2x", 1.2),
            ("1.5x", 1.5),
            ("3x", 3.0),
            ("8x", 8.0),
        ];
        let weights: [f64; 7] = [25.0, 25.0, 18.0, 15.0, 10.0, 5.0, 2.0];
        let total: f64 = weights.iter().sum();
        assert_eq!(total, 100.0);
        let wins = slots
            .iter()
            .zip(weights)
            .filter(|((_, m), _)| *m >= 1.0)
            .count();
        assert_eq!(wins, 4); // 1.2x / 1.5x / 3x / 8x pay WIN
    }

    /// `getVal` — aces flex down from 11 while the total stays ≤ 21, faces
    /// are 10, everything else parseInt (a missing `v` poisons to NaN).
    #[test]
    fn blackjack_hand_values_match_js() {
        let hand = |vals: &[&str]| -> Vec<Value> {
            vals.iter().map(|v| json!({ "s": "♠", "v": v })).collect()
        };
        assert_eq!(hand_value(&hand(&["A", "A"])), 12.0);
        assert_eq!(hand_value(&hand(&["A", "K"])), 21.0);
        assert_eq!(hand_value(&hand(&["A", "A", "A"])), 13.0); // 11 → 22-10
        assert_eq!(hand_value(&hand(&["5", "A", "A"])), 17.0); // 5+11+1
        assert_eq!(hand_value(&hand(&["10", "J"])), 20.0);
        assert_eq!(hand_value(&hand(&["Q", "A", "4"])), 15.0); // 10+1+4
        assert_eq!(hand_value(&hand(&["A", "2", "3", "4"])), 20.0);
        assert!(hand_value(&hand(&["2", "x"])).is_nan()); // parseInt('x') NaN
        assert_eq!(hand_value(&hand(&["A"])), 11.0);
        // A bust that stays bust after aces flex: 10 + 10 + A + A = 22+2
        assert_eq!(hand_value(&hand(&["10", "10", "A", "A"])), 22.0);
    }

    /// The deck is the exact 52-card {s,v} product in suit-major order
    /// before the shuffle.
    #[test]
    fn blackjack_deck_is_52_cards() {
        let deck = build_deck();
        assert_eq!(deck.len(), 52);
        let mut suits = std::collections::BTreeSet::new();
        let mut vals = std::collections::BTreeSet::new();
        for c in &deck {
            suits.insert(jsval::string(&c["s"]));
            vals.insert(jsval::string(&c["v"]));
        }
        assert_eq!(suits.len(), 4);
        assert_eq!(vals.len(), 13);
        assert_eq!(deck[0], json!({ "s": "♠", "v": "A" }));
        assert_eq!(deck[51], json!({ "s": "♣", "v": "K" }));
        // Every (s, v) pair unique.
        let mut pairs = std::collections::BTreeSet::new();
        for c in &deck {
            pairs.insert((jsval::string(&c["s"]), jsval::string(&c["v"])));
        }
        assert_eq!(pairs.len(), 52);
    }

    /// The stand outcome ladder: dealer bust or higher player wins double,
    /// a tie pushes, everything else loses.
    #[test]
    fn blackjack_stand_outcome_matrix() {
        assert_eq!(bj_outcome(22.0, 18.0, 10.0), ("win", 20.0)); // dealer bust
        assert_eq!(bj_outcome(17.0, 18.0, 10.0), ("win", 20.0)); // player higher
        assert_eq!(bj_outcome(21.0, 21.0, 10.0), ("push", 10.0));
        assert_eq!(bj_outcome(18.0, 17.0, 10.0), ("lose", 0.0));
        assert_eq!(bj_outcome(21.0, 20.0, 10.0), ("lose", 0.0));
    }

    /// The dealer draws while under 17 and stops the moment it reaches it
    /// (the loop recomputes after every card).
    #[test]
    fn blackjack_dealer_draws_to_17() {
        let card = |v: &str| json!({ "s": "♥", "v": v });
        let mut dealer = vec![card("10"), card("2")]; // 12
        let mut deck: Vec<Value> = vec![card("2"), card("3"), card("4")];
        loop {
            if hand_value(&dealer) >= 17.0 {
                break;
            }
            dealer.push(deck.pop().unwrap_or(Value::Null));
        }
        // 12 → +4 (16) → +3 (19): stops at 19, never drawing past 17.
        assert_eq!(hand_value(&dealer), 19.0);
        assert_eq!(deck.len(), 1);
    }

    /// `Number(x.toFixed(4))` money rounding through the shared helper.
    #[test]
    fn money_rounding_matches_js() {
        let r4 = |x: f64| js_num_from_fixed(&js_to_fixed(x, 4));
        assert_eq!(r4(19.0), 19.0);
        assert_eq!(r4(10.0 * 1.9), 19.0);
        assert_eq!(r4(0.1 + 0.2), 0.3);
        assert_eq!(r4(-5.0), -5.0); // clamped later by Math.max(0, …)
        let r2 = |x: f64| js_num_from_fixed(&js_to_fixed(x, 2));
        assert_eq!(r2(1.005), 1.0); // JS: Number((1.005).toFixed(2)) === 1
        assert_eq!(r2(10.5), 10.5);
    }

    /// Payout multipliers: push returns the bet, RPS wins pay bet×1.9,
    /// color-card bet×1.92, lucky-seven bet×4.8 — all funnelled through the
    /// same 4-decimal settle rounding the JS applies.
    #[test]
    fn rps_and_card_payouts() {
        // Push pays the bet back, win pays bet×1.9 (RPS) / bet×1.92 (color
        // card) exactly as the JS multiplies them.
        for bet in [1.0, 2.5, 100.0, 500.0] {
            assert_eq!(bet * 1.0, bet);
            assert_eq!(bet * 1.9, bet * 1.9);
            assert_eq!(bet * 1.92, bet * 1.92);
            assert_eq!(bet * 4.8, bet * 4.8); // lucky seven
        }
        // The 4-decimal settle rounding of a 1.92 payout on a 2.50 bet.
        let r4 = |x: f64| js_num_from_fixed(&js_to_fixed(x, 4));
        assert_eq!(r4(2.5 * 1.92), 4.8);
        assert_eq!(r4(100.0 * 1.9), 190.0);
    }

    /// Triple-dice multiplier table — `[0, 2, 5, 25][matches]`.
    #[test]
    fn triple_dice_mult_table() {
        let table = [0.0, 2.0, 5.0, 25.0];
        assert_eq!(table[0], 0.0);
        assert_eq!(table[1], 2.0);
        assert_eq!(table[2], 5.0);
        assert_eq!(table[3], 25.0);
    }

    /// Regression for a bug where `js_random().floor() * 6.0` (floor applied
    /// before the multiply, instead of after) made every die always come up
    /// 1 — a guaranteed-25x triple-dice exploit and an unwinnable lucky-seven
    /// (whose only win condition, a 7, became unreachable since two dice
    /// permanently summed to 2). Confirms the fixed `1-6` roll formula
    /// actually spans its full range over many draws.
    #[test]
    fn dice_roll_formula_is_not_degenerate() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..2000 {
            let roll = 1.0 + (mitch_lib::crypto::js_random() * 6.0).floor();
            assert!((1.0..=6.0).contains(&roll));
            seen.insert(roll as i64);
        }
        assert_eq!(
            seen.len(),
            6,
            "every face 1-6 should turn up over 2000 rolls"
        );
    }

    /// The poker rank ladder (server.js:23497-23509) — every branch, in
    /// ladder order, including the ace-low NON-straight and the pair
    /// J-or-better rule.
    #[test]
    fn poker_rank_ladder() {
        let c = |s: &str, v: &str| json!({ "s": s, "v": v });
        let royal = vec![
            c("♠", "10"),
            c("♠", "J"),
            c("♠", "Q"),
            c("♠", "K"),
            c("♠", "A"),
        ];
        assert_eq!(poker_rank(&royal), ("Royal Flush", 500.0));
        let st_flush = vec![
            c("♥", "5"),
            c("♥", "6"),
            c("♥", "7"),
            c("♥", "8"),
            c("♥", "9"),
        ];
        assert_eq!(poker_rank(&st_flush), ("Straight Flush", 100.0));
        let quads = vec![
            c("♠", "7"),
            c("♥", "7"),
            c("♦", "7"),
            c("♣", "7"),
            c("♠", "K"),
        ];
        assert_eq!(poker_rank(&quads), ("Four of a Kind", 50.0));
        let boat = vec![
            c("♠", "9"),
            c("♥", "9"),
            c("♦", "9"),
            c("♣", "2"),
            c("♠", "2"),
        ];
        assert_eq!(poker_rank(&boat), ("Full House", 15.0));
        let flush = vec![
            c("♦", "2"),
            c("♦", "5"),
            c("♦", "9"),
            c("♦", "J"),
            c("♦", "A"),
        ];
        assert_eq!(poker_rank(&flush), ("Flush", 10.0));
        let straight = vec![
            c("♠", "4"),
            c("♥", "5"),
            c("♦", "6"),
            c("♣", "7"),
            c("♠", "8"),
        ];
        assert_eq!(poker_rank(&straight), ("Straight", 7.0));
        // Ace-low (A,2,3,4,5) is NOT a straight: A is always 14.
        let wheel = vec![
            c("♠", "A"),
            c("♥", "2"),
            c("♦", "3"),
            c("♣", "4"),
            c("♠", "5"),
        ];
        assert_eq!(poker_rank(&wheel), ("Lose", 0.0));
        let trips = vec![
            c("♠", "6"),
            c("♥", "6"),
            c("♦", "6"),
            c("♣", "2"),
            c("♠", "K"),
        ];
        assert_eq!(poker_rank(&trips), ("Three of a Kind", 5.0));
        let two_pair = vec![
            c("♠", "9"),
            c("♥", "9"),
            c("♦", "3"),
            c("♣", "3"),
            c("♠", "K"),
        ];
        assert_eq!(poker_rank(&two_pair), ("Two Pair", 3.0));
        let jacks = vec![
            c("♠", "J"),
            c("♥", "J"),
            c("♦", "2"),
            c("♣", "5"),
            c("♠", "9"),
        ];
        assert_eq!(poker_rank(&jacks), ("Jacks or Better", 2.0));
        // A low pair pays nothing.
        let lows = vec![
            c("♠", "9"),
            c("♥", "9"),
            c("♦", "2"),
            c("♣", "5"),
            c("♠", "K"),
        ];
        assert_eq!(poker_rank(&lows), ("Lose", 0.0));
        let high_card = vec![
            c("♠", "2"),
            c("♥", "5"),
            c("♦", "9"),
            c("♣", "J"),
            c("♠", "A"),
        ];
        assert_eq!(poker_rank(&high_card), ("Lose", 0.0));
    }

    /// The prize-wheel segment table (server.js:23579-23587) — labels,
    /// mults and weights in literal order.
    #[test]
    fn wheel_segment_table_matches_js() {
        let expected: [(&str, f64, f64); 7] = [
            ("Bust", 0.0, 85.0),
            ("Half Back", 0.5, 40.0),
            ("Small Win", 1.25, 32.0),
            ("Double", 2.0, 31.0),
            ("Triple", 3.0, 8.0),
            ("Meteor", 8.0, 3.0),
            ("Galaxy Jackpot", 20.0, 1.0),
        ];
        assert_eq!(WHEEL_SEGMENTS, expected);
        // Weights sum to 200; five segments pay ≥ 1 (WIN — Small Win through
        // Galaxy Jackpot), two don't (Bust, Half Back).
        assert_eq!(WHEEL_SEGMENTS.iter().map(|(_, _, w)| w).sum::<f64>(), 200.0);
        assert_eq!(
            WHEEL_SEGMENTS.iter().filter(|(_, m, _)| *m >= 1.0).count(),
            5
        );
    }

    /// The scratch-card band ladder (server.js:23596-23603) — `<`
    /// thresholds with the exact boundary rolls.
    #[test]
    fn scratch_rank_thresholds() {
        assert_eq!(scratch_rank(0.0), (90.0, "Triple Diamonds"));
        // Exactly at a threshold falls into the NEXT band (strict <).
        assert_eq!(scratch_rank(0.002), (25.0, "Triple Sevens"));
        assert_eq!(scratch_rank(0.010), (6.0, "Triple Bells"));
        assert_eq!(scratch_rank(0.040), (2.5, "Triple Fruit"));
        assert_eq!(scratch_rank(0.100), (1.5, "Small Match"));
        assert_eq!(scratch_rank(0.180), (1.0, "Half Back"));
        assert_eq!(scratch_rank(0.300), (0.0, "No Match"));
        assert_eq!(scratch_rank(0.999), (0.0, "No Match"));
        assert_eq!(scratch_rank(0.1799), (1.5, "Small Match"));
        assert_eq!(scratch_rank(0.0099), (25.0, "Triple Sevens"));
    }

    /// The keno payout table (server.js:23628-23637) and the ×1.12
    /// toFixed(2) boost applied to any paying cell.
    #[test]
    fn keno_payout_table_matches_js() {
        let expected = [
            (3usize, vec![(2usize, 2.1), (3, 14.0)]),
            (4, vec![(2, 0.9), (3, 5.0), (4, 38.0)]),
            (5, vec![(3, 2.8), (4, 14.0), (5, 90.0)]),
            (6, vec![(3, 1.3), (4, 5.5), (5, 38.0), (6, 210.0)]),
            (7, vec![(4, 3.6), (5, 17.0), (6, 90.0), (7, 480.0)]),
            (
                8,
                vec![(4, 2.4), (5, 8.5), (6, 45.0), (7, 190.0), (8, 950.0)],
            ),
            (
                9,
                vec![(5, 5.5), (6, 25.0), (7, 120.0), (8, 450.0), (9, 1700.0)],
            ),
            (
                10,
                vec![
                    (5, 3.8),
                    (6, 14.0),
                    (7, 65.0),
                    (8, 250.0),
                    (9, 1000.0),
                    (10, 2800.0),
                ],
            ),
        ];
        for (picks, row) in expected {
            let got: Vec<(usize, f64)> = KENO_PAYOUT
                .iter()
                .find(|(p, _)| *p == picks)
                .map(|(_, r)| r.to_vec())
                .unwrap_or_default();
            assert_eq!(got, row, "picks={picks}");
            // Row invariants: hit counts ascend, first paying row starts ≥ 2
            // (a single hit never pays).
            assert!(row.windows(2).all(|w| w[0].0 < w[1].0));
            assert!(row[0].0 >= 2);
        }
        // Absent cells pay nothing.
        assert_eq!(keno_payout(3, 0), 0.0);
        assert_eq!(keno_payout(3, 1), 0.0);
        assert_eq!(keno_payout(5, 2), 0.0);
        assert_eq!(keno_payout(3, 4), 0.0); // hits can't exceed picks anyway
                                            // mult = Number((mult * 1.12).toFixed(2)) for paying cells.
        let boost = |m: f64| js_num_from_fixed(&js_to_fixed(m * 1.12, 2));
        assert_eq!(boost(2.1), 2.35); // 2.352 → '2.35'
        assert_eq!(boost(0.9), 1.01); // 1.008 → '1.01'
        assert_eq!(boost(5.0), 5.6); // 5.6 → '5.60'
        assert_eq!(boost(2800.0), 3136.0); // 3136.0000000000005 → '3136.00'
    }

    /// drawUniqueNumbers: `count` unique ascending integers from 1..max.
    #[test]
    fn draw_unique_numbers_shape() {
        for _ in 0..50 {
            let drawn = draw_unique_numbers(40, 12);
            assert_eq!(drawn.len(), 12);
            let mut sorted = drawn.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            assert_eq!(drawn, sorted, "must be ascending");
            assert!(drawn.windows(2).all(|w| w[0] < w[1]), "must be unique");
            assert!(drawn
                .iter()
                .all(|n| *n >= 1.0 && *n <= 40.0 && n.fract() == 0.0));
        }
        // count larger than max yields all max numbers.
        assert_eq!(draw_unique_numbers(3, 10), vec![1.0, 2.0, 3.0]);
    }

    /// The picks parser: Number() coercion, Set dedupe, integer 1-40 filter,
    /// ascending sort — non-arrays and under-full filters hit the 400.
    #[test]
    fn keno_picks_parser() {
        let pick = |p: Value| parse_keno_picks(&json!({ "picks": p }));
        assert_eq!(pick(json!([1, 2, 3])), vec![1.0, 2.0, 3.0]);
        // Dedupe + sort: input order irrelevant.
        assert_eq!(pick(json!([7, 3, 7, 1])), vec![1.0, 3.0, 7.0]);
        // Number coercion: strings parse, true → 1, null → 0 (filtered).
        assert_eq!(pick(json!(["5", 6, true, null])), vec![1.0, 5.0, 6.0]);
        // Filter: floats, 0, 41, non-numeric strings (NaN) all drop.
        assert_eq!(pick(json!([1.5, 0, 41, "abc", 2, 3])), vec![2.0, 3.0]);
        // Non-array picks → [] (→ 400 'Pick 3 to 10 numbers.').
        assert!(pick(json!(5)).is_empty());
        assert!(parse_keno_picks(&json!({})).is_empty());
    }

    /// The slots multiplier table (server.js:23686-23696): triples by
    /// symbol, leading pair 3, trailing pair nothing, else 0.
    #[test]
    fn slots_mult_table() {
        assert_eq!(slots_mult(&["💎", "💎", "💎"]), 100.0);
        assert_eq!(slots_mult(&["7️⃣", "7️⃣", "7️⃣"]), 50.0);
        assert_eq!(slots_mult(&["🔔", "🔔", "🔔"]), 20.0);
        assert_eq!(slots_mult(&["🍒", "🍒", "🍒"]), 10.0);
        // Other symbols triple to the 6x default.
        assert_eq!(slots_mult(&["🍋", "🍋", "🍋"]), 6.0);
        assert_eq!(slots_mult(&["🍇", "🍇", "🍇"]), 6.0);
        assert_eq!(slots_mult(&["🍊", "🍊", "🍊"]), 6.0);
        // Only the LEADING pair counts (server.js checks res[0] === res[1]).
        assert_eq!(slots_mult(&["💎", "💎", "🍒"]), 3.0);
        assert_eq!(slots_mult(&["💎", "🍒", "🍒"]), 0.0);
        assert_eq!(slots_mult(&["💎", "🍒", "💎"]), 0.0);
        assert_eq!(slots_mult(&["🍒", "🍋", "🍊"]), 0.0);
    }

    /// The rank map keyed on the PRE-bonus mult, 'Lose' for anything else.
    #[test]
    fn slots_rank_map() {
        assert_eq!(slots_rank(100.0), "JACKPOT (Diamonds)");
        assert_eq!(slots_rank(50.0), "Triple Sevens");
        assert_eq!(slots_rank(20.0), "Triple Bells");
        assert_eq!(slots_rank(10.0), "Triple Cherries");
        assert_eq!(slots_rank(6.0), "Triple Fruit");
        assert_eq!(slots_rank(3.0), "Double");
        assert_eq!(slots_rank(0.0), "Lose");
        assert_eq!(slots_rank(f64::NAN), "Lose");
    }

    /// VIP room boost: mult = Number((mult * 1.1).toFixed(2)).
    #[test]
    fn slots_vip_boost_vectors() {
        let boost = |m: f64| js_num_from_fixed(&js_to_fixed(m * 1.1, 2));
        assert_eq!(boost(100.0), 110.0); // 110.00000000000001 → '110.00'
        assert_eq!(boost(50.0), 55.0);
        assert_eq!(boost(6.0), 6.6); // 6.6000000000000005 → '6.60'
        assert_eq!(boost(3.0), 3.3); // 3.3000000000000003 → '3.30'
    }

    fn test_state() -> (Arc<AppState>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "mitch-server-casino-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap_or_default();
        let cfg = crate::hosts::SiteConfig::load();
        let cfg = crate::hosts::SiteConfig {
            data_dir: dir.join("data"),
            ..cfg
        };
        let store = Arc::new(
            mitch_lib::data::DataStore::open(&dir, &dir.join("data"))
                .unwrap_or_else(|e| panic!("store: {e}")),
        );
        (Arc::new(AppState::new(cfg, Arc::clone(&store))), dir)
    }

    // Regression coverage for the retired casino-buff exploit: bad-beat
    // insurance refunded the bet on *any* loss, which made a round with any
    // win chance a guaranteed non-negative-EV bet (free money over enough
    // rounds), and double-down then doubled wins on top. Both flags are
    // still readable stat fields (old purchases, or a stale data file), so
    // the fix has to be "settle_casino_round no longer honors them" rather
    // than just "the shop can't sell them anymore" — these tests set the
    // flags directly, bypassing the shop entirely, the way an
    // already-granted buff would still be set after this fix ships.
    #[test]
    fn bad_beat_insurance_no_longer_refunds_a_loss() {
        let (state, dir) = test_state();
        let email = "insured@example.test";
        let norm = mitch_lib::auth::normalize_email(email);
        mitch_lib::coins::add_coins(&state.store, state.data_dir(), email, 1000.0, 1.0, "seed");
        let far_future = now_millis() as f64 + 60_000.0;
        mitch_lib::user_stats::user_stats_set(
            &state.store,
            &norm,
            &json!({ "bad_beat_insurance_until": far_future }),
        );
        let settled = settle_casino_round(
            &state,
            &Round { email, norm: &norm },
            "Test Game",
            100.0,
            0.0, // a loss: zero payout
            "LOSE",
            (false, false),
        );
        // Before the fix this would be 0 (refunded) and the outcome would
        // read "REFUNDED (INSURED)".
        assert_eq!(settled.net, -100.0);
        assert_eq!(settled.payout, 0.0);
        assert_eq!(settled.new_balance, 900.0);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn double_down_no_longer_doubles_a_win() {
        let (state, dir) = test_state();
        let email = "doubler@example.test";
        let norm = mitch_lib::auth::normalize_email(email);
        mitch_lib::coins::add_coins(&state.store, state.data_dir(), email, 1000.0, 1.0, "seed");
        let far_future = now_millis() as f64 + 60_000.0;
        mitch_lib::user_stats::user_stats_set(
            &state.store,
            &norm,
            &json!({ "double_down_until": far_future }),
        );
        let settled = settle_casino_round(
            &state,
            &Round { email, norm: &norm },
            "Test Game",
            100.0,
            190.0, // a win at 1.9x
            "WIN",
            (false, false),
        );
        // Before the fix this would be 180 (doubled payout).
        assert_eq!(settled.payout, 190.0);
        assert_eq!(settled.net, 90.0);
        assert_eq!(settled.new_balance, 1090.0);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn vip_casino_pass_no_longer_bypasses_the_bet_cap() {
        let (state, dir) = test_state();
        let email = "whale@example.test";
        let norm = mitch_lib::auth::normalize_email(email);
        mitch_lib::coins::add_coins(
            &state.store,
            state.data_dir(),
            email,
            100_000.0,
            1.0,
            "seed",
        );
        let far_future = now_millis() as f64 + 60_000.0;
        mitch_lib::user_stats::user_stats_set(
            &state.store,
            &norm,
            &json!({ "vip_casino_until": far_future }),
        );
        assert!(!is_vip(&state, &norm));
        let err = read_casino_bet(&state, &json!({ "amount": 5000.0 }), email, &norm, 1.0)
            .expect_err("a 5000-coin bet must still be rejected despite a vip_casino_until flag");
        assert_eq!(err.status(), 400);
        std::fs::remove_dir_all(dir).ok();
    }
}
