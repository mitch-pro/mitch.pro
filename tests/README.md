# Tests

The primary test suite is the Rust workspace's `cargo test`:

- Unit tests live alongside the code (`rust/crates/*/src/**/*.rs`, `#[cfg(test)] mod tests`).
- Integration tests in `rust/crates/mitch-server/tests/` start the `mitch-server`
  binary against a throwaway data directory, seed fixture accounts into its SQLite
  database, and call it over HTTP. They never touch production data. Shared setup lives in
  `rust/crates/mitch-server/tests/common/mod.rs`.

| File | Covers |
|---|---|
| `encryption_at_rest.rs` | `/api/userdata` save encryption on disk and key bootstrap |
| `matrix_outside_access.rs` | Moderator approval gate for non-student Matrix chat access |
| `privacy.rs` | In-memory presence, no persisted IP/page history, startup cleanup, hashed IP bans |
| `site_updates.rs` | Homepage update log admin CRUD and public feed |
| `staff_access.rs` | Owner / co-owner / admin / member gates on VM owner resources and account lists |
| `tester_hub.rs` | `/api/tester/*` and beta tester sandbox rules |
| `vm_access.rs` | Owner-consented admin VM access grants and owner-locked VMs |

```sh
cargo test --manifest-path rust/Cargo.toml --workspace
```

## JavaScript helper tests

Two Node.js unit tests check pure client-side helpers without a browser:

- `test_e2ee_backup.mjs` — Matrix E2EE recovery-key encoding, transfer validation, key chunking, and device naming (`webserver/matrix/e2ee-backup.js`).
- `test_holiday_theme.mjs` — holiday theme date windows and overrides (`webserver/site-theme.js`).

Run with `node tests/<file>.mjs`. There are no headless browser tests; page layout and client UI are verified manually.

## Python tests

- `test_coin_balance_reset.py` and `test_request_log_repair.py` test the maintenance scripts in `tools/`.
- `setup_ban_fixture.py` builds a synthetic local fixture for manual ban checks (see `docs/account-ban-cleanup.md`).

Run with `python3 tests/<file>.py`. CI's email-leak scanner also has unit tests at `.github/scripts/test_check_new_emails.py` (`python3 -m unittest discover -s .github/scripts -v`).

## Shell tests

- `test_secrets_provider.sh` checks the Infisical helper block shared across the host scripts.
- `test_deploy_rollback.sh` exercises `tools/deploy.sh` health-check retries and git rollback against stand-in commands (`docs/deploy-health-check.md`).

Run with `bash tests/<file>.sh`.

## CI

- `.github/workflows/rust-ci.yml` runs `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`. Production deploys wait for this workflow to pass.
- `.github/workflows/email-check.yml` runs the scanner tests and checks PR diffs for non-allowlisted email addresses (`docs/email-check.md`).
