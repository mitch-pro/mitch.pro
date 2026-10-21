# MitchCoin casino

The casino is implemented in `mitch-lib/src/casino.rs`, the authenticated
`/api/casino/{state,play}` routes, and `webserver/casino/`. It uses MitchCoins
only. It does not reuse the retired casino or any shop casino perks.

Each bet is one completed round: coin flip, two dice, or single-zero roulette.
Payout includes the original stake. Flip pays 1.95× with probability 1/2 (97.5%
expected return). Dice under/over seven pays 2.3× with probability 15/36
(95.833…% return); seven loses both choices. Roulette red/black pays 2× with
probability 18/37 (97.297…% return); zero loses both. Every choice has losing
outcomes. Combining opposite bets cannot guarantee non-negative profit: the
expected combined payout is below the combined stake. Dice can win a combined
bet on non-seven outcomes, but a seven loses both stakes. There are no loss
refunds, earning multipliers, lifetime-earned increments, or shop modifiers.

The server draws from OS entropy using rejection sampling. Client-supplied
outcomes, multipliers, and balances are never trusted. Stakes must be integers
from 1 to 10,000. Invalid bets, missing balance, insufficient funds, and failed
entropy abort before committing a debit.

`casino_rounds` and the balance update share a SQLite IMMEDIATE transaction.
The account plus client round UUID is unique. Retrying the same bet returns the
stored result without drawing or charging again; changing a bet under the same
UUID returns 409. This also holds across concurrent processes sharing the DB.
The round ledger is durable, with the latest 25 rounds visible only to their
owner. General coin audit logging follows the commit. A failed auxiliary text
log cannot roll back a committed round; the SQL round remains authoritative.

The client disables controls during a request. An uncertain response keeps its
original UUID and offers **Check this round**, rather than automatically placing
another bet. Refreshing history never places bets. Page navigation can abandon a
pending client request, but its committed result remains visible in history.

Run `cargo test --workspace` for finite-outcome payout checks, rollback,
idempotency, and concurrent duplicate settlement. The admin modernization browser
suite checks all games, responsive layout, authentication, and CSRF locally.
