# Tests

The primary suite is the Rust workspace's own `cargo test`:

- Unit tests live next to the code (`rust/crates/*/src/**/*.rs`, `#[cfg(test)] mod tests`).
- Integration tests in `rust/crates/mitch-server/tests/` start the real `mitch-server`
  binary against a throwaway data directory, seed fixture accounts into its SQLite
  database, and call it over HTTP. They never touch production data. Shared setup is in
  `tests/common/mod.rs`.

| File | Covers |
|---|---|
| `tester_hub.rs` | `/api/tester/*` and the tester sandbox rules |
| `staff_access.rs` | Owner / co-owner / admin / member gates on VM owner-resources and account lists |

```sh
cargo test --manifest-path rust/Cargo.toml --workspace
```

There are no browser or Node tests. Layout and client-side JavaScript are checked by hand.

## Python scripts here

`test_coin_balance_reset.py` and `test_request_log_repair.py` test the maintenance
scripts in `tools/`. `setup_ban_fixture.py` builds a local fixture for manual ban checks
(see `docs/account-ban-cleanup.md`). Run the tests with `python3 tests/<file>.py`.

## Shell tests

`test_secrets_provider.sh` checks the secrets-provider block copied into the host scripts.
`test_deploy_rollback.sh` runs `tools/deploy.sh` against stand-in commands to check the
health-check retry and git rollback (`docs/deploy-health-check.md`). Run with `bash tests/<file>.sh`.

## CI

`rust-ci.yml` runs fmt, clippy and `cargo test --workspace`. The production deploy waits
for that workflow to pass.
