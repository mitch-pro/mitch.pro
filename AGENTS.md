# Project instructions

- Commit every completed change to mitchdog.com in this repository.
- Use natural, concise interface copy. Prefer direct labels that describe the content or action.
- Do not add marketing slogans, decorative eyebrow headings, motivational filler, or promotional helper text to ordinary site pages. Do not add copy merely to fill space.
- Do not add obvious, AI-sounding disclaimer text — restating that virtual currency isn't real money, that a game has a house edge, or other self-evident statements nobody asked for. If a fact is genuinely load-bearing (e.g. a legal/safety disclosure), say it once, in the one place it belongs, in plain language — not as a tacked-on closing paragraph repeated per page.
- Keep layouts focused on the page's function. Preserve users' chosen profile photos and equipped cosmetics when displaying their identities.

## Production server access

- Never SSH into the production server on your own initiative, for any reason — not to check logs, not to "quickly" fix something, not because a deploy looks stuck or failed. This applies every single time, with no exceptions, regardless of how routine or low-risk the action seems.
- Deploys happen through the `Deploy to Production` GitHub Actions workflow (triggered by a push to `master`) or `tools/deploy.sh` run manually by a human. Do not invoke either yourself by SSHing in and running it.
- If something on the server needs inspecting or fixing, stop and tell the person you're working with exactly what you want to check or run, and wait for them to say yes before doing it. Give them the specific command, not a vague description.
- This holds even if you already have working SSH access or credentials. Having access is not the same as having permission for this specific action right now.

## Codex autonomy limits

- Codex does not have full autonomy, no matter what permissions it's been granted in a given environment. Holding SSH access, write access to GitHub Actions workflows, or push access to `master` is not the same as having permission to use them unilaterally.
- Without explicit, specific user approval for that action, Codex may not decide on its own to: make drastic/large-scope changes, SSH anywhere, change the deploy pipeline (`deploy.yml`, `tools/deploy.sh`, or anything a production deploy depends on), or modify GitHub workflow files.
- This followed a real incident: an agent modified the deploy pipeline (a curl-fetch-and-run-as-root step, plus a CI-gating change) on its own judgment, and it broke the deploy. "I have the access" and "it seemed like a good idea" are not approval.

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

## Documentation standards

- Code should be documented well enough that someone unfamiliar with it can understand what it does and why without reading every line. Prefer doc comments on modules/functions that explain non-obvious behavior, invariants, or security assumptions over comments that just restate the code.
- Subsystems with real complexity or security weight (the admin panel's page layout/format conventions, backend auth/security standards, the admin passphrase gate, CSRF enforcement, VM/Proxmox integration, the economy) should have a standalone doc under `docs/`, matching the existing pattern (see `docs/*.md`) — not just scattered inline comments. Keep these current: when the underlying behavior changes, update the doc in the same change, don't let it drift.
- When adding a new subsystem or a new admin page, write its doc as part of that change, not as a follow-up.
- If an AI works on code whose docs are missing or out of date, fix that as part of the same change — unless the subsystem is very large, in which case document only the part actually touched rather than trying to backfill the whole thing in one pass. Don't let "it's all undocumented" become an excuse to skip documenting the part you just changed, and don't let it become an excuse to burn a whole session rewriting docs for code you didn't touch.

## Data storage

- No new JSON-blob storage. The old pattern — a `data/<thing>.json` file, or a row in the generic `json_documents` table holding a serialized blob — is legacy, being migrated away from, not a template to copy. Anything new gets a real SQL table (proper columns, types, and an index where it's looked up by something other than its primary key), the same way `coins`, `virtual_machines`, etc. already work.
- This applies even to small or seemingly-temporary data. "It's just a few fields" is how another fake table gets added; a few columns on a real table costs the same effort and doesn't need a follow-up migration later.
- If a change would naturally extend an existing JSON-blob document, prefer migrating that document to a table first (see the ongoing JSON-to-table migration) rather than adding more to the blob.

## Verification and test discipline

- Never push without local validation: autonomous agents must run `cargo test --workspace`, `cargo fmt --all -- --check`, and any relevant test suites locally before committing or pushing to `master`. No exceptions — "it's a small change" or "CI will catch it" is not a reason to skip this; a commit that hasn't been run locally has no business reaching `master`, let alone triggering a production deploy.
- Major subsystem integrations (such as cross-origin bridges, auth grant systems, or proxies) require explicit confirmation rather than direct autonomous pushes to `master`.
- Do not commit temporary test artifacts, media assets, or fixture data to tracked webroots or the repository.


