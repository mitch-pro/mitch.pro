# Casino (Coinflip, Dice, Slots, Blackjack, Poker)

`rust/crates/mitch-lib/src/casino.rs` holds all game logic — RNG draws,
payout tables, hand evaluation — as plain functions with no database
access. `rust/crates/mitch-server/src/routes/casino.rs` is the HTTP layer
on top of it. Frontend is `webserver/casino/{index.html,casino.js,casino.css}`.

## Money-safety model

1. **One function moves money.** `mitch_lib::coins::settle_bet` debits the
   bet and credits the payout against the same account row inside one
   `BEGIN IMMEDIATE` transaction (actually just one `std::sync::Mutex`-
   guarded connection, held for the whole operation). There is no window
   between "check the balance" and "write the new balance" where a second
   concurrent bet for the same email could read the same stale balance and
   also pass — see `coins.rs`'s `settle_bet_never_goes_negative_under_concurrent_bets`
   test, which fires 20 threads at one 10-coin balance and asserts exactly
   one succeeds.
2. **The server always rolls first.** Every game computes its outcome from
   `rand::rng()` before it ever looks at the account balance. The client
   only ever supplies a bet amount and a choice (heads/tails, a dice
   target, which cards to hold) — never an outcome.
3. **Every game targets a ~4% house edge (96% RTP).** Coinflip and Dice
   derive their payout multiplier directly from `HOUSE_EDGE`. Slots and
   Poker get there through their paytables instead, and those paytables
   are checked by *exhaustive enumeration* tests, not by hand-derived
   arithmetic: `slots_exact_rtp_is_a_light_house_edge` sums the exact
   expected payout over all 8,000 reel-stop combinations, and
   `evaluate_poker_hand_matches_textbook_frequencies_exactly` enumerates
   all 2,598,960 five-card hands and checks the category counts against
   the published 5-card-poker frequency table. Poker's paytable is the
   standard "6/5 Jacks or Better" table (~95% RTP under optimal play,
   published figure — not independently re-derived here, since that
   requires simulating optimal draw strategy).
4. **Blackjack/Poker hand state is server-memory-only.** `AppState`'s
   `blackjack_games` / `poker_games` maps (keyed by normalized email) hold
   the in-progress hand; a `GET /api/casino/blackjack/state` (and
   `/poker/state`) lets the frontend recover it after a reload, since
   `/start` refuses a second hand while one is active. A server restart
   mid-hand loses that hand's bet — the money was already debited at
   whichever point the round would normally settle, so this never creates
   money, it just means an abandoned hand doesn't get played out.
5. **Bets are never auto-retried.** `webserver/casino/casino.js` uses a
   plain `fetch`, not the site's `fetchWithRetry` — a bet isn't
   idempotent, so retrying one whose response got lost after the server
   already settled it would be a bad experience even though it can't
   create money (the retried request just resolves a *new* round, or 404s
   if nothing is pending).

## Admin kill switch

`AppState::casino_enabled` (an `AtomicBool`, persisted to
`data/casino_settings.json`) gates every `/api/casino/*` mutating route.
Toggle it with `POST /api/admin/casino/toggle` (full admins only) in the
admin panel's Economy section. There's no separate casino audit log —
every settle writes to the existing `logs/coins.log` with a
`casino:<game>:<outcome>` reason prefix, so the admin Coin Log table's
free-text search already filters to just casino activity.

## Adding a game

Keep the split: a pure `play_*`/`resolve_*` function in `mitch-lib::casino`
that takes a bet and returns an outcome + payout (no DB access, no
`settle_bet` call), a thin route handler in `mitch-server::routes::casino`
that calls it once and settles once, and — if the odds aren't a simple
fixed-probability multiplier — an exhaustive-enumeration test proving the
real RTP, the same way Slots and Poker are proven here. Don't let a new
game call `settle_bet` more than once per round, and don't let it accept
anything resembling an outcome from the request body.
