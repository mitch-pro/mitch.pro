# Beta tester hub (`/tester/`)

A sandbox for people on the tester list to check how the site behaves with
different coin balances, premium states, cosmetics and cooldowns, without
touching the real economy.

## Who is a tester

`data/testers.json` (edited from the admin panel, `POST /api/admin/testers`)
lists tester emails. Admins count as testers for *access* (`auth::is_tester_email`,
used by the staff hub link) but only accounts **on the list** get the sandbox: all
`/api/tester/*` endpoints return 403 for everyone else, so an admin's real balance
is never replaced by a sandbox one. Add an admin to the list if they want it. The owner and developers can also toggle themselves from
the admin panel (Site operations -> Tester mode, `GET|POST /api/admin/tester-self`), which adds or removes
only their own email.

Sandbox state lives in the `tester_settings` SQL table
(`mitch_lib::tester`): `premium`, `unlimited_coins`, `custom_coins`,
`unlock_all_cosmetics`, `bypass_cooldowns`. A row means "sandbox tester". Rows are
created when an admin adds the account, or on first hub visit, and deleted when
they are removed from the list, which restores their real balance and real
premium state.

The wallet "Tester" badge (`/api/me/coins` -> `isTester`, mounted by
`mitch-coins.js`) only shows for accounts on the list, so it never links an admin to a
hub they can't use. The hub itself uses the shared site shell and theme tokens
(`/app.css`, `/app-shell.js`), like the other pages.

## API (all require a session on the tester list)

| Endpoint | Body | Effect |
| --- | --- | --- |
| `GET /api/tester/status` | | email, premium, coins, settings |
| `POST /api/tester/toggle-premium` | `{premium}` | force premium on/off |
| `POST /api/tester/coins` | `{mode:"unlimited"}` / `{mode:"custom",amount}` | sandbox balance (custom is 0 to 999,999,999) |
| `POST /api/tester/settings` | `{unlockAllCosmetics?, bypassCooldowns?}` | toggles |
| `POST /api/tester/reset-cooldowns` | | moves the last daily-claim date to yesterday (streak kept) |
| `POST /api/tester/test-notification` | `{message}` (max 200) | in-app notification to the caller |

## Where the settings are honored

- **Coins:** `coins::get_coins / add_coins / spend_coins / settle_bet /
  escrow_bet / settle_escrowed_bet` check `tester::balance` / `apply_delta` first.
  Unlimited reads as 999,999,999 and never drains; custom is tracked exactly and
  can't go negative or overspend. Sandbox changes are not written to `coins`,
  `lifetime_earned` or `logs/coins.log`. `/api/profile` and `/api/me/coins`
  report `unlimitedCoins` so the wallet pill shows ∞.
- **Premium:** `auth::is_premium_email` returns the sandbox flag.
- **Cosmetics:** `shop::sanitize_cosmetics_for_email` adds every non-admin-only
  catalog cosmetic to the tester's owned set when unlock-all is on. Equipping
  saves the owned set, so items equipped while unlocked stay on the account.
- **Daily claim:** `routes/daily_login.rs` lets a sandbox tester with bypass on
  claim repeatedly.

## Economy integrity

Sandbox coins are not real, so anything that would move value *to another
account* is refused for sandbox testers (otherwise they could mint coins for a
real account by paying it, or by losing a wager on purpose):

- marketplace listing and buying (`routes/marketplace.rs`)
- placing or accepting a wager in battleship or chess-vs (either side)
- gifting Premium (`/api/coins/gift-premium`)

Testers are excluded from every leaderboard (`routes/misc.rs`). Things a tester can still
do with unlimited coins are self-contained: shop purchases, casino play, VM
reservations that spend coins, and so on. If a new feature lets one account pay
or credit another, it must refuse `tester::is_sandboxed` accounts too.

Tests: `rust/crates/mitch-lib/src/tester.rs` (unit) and
`tests/test_tester_hub.mjs` (end to end against a local server).
