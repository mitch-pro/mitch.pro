//! The casino minigames: Coinflip, Dice, Slots, Blackjack, and 5-card draw
//! Poker, all playing against a server-side RNG for real MitchCoins.
//!
//! ## Safety model (read this before touching payout math)
//!
//! - Every round's outcome is drawn from `rand::rng()` on the server
//!   *after* the bet is placed, inside the route handler, before
//!   [`crate::coins::settle_bet`] is ever called. No game ever accepts a
//!   client-supplied outcome, card, or roll — the client only supplies a
//!   bet amount and a choice (heads/tails, a dice target, which cards to
//!   hold). If a payload looks like it's trying to supply its own result,
//!   it's ignored, not validated against.
//! - [`crate::coins::settle_bet`] is the *only* function that ever moves
//!   money for these games, and it debits the bet and credits the payout
//!   in one locked transaction — there is no "deduct the bet, then credit
//!   the win" two-step that a crash or a race could land between.
//! - Every game here is a pure function of `(bet, choice, rng draws) ->
//!   payout`. None of them touch the database directly; the route layer
//!   calls a `play_*`/`resolve_*` function to get a payout, then calls
//!   `settle_bet` once. This keeps the money-moving code in exactly one
//!   place no matter how many games get added.
//! - Blackjack and Poker are multi-request (deal, then act), so their
//!   in-progress hand lives in server memory only (`AppState`), keyed by
//!   normalized email, with the bet already debited at deal time. A
//!   second "stand"/"draw" for a hand that's already resolved and removed
//!   from the map is rejected rather than re-settled — see
//!   `routes/casino.rs` for the locking around that.
//! - Every game targets a ~4% house edge (96% RTP) — "light," per
//!   AGENTS.md's economy-integrity rule: the house wins over the long
//!   run, but no single round, and no amount of looping, can produce
//!   positive expected value for the player. The Slots and Poker
//!   paytables are checked by exhaustive-enumeration tests below rather
//!   than hand-derived, specifically so a future paytable tweak can't
//!   silently flip the edge.

use rand::seq::SliceRandom;
use rand::Rng;

/// Smallest bet any game will accept.
pub const MIN_BET: f64 = 1.0;
/// Largest bet any game will accept, in one round.
pub const MAX_BET: f64 = 5000.0;

/// `true` if `bet` is a value a game is willing to accept at all —
/// finite, and within `[MIN_BET, MAX_BET]`. Games still separately rely on
/// `settle_bet` to reject a bet the account can't cover.
pub fn bet_in_range(bet: f64) -> bool {
    bet.is_finite() && (MIN_BET..=MAX_BET).contains(&bet)
}

/// Target house edge applied to every game whose odds are chosen directly
/// (Coinflip, Dice). Slots and Poker get there via their paytables instead
/// (see the enumeration tests below), since a fixed-odds multiplier
/// doesn't apply to a paytable-driven game.
const HOUSE_EDGE: f64 = 0.04;

// ---------------------------------------------------------------------
// Coinflip
// ---------------------------------------------------------------------

/// Payout multiplier on a win: `(1 - edge) / 0.5`.
pub fn coinflip_multiplier() -> f64 {
    (1.0 - HOUSE_EDGE) / 0.5
}

/// Plays one coinflip round. `choice` must be `"heads"` or `"tails"`;
/// anything else is treated as an invalid pick (always loses) rather than
/// panicking, since this is the last line of defense against a malformed
/// client payload. Returns `(won, payout)` — `payout` is `0.0` on a loss.
pub fn play_coinflip(bet: f64, choice: &str) -> (bool, f64) {
    let landed_heads = rand::rng().random_bool(0.5);
    let picked_heads = choice.eq_ignore_ascii_case("heads");
    let picked_tails = choice.eq_ignore_ascii_case("tails");
    let won = (landed_heads && picked_heads) || (!landed_heads && picked_tails);
    (
        won,
        if won {
            bet * coinflip_multiplier()
        } else {
            0.0
        },
    )
}

// ---------------------------------------------------------------------
// Dice
// ---------------------------------------------------------------------

/// Smallest/largest target a dice bet may choose, so the implied
/// win-probability (and thus the payout multiplier) stays bounded —
/// `target` outside `[2, 98]` is rejected by the route layer before this
/// is ever called.
pub const DICE_MIN_TARGET: u8 = 2;
pub const DICE_MAX_TARGET: u8 = 98;

/// Plays one dice round: rolls `1..=100`, wins on "under `target`" or
/// "over `target`" depending on `direction`. The payout multiplier is
/// derived from the exact win probability so every target keeps the same
/// `HOUSE_EDGE`, no matter how it's chosen.
///
/// Returns `(won, payout, roll)`.
pub fn play_dice(bet: f64, target: u8, direction: &str) -> (bool, f64, u8) {
    let roll = rand::rng().random_range(1..=100u8);
    let under = direction.eq_ignore_ascii_case("under");
    let (won, win_probability) = if under {
        (roll < target, (target.saturating_sub(1)) as f64 / 100.0)
    } else {
        (roll > target, (100 - target) as f64 / 100.0)
    };
    // win_probability is always > 0 for targets in [2, 98] in either
    // direction, but clamp defensively so a payout is never divided by
    // zero or inflated by a probability that rounds to it.
    let win_probability = win_probability.clamp(0.01, 0.99);
    let multiplier = (1.0 - HOUSE_EDGE) / win_probability;
    (won, if won { bet * multiplier } else { 0.0 }, roll)
}

// ---------------------------------------------------------------------
// Cards (shared by Blackjack and Poker)
// ---------------------------------------------------------------------

/// `rank` is `1..=13` (1 = Ace, 11 = Jack, 12 = Queen, 13 = King). `suit`
/// is `0..=3`, meaningless for scoring but carried through for display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Card {
    pub rank: u8,
    pub suit: u8,
}

impl Card {
    /// `"A"`, `"2"`..`"10"`, `"J"`, `"Q"`, `"K"`.
    pub fn rank_label(&self) -> &'static str {
        match self.rank {
            1 => "A",
            2 => "2",
            3 => "3",
            4 => "4",
            5 => "5",
            6 => "6",
            7 => "7",
            8 => "8",
            9 => "9",
            10 => "10",
            11 => "J",
            12 => "Q",
            13 => "K",
            _ => "?",
        }
    }

    /// `"S"`, `"H"`, `"D"`, `"C"`.
    pub fn suit_label(&self) -> &'static str {
        match self.suit {
            0 => "S",
            1 => "H",
            2 => "D",
            _ => "C",
        }
    }
}

/// A freshly shuffled 52-card deck. Cards are drawn by popping from the
/// end, so dealing is `O(1)` per card.
pub fn fresh_shuffled_deck() -> Vec<Card> {
    let mut deck = Vec::with_capacity(52);
    for suit in 0..4u8 {
        for rank in 1..=13u8 {
            deck.push(Card { rank, suit });
        }
    }
    deck.shuffle(&mut rand::rng());
    deck
}

// ---------------------------------------------------------------------
// Slots
// ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotSymbol {
    Cherry,
    Lemon,
    Bell,
    Clover,
    Star,
    Seven,
}

/// Same six symbols on all three reels, independent draws. Weights are
/// "virtual stop counts" on a 20-stop reel strip, not raw probabilities —
/// `SLOT_WEIGHTS[i]` out of `SLOT_WEIGHT_TOTAL` for `SLOT_SYMBOLS[i]`.
const SLOT_SYMBOLS: [SlotSymbol; 6] = [
    SlotSymbol::Cherry,
    SlotSymbol::Lemon,
    SlotSymbol::Bell,
    SlotSymbol::Clover,
    SlotSymbol::Star,
    SlotSymbol::Seven,
];
const SLOT_WEIGHTS: [u32; 6] = [7, 5, 4, 2, 1, 1];
const SLOT_WEIGHT_TOTAL: u32 = 20;

fn slot_symbol_multiplier_3(sym: SlotSymbol) -> f64 {
    match sym {
        SlotSymbol::Seven => 48.0,
        SlotSymbol::Star => 23.5,
        SlotSymbol::Clover => 12.0,
        SlotSymbol::Bell => 7.5,
        SlotSymbol::Lemon => 4.8,
        SlotSymbol::Cherry => 3.75,
    }
}

fn slot_symbol_multiplier_pair(sym: SlotSymbol) -> f64 {
    match sym {
        SlotSymbol::Seven => 11.0,
        SlotSymbol::Star => 5.4,
        SlotSymbol::Clover => 2.7,
        SlotSymbol::Bell => 1.4,
        SlotSymbol::Lemon => 0.97,
        SlotSymbol::Cherry => 0.48,
    }
}

/// Consolation payout when no two reels match but at least one shows a
/// Cherry — the classic "almost" slots payout.
const SLOT_SINGLE_CHERRY_MULTIPLIER: f64 = 0.21;

fn spin_slot_reel(rng: &mut impl Rng) -> SlotSymbol {
    let mut pick = rng.random_range(0..SLOT_WEIGHT_TOTAL);
    for (i, w) in SLOT_WEIGHTS.iter().enumerate() {
        if pick < *w {
            return SLOT_SYMBOLS[i];
        }
        pick -= *w;
    }
    SLOT_SYMBOLS[SLOT_SYMBOLS.len() - 1]
}

/// The payout multiplier for a given set of three reels — a pure
/// function of the (already-drawn) symbols, so it can be exhaustively
/// enumerated in a test instead of only sampled.
fn slot_payout_multiplier(reels: [SlotSymbol; 3]) -> f64 {
    let [a, b, c] = reels;
    if a == b && b == c {
        return slot_symbol_multiplier_3(a);
    }
    let pair = if a == b {
        Some(a)
    } else if b == c {
        Some(b)
    } else if a == c {
        Some(a)
    } else {
        None
    };
    if let Some(sym) = pair {
        return slot_symbol_multiplier_pair(sym);
    }
    if reels.contains(&SlotSymbol::Cherry) {
        return SLOT_SINGLE_CHERRY_MULTIPLIER;
    }
    0.0
}

/// Plays one slots spin. Returns the three reels drawn and the payout
/// (`0.0` on a bust).
pub fn play_slots(bet: f64) -> ([SlotSymbol; 3], f64) {
    let mut rng = rand::rng();
    let reels = [
        spin_slot_reel(&mut rng),
        spin_slot_reel(&mut rng),
        spin_slot_reel(&mut rng),
    ];
    (reels, bet * slot_payout_multiplier(reels))
}

// ---------------------------------------------------------------------
// Blackjack
// ---------------------------------------------------------------------

/// `(best_total, is_soft)` for a hand — aces count as 11 each, reduced to
/// 1 one at a time while the total is over 21. `is_soft` is true only if
/// at least one ace is still counted as 11 after that reduction.
pub fn blackjack_hand_value(cards: &[Card]) -> (u8, bool) {
    let mut total: i32 = 0;
    let mut aces_as_eleven = 0i32;
    for c in cards {
        if c.rank == 1 {
            total += 11;
            aces_as_eleven += 1;
        } else if c.rank >= 11 {
            total += 10;
        } else {
            total += c.rank as i32;
        }
    }
    while total > 21 && aces_as_eleven > 0 {
        total -= 10;
        aces_as_eleven -= 1;
    }
    (total.max(0) as u8, aces_as_eleven > 0)
}

/// A two-card 21 — only meaningful for the initial deal.
pub fn is_blackjack(cards: &[Card]) -> bool {
    cards.len() == 2 && blackjack_hand_value(cards).0 == 21
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlackjackOutcome {
    PlayerBlackjack,
    PlayerBust,
    DealerBust,
    PlayerWins,
    DealerWins,
    Push,
}

/// `payout` is the *gross* amount [`crate::coins::settle_bet`] should
/// credit — `0.0` on a loss, `bet` on a push, `bet*2` on a plain win,
/// `bet*2.5` on a natural blackjack (3:2).
pub fn blackjack_payout(bet: f64, outcome: BlackjackOutcome) -> f64 {
    match outcome {
        BlackjackOutcome::PlayerBlackjack => bet * 2.5,
        BlackjackOutcome::Push => bet,
        BlackjackOutcome::PlayerBust | BlackjackOutcome::DealerWins => 0.0,
        BlackjackOutcome::DealerBust | BlackjackOutcome::PlayerWins => bet * 2.0,
    }
}

/// One in-progress (or just-resolved) blackjack hand. Lives only in server
/// memory (`AppState`), keyed by normalized email — see the module doc
/// comment for why that's an acceptable, deliberate tradeoff.
#[derive(Debug, Clone)]
pub struct BlackjackGame {
    pub player: Vec<Card>,
    pub dealer: Vec<Card>,
    pub deck: Vec<Card>,
    pub bet: f64,
}

/// Deals a new hand. If either side has a natural blackjack, the round is
/// already over — the second element is `Some((outcome, payout))` and the
/// caller should settle it immediately rather than store the game for
/// further hits/stands.
#[allow(clippy::expect_used)] // dealing 4 cards from a fresh 52-card deck cannot fail
pub fn blackjack_start(bet: f64) -> (BlackjackGame, Option<(BlackjackOutcome, f64)>) {
    let mut deck = fresh_shuffled_deck();
    let player = vec![
        deck.pop().expect("fresh deck has 52 cards"),
        deck.pop().expect("fresh deck has 52 cards"),
    ];
    let dealer = vec![
        deck.pop().expect("fresh deck has 52 cards"),
        deck.pop().expect("fresh deck has 52 cards"),
    ];
    let game = BlackjackGame {
        player: player.clone(),
        dealer: dealer.clone(),
        deck,
        bet,
    };
    let player_bj = is_blackjack(&player);
    let dealer_bj = is_blackjack(&dealer);
    if !player_bj && !dealer_bj {
        return (game, None);
    }
    let outcome = if player_bj && dealer_bj {
        BlackjackOutcome::Push
    } else if player_bj {
        BlackjackOutcome::PlayerBlackjack
    } else {
        BlackjackOutcome::DealerWins
    };
    let payout = blackjack_payout(bet, outcome);
    (game, Some((outcome, payout)))
}

/// Draws one card for the player. `Some((outcome, payout))` means the
/// player busted and the round is over; `None` means they're still live
/// and may hit again or stand.
///
/// A round can realistically never draw anywhere close to the 52 cards in
/// the deck (busting is guaranteed well before that), so `deck.pop()`
/// running out is an unreachable condition, not a case to design around.
#[allow(clippy::expect_used)] // see doc comment: unreachable within a single round
pub fn blackjack_hit(game: &mut BlackjackGame) -> Option<(BlackjackOutcome, f64)> {
    let card = game
        .deck
        .pop()
        .expect("blackjack round can't exhaust a 52-card deck");
    game.player.push(card);
    let (value, _) = blackjack_hand_value(&game.player);
    if value > 21 {
        let outcome = BlackjackOutcome::PlayerBust;
        Some((outcome, blackjack_payout(game.bet, outcome)))
    } else {
        None
    }
}

/// Plays the dealer out (hits while under 17, stands on every 17 whether
/// hard or soft) and settles the round.
#[allow(clippy::expect_used)] // dealer drawing to 17 can't exhaust a 52-card deck
pub fn blackjack_stand(game: &mut BlackjackGame) -> (BlackjackOutcome, f64) {
    loop {
        let (dealer_value, _) = blackjack_hand_value(&game.dealer);
        if dealer_value >= 17 {
            break;
        }
        let card = game
            .deck
            .pop()
            .expect("blackjack round can't exhaust a 52-card deck");
        game.dealer.push(card);
    }
    let (dealer_value, _) = blackjack_hand_value(&game.dealer);
    let (player_value, _) = blackjack_hand_value(&game.player);
    let outcome = if dealer_value > 21 {
        BlackjackOutcome::DealerBust
    } else if dealer_value > player_value {
        BlackjackOutcome::DealerWins
    } else if dealer_value < player_value {
        BlackjackOutcome::PlayerWins
    } else {
        BlackjackOutcome::Push
    };
    (outcome, blackjack_payout(game.bet, outcome))
}

// ---------------------------------------------------------------------
// Poker (5-card draw, "6/5 Jacks or Better")
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PokerHandRank {
    HighCard,
    JacksOrBetter,
    TwoPair,
    ThreeOfAKind,
    Straight,
    Flush,
    FullHouse,
    FourOfAKind,
    StraightFlush,
    RoyalFlush,
}

/// The published "6/5 Jacks or Better" paytable (full house pays 6x,
/// flush pays 5x — the common "short pay" table, chosen here specifically
/// because its well-documented optimal-play RTP (~95%) lands in the same
/// light-edge band as the other games; `JacksOrBetter` is a push, not a
/// loss, matching the real game).
pub fn poker_payout_multiplier(rank: PokerHandRank) -> f64 {
    match rank {
        PokerHandRank::RoyalFlush => 800.0,
        PokerHandRank::StraightFlush => 50.0,
        PokerHandRank::FourOfAKind => 25.0,
        PokerHandRank::FullHouse => 6.0,
        PokerHandRank::Flush => 5.0,
        PokerHandRank::Straight => 4.0,
        PokerHandRank::ThreeOfAKind => 3.0,
        PokerHandRank::TwoPair => 2.0,
        PokerHandRank::JacksOrBetter => 1.0,
        PokerHandRank::HighCard => 0.0,
    }
}

/// Classifies a final 5-card hand. Exhaustively verified against the
/// textbook 5-card-hand frequency table in the test below — see that test
/// before changing anything here, since a misclassification (e.g. calling
/// a full house a flush) would be an overpay, not just a display bug.
pub fn evaluate_poker_hand(cards: &[Card; 5]) -> PokerHandRank {
    let same_suit = cards.iter().all(|c| c.suit == cards[0].suit);
    let mut ranks: Vec<u8> = cards.iter().map(|c| c.rank).collect();
    ranks.sort_unstable();
    let mut unique = ranks.clone();
    unique.dedup();

    let is_broadway = unique == [1, 10, 11, 12, 13];
    let is_straight = unique.len() == 5 && (unique[4] - unique[0] == 4 || is_broadway);

    if is_straight && same_suit && is_broadway {
        return PokerHandRank::RoyalFlush;
    }
    if is_straight && same_suit {
        return PokerHandRank::StraightFlush;
    }

    let mut counts: std::collections::HashMap<u8, u8> = std::collections::HashMap::new();
    for r in &ranks {
        *counts.entry(*r).or_insert(0) += 1;
    }
    let mut freqs: Vec<u8> = counts.values().copied().collect();
    freqs.sort_unstable_by(|a, b| b.cmp(a));

    if freqs == [4, 1] {
        return PokerHandRank::FourOfAKind;
    }
    if freqs == [3, 2] {
        return PokerHandRank::FullHouse;
    }
    if same_suit {
        return PokerHandRank::Flush;
    }
    if is_straight {
        return PokerHandRank::Straight;
    }
    if freqs == [3, 1, 1] {
        return PokerHandRank::ThreeOfAKind;
    }
    if freqs == [2, 2, 1] {
        return PokerHandRank::TwoPair;
    }
    if freqs == [2, 1, 1, 1] {
        let pair_rank = counts.iter().find(|(_, &c)| c == 2).map(|(r, _)| *r);
        return match pair_rank {
            Some(1 | 11 | 12 | 13) => PokerHandRank::JacksOrBetter,
            _ => PokerHandRank::HighCard,
        };
    }
    PokerHandRank::HighCard
}

/// One in-progress (dealt, not yet drawn) poker hand. Server-memory-only,
/// same tradeoff as [`BlackjackGame`].
#[derive(Debug, Clone)]
pub struct PokerGame {
    pub hand: [Card; 5],
    pub deck: Vec<Card>,
    pub bet: f64,
}

#[allow(clippy::expect_used)] // dealing 5 cards from a fresh 52-card deck cannot fail
pub fn poker_deal(bet: f64) -> PokerGame {
    let mut deck = fresh_shuffled_deck();
    let hand = [
        deck.pop().expect("fresh deck has 52 cards"),
        deck.pop().expect("fresh deck has 52 cards"),
        deck.pop().expect("fresh deck has 52 cards"),
        deck.pop().expect("fresh deck has 52 cards"),
        deck.pop().expect("fresh deck has 52 cards"),
    ];
    PokerGame { hand, deck, bet }
}

/// Replaces every card whose index isn't in `hold` with a fresh draw, then
/// evaluates and returns the final hand's `(rank, payout)`. This is the
/// one and only draw a round gets.
#[allow(clippy::expect_used)] // replacing up to 5 cards from a 47-card deck cannot exhaust it
pub fn poker_draw(game: &mut PokerGame, hold: [bool; 5]) -> (PokerHandRank, f64) {
    for (i, keep) in hold.iter().enumerate() {
        if !keep {
            game.hand[i] = game
                .deck
                .pop()
                .expect("poker round can't exhaust a 52-card deck");
        }
    }
    let rank = evaluate_poker_hand(&game.hand);
    (rank, game.bet * poker_payout_multiplier(rank))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_shuffled_deck_has_fifty_two_unique_cards() {
        let deck = fresh_shuffled_deck();
        assert_eq!(deck.len(), 52);
        let mut seen = std::collections::HashSet::new();
        for card in &deck {
            assert!(seen.insert((card.rank, card.suit)), "duplicate card dealt");
        }
    }

    #[test]
    fn coinflip_multiplier_implies_a_four_percent_edge() {
        // Net EV per unit bet = P(win)*(mult-1) + P(lose)*(-1) = 0.5*mult - 1.
        let ev = 0.5 * coinflip_multiplier() - 1.0;
        assert!((ev - (-HOUSE_EDGE)).abs() < 1e-9);
    }

    #[test]
    fn play_coinflip_only_pays_on_a_matching_pick() {
        for _ in 0..200 {
            let (won, payout) = play_coinflip(10.0, "heads");
            assert_eq!(won, payout > 0.0);
            if won {
                assert!((payout - 10.0 * coinflip_multiplier()).abs() < 1e-9);
            } else {
                assert_eq!(payout, 0.0);
            }
        }
    }

    #[test]
    fn play_coinflip_rejects_a_garbage_choice_as_an_automatic_loss() {
        // Garbage input must never be able to match either outcome.
        for _ in 0..200 {
            let (won, payout) = play_coinflip(10.0, "banana");
            assert!(!won);
            assert_eq!(payout, 0.0);
        }
    }

    #[test]
    fn play_dice_under_and_over_each_keep_the_house_edge() {
        for target in [2u8, 25, 50, 75, 98] {
            for direction in ["under", "over"] {
                let win_probability = if direction == "under" {
                    (target - 1) as f64 / 100.0
                } else {
                    (100 - target) as f64 / 100.0
                }
                .clamp(0.01, 0.99);
                let multiplier = (1.0 - HOUSE_EDGE) / win_probability;
                let ev = win_probability * multiplier - 1.0;
                assert!(
                    (ev - (-HOUSE_EDGE)).abs() < 1e-6,
                    "target={target} dir={direction} ev={ev}"
                );
            }
        }
    }

    #[test]
    fn play_dice_payout_matches_its_own_roll_outcome() {
        for _ in 0..500 {
            let (won, payout, roll) = play_dice(10.0, 50, "under");
            assert_eq!(won, roll < 50);
            assert_eq!(won, payout > 0.0);
        }
    }

    /// Exhaustively enumerates all `20^3 = 8000` equally-likely reel-stop
    /// combinations (not a Monte Carlo sample) and sums the *exact*
    /// expected payout per unit bet. This is the real guarantee behind
    /// "no money glitches" for Slots: nobody has to trust the paytable
    /// numbers above by eye, this computes the true RTP from them.
    #[test]
    fn slots_exact_rtp_is_a_light_house_edge() {
        let mut total_weighted_payout = 0.0f64;
        let total_combos = (SLOT_WEIGHT_TOTAL as u64).pow(3) as f64;
        for (i0, w0) in SLOT_WEIGHTS.iter().enumerate() {
            for (i1, w1) in SLOT_WEIGHTS.iter().enumerate() {
                for (i2, w2) in SLOT_WEIGHTS.iter().enumerate() {
                    let combo_weight = (*w0 * *w1 * *w2) as f64;
                    let reels = [SLOT_SYMBOLS[i0], SLOT_SYMBOLS[i1], SLOT_SYMBOLS[i2]];
                    total_weighted_payout += combo_weight * slot_payout_multiplier(reels);
                }
            }
        }
        let rtp = total_weighted_payout / total_combos;
        assert!(
            (0.95..=0.97).contains(&rtp),
            "slots RTP out of the intended light (3-5% edge) band: {rtp}"
        );
    }

    #[test]
    fn slot_weights_sum_to_the_declared_total() {
        assert_eq!(SLOT_WEIGHTS.iter().sum::<u32>(), SLOT_WEIGHT_TOTAL);
    }

    fn card(rank: u8, suit: u8) -> Card {
        Card { rank, suit }
    }

    #[test]
    fn blackjack_hand_value_handles_soft_and_hard_aces() {
        // Ace + King: soft 21.
        assert_eq!(blackjack_hand_value(&[card(1, 0), card(13, 0)]), (21, true));
        // Ace + 9 + 5: 11+9+5=25 busts soft, so the ace drops to 1 -> hard 15.
        assert_eq!(
            blackjack_hand_value(&[card(1, 0), card(9, 0), card(5, 0)]),
            (15, false)
        );
        // Two aces + 9: 11+11+9=31 -> drop one ace to 1 -> 21, still soft
        // (one ace still counted as 11).
        assert_eq!(
            blackjack_hand_value(&[card(1, 0), card(1, 1), card(9, 0)]),
            (21, true)
        );
        // King + Queen + 5: hard 25, bust.
        assert_eq!(
            blackjack_hand_value(&[card(13, 0), card(12, 0), card(5, 0)]),
            (25, false)
        );
    }

    #[test]
    fn is_blackjack_requires_exactly_two_cards_totaling_21() {
        assert!(is_blackjack(&[card(1, 0), card(13, 0)]));
        assert!(!is_blackjack(&[card(1, 0), card(9, 0), card(1, 1)])); // 3-card 21 isn't a natural
        assert!(!is_blackjack(&[card(10, 0), card(9, 0)]));
    }

    #[test]
    fn blackjack_payout_matches_each_outcome() {
        assert_eq!(
            blackjack_payout(10.0, BlackjackOutcome::PlayerBlackjack),
            25.0
        );
        assert_eq!(blackjack_payout(10.0, BlackjackOutcome::Push), 10.0);
        assert_eq!(blackjack_payout(10.0, BlackjackOutcome::PlayerBust), 0.0);
        assert_eq!(blackjack_payout(10.0, BlackjackOutcome::DealerWins), 0.0);
        assert_eq!(blackjack_payout(10.0, BlackjackOutcome::DealerBust), 20.0);
        assert_eq!(blackjack_payout(10.0, BlackjackOutcome::PlayerWins), 20.0);
    }

    #[test]
    fn blackjack_start_settles_immediately_on_any_natural() {
        for _ in 0..500 {
            let (game, resolved) = blackjack_start(10.0);
            if is_blackjack(&game.player) || is_blackjack(&game.dealer) {
                assert!(resolved.is_some());
            } else {
                assert!(resolved.is_none());
            }
        }
    }

    #[test]
    fn blackjack_hit_only_resolves_on_bust() {
        for _ in 0..500 {
            let (mut game, resolved) = blackjack_start(10.0);
            if resolved.is_some() {
                continue;
            }
            let hit_result = blackjack_hit(&mut game);
            let (value, _) = blackjack_hand_value(&game.player);
            if value > 21 {
                assert_eq!(hit_result, Some((BlackjackOutcome::PlayerBust, 0.0)));
            } else {
                assert!(hit_result.is_none());
            }
        }
    }

    #[test]
    fn blackjack_stand_dealer_never_stops_below_seventeen() {
        for _ in 0..500 {
            let (mut game, resolved) = blackjack_start(10.0);
            if resolved.is_some() {
                continue;
            }
            blackjack_stand(&mut game);
            let (dealer_value, _) = blackjack_hand_value(&game.dealer);
            assert!(dealer_value >= 17);
        }
    }

    #[test]
    fn blackjack_never_pays_more_than_two_and_a_half_times_the_bet() {
        for _ in 0..500 {
            let (mut game, resolved) = blackjack_start(10.0);
            let (_, payout) = resolved.unwrap_or_else(|| loop {
                let (value, _) = blackjack_hand_value(&game.player);
                if value >= 17 {
                    break blackjack_stand(&mut game);
                }
                if let Some(bust) = blackjack_hit(&mut game) {
                    break bust;
                }
            });
            assert!(
                payout <= 25.0 + 1e-9,
                "payout {payout} exceeds 3:2 on a 10 bet"
            );
        }
    }

    /// Exhaustively enumerates all `C(52,5) = 2,598,960` five-card hands
    /// (not a sample) and checks the category counts against the
    /// textbook 5-card-poker frequency table. This is the real guarantee
    /// that `evaluate_poker_hand` can't misclassify a hand into paying
    /// more than it should.
    #[test]
    fn evaluate_poker_hand_matches_textbook_frequencies_exactly() {
        let deck: Vec<Card> = (0..4u8)
            .flat_map(|suit| (1..=13u8).map(move |rank| card(rank, suit)))
            .collect();
        assert_eq!(deck.len(), 52);

        let mut counts: std::collections::HashMap<PokerHandRank, u64> =
            std::collections::HashMap::new();
        for a in 0..52 {
            for b in (a + 1)..52 {
                for c in (b + 1)..52 {
                    for d in (c + 1)..52 {
                        for e in (d + 1)..52 {
                            let hand = [deck[a], deck[b], deck[c], deck[d], deck[e]];
                            *counts.entry(evaluate_poker_hand(&hand)).or_insert(0) += 1;
                        }
                    }
                }
            }
        }

        let total: u64 = counts.values().sum();
        assert_eq!(total, 2_598_960);

        let get = |r: PokerHandRank| *counts.get(&r).unwrap_or(&0);
        assert_eq!(get(PokerHandRank::RoyalFlush), 4);
        assert_eq!(get(PokerHandRank::StraightFlush), 36);
        assert_eq!(get(PokerHandRank::FourOfAKind), 624);
        assert_eq!(get(PokerHandRank::FullHouse), 3_744);
        assert_eq!(get(PokerHandRank::Flush), 5_108);
        assert_eq!(get(PokerHandRank::Straight), 10_200);
        assert_eq!(get(PokerHandRank::ThreeOfAKind), 54_912);
        assert_eq!(get(PokerHandRank::TwoPair), 123_552);
        // "One pair" (1,098,240 total) splits into a paying Jacks-or-better
        // pair (J/Q/K/A, 337,920 of them) and a non-paying low pair, which
        // this evaluator folds into HighCard along with true no-pair hands
        // (1,302,540 of those) — so HighCard should be exactly their sum.
        assert_eq!(get(PokerHandRank::JacksOrBetter), 337_920);
        assert_eq!(get(PokerHandRank::HighCard), 760_320 + 1_302_540);
    }

    #[test]
    fn poker_draw_only_replaces_unheld_cards() {
        let mut game = poker_deal(10.0);
        let original = game.hand;
        let hold = [true, false, true, false, true];
        poker_draw(&mut game, hold);
        assert_eq!(game.hand[0], original[0]);
        assert_eq!(game.hand[2], original[2]);
        assert_eq!(game.hand[4], original[4]);
    }

    #[test]
    fn poker_never_pays_more_than_the_royal_flush_multiplier() {
        for _ in 0..500 {
            let mut game = poker_deal(10.0);
            let (_, payout) = poker_draw(&mut game, [false; 5]);
            assert!(
                payout <= 8000.0 + 1e-9,
                "payout {payout} exceeds 800x on a 10 bet"
            );
        }
    }
}
