# Tests

The primary suite is the Rust workspace's own `cargo test` (`rust/crates/*/src/**/*.rs`,
inline `#[cfg(test)] mod tests` blocks). The older JS unit and integration tests that
read the removed `server.js` have been deleted.

## Browser and script tests here

These run a real local `mitch-server` (build it first with `cargo build`) and use
fixture accounts in a throwaway data directory. They never touch production.

| File | Covers |
|---|---|
| `test_admin_modernization.mjs` | Admin panel, VM resource editor, bans, friends, notifications |
| `test_presence_browser.mjs` | Presence, friends API, privacy toggles |
| `test_admin_email_browser.mjs` | Newsletter editor and delivery |
| `test_leaderboard_ui.mjs` | Leaderboard UI |
| `test_notifications.mjs` | Service-worker notification handling |
| `test_service_worker_unit.js` | Service worker bypass and version consistency (the only `.js` test) |
| `test_saves_page.mjs` | `/saves/` manager and the Matrix/E2E key exclusion |
| `test_tester_hub.mjs` | `/api/tester/*` and the tester sandbox rules |
| `test_site_ui_browser.mjs`, `test_site_refresh_browser.mjs`, `test_site_bans_browser.mjs`, `test_game_discovery_browser.mjs`, `test_matrix_*` | Other site-level browser checks |

## Running

```sh
cargo test --manifest-path rust/Cargo.toml --workspace   # primary suite, no server needed
npx playwright install chromium                           # once, for the browser tests
cargo build --manifest-path rust/Cargo.toml               # the browser tests spawn rust/target/debug/mitch-server
node tests/test_saves_page.mjs                            # etc.
```

## CI

`rust-ci.yml` runs fmt, clippy and `cargo test --workspace`, then these browser
tests: `test_leaderboard_ui.mjs`, `test_service_worker_unit.js`,
`test_notifications.mjs`, `test_admin_modernization.mjs`,
`test_presence_browser.mjs` and `test_admin_email_browser.mjs`. The deploy waits for
that workflow to pass.
