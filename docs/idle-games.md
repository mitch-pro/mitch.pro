# Idle games (Adrian's Clicker, Richard's Riches)

`rust/crates/mitch-server/src/routes/games/idle.rs` implements both idle
games. Each has its own in-game currency (`points`/`cash`) that converts
to real MitchCoins through two paths: an explicit "claim" (spend in-game
currency at an exponentially increasing cost per coin) and a passive
"playtime bonus" (a flat MitchCoins trickle just for having the game open).

## The playtime-bonus exploit (fixed, both games)

The playtime bonus is computed as `floor((now - startTime) / 600_000)`
whole 10-minute units since `startTime`, minus whatever's already been
credited (`playtimeCoins`), times a flat per-unit payout. The bug: neither
game ever reset `startTime` on idle or logout, so `now - startTime` was
wall-clock time since the session's first-ever creation, not time since
the last sync. Anyone who went idle and came back got the *entire*
elapsed gap paid out in one sync — a multi-day gap alone produced tens of
thousands of MitchCoins in a single request (the reported "~70k/day").

Fixed identically in both `process_adrian_sync` and `richard_sync`:
whenever there's been a >30s gap since the account's own `lastTs`, reset
`startTime` to now and `playtimeCoins` to 0 *before* computing the bonus,
and additionally clamp the elapsed-time inputs themselves (`elapsed` to
30s, `session_elapsed` to 12h) as a second layer of defense independent
of the reset. A session that's never idle more than 30s (the frontend
auto-syncs every 10s while a tab is open) can still legitimately accrue
up to the 12h cap; one that goes idle gets zeroed out instead of paid out.

Walking it through in a loop (the standard "can this be abused
repeatedly" check — see AGENTS.md's economy-integrity rule): the only way
to extract more than the capped legitimate rate is to deliberately time
visits to land just past each 10-minute boundary and then go idle 31s+
before the *next* one, over and over. That requires active scripting
effort for a bounded, modest ceiling — not a passive "leave it running"
or "don't check it for a week" exploit.

Per-unit payout is 25 MitchCoins (non-premium) / 50 (premium) for
Richard's Riches — one unit is 10 minutes, so that's 150/300 coins per
hour at the capped rate. Reduced from 100/200 at the same time as the
reset fix (then 50/100, then this), since the exploit was about both the
frequency *and* the size of each payout. Adrian's Clicker's playtime
bonus pays 1/2 (non-premium/premium),
much smaller because it's additionally gated behind an explicit `claim`
rather than granted on every sync.

## If you touch either sync function again

Both `process_adrian_sync` (Adrian, extracted as its own function, called
from both `/sync` and `/buy`) and `richard_sync` (Richard, inline — no
separate `/buy` endpoint) move real money. Keep the reset-before-compute
ordering intact, and if you add a third idle game with the same
playtime-bonus shape, give it the identical reset, not a copy of the
pre-fix unreset version.
