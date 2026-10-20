# Project instructions

- Commit every completed change to mitchdog.com in this repository.
- Use natural, concise interface copy. Prefer direct labels that describe the content or action.
- Do not add marketing slogans, decorative eyebrow headings, motivational filler, or promotional helper text to ordinary site pages. Do not add copy merely to fill space.
- Keep layouts focused on the page's function. Preserve users' chosen profile photos and equipped cosmetics when displaying their identities.

## Production server access

- Never SSH into the production server on your own initiative, for any reason — not to check logs, not to "quickly" fix something, not because a deploy looks stuck or failed. This applies every single time, with no exceptions, regardless of how routine or low-risk the action seems.
- Deploys happen through the `Deploy to Production` GitHub Actions workflow (triggered by a push to `master`) or `tools/deploy.sh` run manually by a human. Do not invoke either yourself by SSHing in and running it.
- If something on the server needs inspecting or fixing, stop and tell the person you're working with exactly what you want to check or run, and wait for them to say yes before doing it. Give them the specific command, not a vague description.
- This holds even if you already have working SSH access or credentials. Having access is not the same as having permission for this specific action right now.

## CI status checks

- All autonomous agents are REQUIRED to check the status of CI after committing, or if they committed and then stopped (which is fine), before doing anything for the next commit.
- Unless the user explicitly tells them to focus on the next commit and ignore CI, they must check CI status.
- The git repository is public, so the `gh` CLI is not required (e.g., status can be checked via public GitHub Actions API requests or `gh run list`).

## Privacy and telemetry

- Default to strict privacy: never track, aggregate, or expose user actions, current URLs, open tabs, or viewing habits to other users or staff without explicit user opt-in.
- Live operational status (such as online or offline presence) must remain ephemeral in memory. Never log, cache, or persist browsing locations or presence data to disk, SQLite, or JSON storage.
- Respect privacy toggles server-side: user privacy settings (such as hiding page location or presence) must be enforced at the server API layer for all consumers, including friends and administrators.

## Credentials and secrets

- Never hardcode credentials, passwords, secret tokens, API keys, private emails, or personal contact addresses into source code, test fixtures, documentation, or admin tools.
- Secrets and service configuration must always be sourced from environment variables, secure stores, or dynamic user/admin inputs.

## Economy integrity (MitchCoins, shop, casino)

- Any change touching coin balances, the shop catalog, or casino payouts must be checked for a path to risk-free or unbounded profit before it ships — not just "does it work," but "can a player use this to generate coins out of nothing, or guarantee a non-negative outcome on a bet." A feature that removes downside (e.g. refunding a loss) while keeping upside is an infinite-money bug even if no single line of code looks wrong in isolation; walk through what happens if a user plays it in a loop.
- When removing an exploitable item, flag, or mechanic, close every path to it, not just the shop listing: strip the effect from the code that *honors* it (so an account that already has it active loses it immediately on deploy), and filter it out of any on-disk data override (e.g. `shop_catalog.json`) in case stale data still lists it — the same way `happy_hour_*` was retired. Removing only the storefront entry while leaving the effect live for existing holders is not a fix.
- Don't maintain two independent copies of the same catalog/config (e.g. a second hardcoded item list for an admin view). A second copy is how a retired exploit item quietly reappears — it drifts out of sync with the real source of truth and nobody notices until it's live again. Delegate to the one real loader instead.
- This applies to this specific codebase's casino/shop/coins code (`rust/crates/mitch-lib/src/{coins,shop}.rs`, `rust/crates/mitch-server/src/routes/casino.rs`, `.../me/cosmetics.rs`), not just literal money — the same reasoning applies to any limited/valuable resource (premium status, VM credits, etc).

## Verification and test discipline

- Never push without local validation: autonomous agents must run `cargo test --workspace`, `cargo fmt --all -- --check`, and any relevant test suites locally before committing or pushing to `master`.
- Major subsystem integrations (such as cross-origin bridges, auth grant systems, or proxies) require explicit confirmation rather than direct autonomous pushes to `master`.
- Do not commit temporary test artifacts, media assets, or fixture data to tracked webroots or the repository.


