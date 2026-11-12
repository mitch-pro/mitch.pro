# Database encryption

`mitchpro.db` can be encrypted with SQLCipher (AES-256, whole file). It is off by
default; `DB_ENCRYPTION=1` turns it on. The key is derived from the master key
described in [encryption-at-rest.md](encryption-at-rest.md), so the same
`DATA_ENCRYPTION_KEY` (Doppler, or Infisical via `infisical run`) protects saves
and the database.

Code: `mitch_lib::db_crypto` (open, convert, snapshot, restore),
`mitch_lib::data::DataStore::open`, `mitch-server/src/db_cli.rs` (the
commands), `spawn_db_snapshots` in `mitch-server/src/main.rs`.

## What changes when it is on

- Every process that opens the database needs the key: the web server **and
  mail-rs**. `docker-compose.yml` passes `DB_ENCRYPTION` and the key variables to
  both. mail-rs reads the key from its environment only; it doesn't call the
  Infisical API itself.
- **Litestream can't replicate an encrypted database**, so the `sql-sync` sidecar
  must not be used with it (see "Backups").
- Anything that opens the file as plain SQLite can't read it: the Python tools
  in `tools/` (`reset-coin-balances.py`, `repair-request-log-table.py`), the
  `sqlite3` CLI, and the sidecar's seed step. To inspect or repair data, run
  `decrypt-db` on a copy of the stopped database, or use a SQLCipher-aware client.
- The server refuses to open a plaintext database while `DB_ENCRYPTION` is on, and an
  encrypted one while it is off. It never converts the file by itself.

## Converting

Stop **every** service that uses the database first (web servers, mail-rs, the
Litestream sidecar). The command refuses to run if the database has a non-empty
`-wal` or `-shm` file next to it, which is what a live one looks like.

```sh
# DATA_DIR / MITCH_DB_PATH as the services use them; the key in the environment
doppler run -- mitch-server encrypt-db
```

It writes `mitchpro.db.encrypted`, checks it (same tables, same row counts,
`quick_check`), then renames the original to `mitchpro.db.plaintext-<ms>` and puts
the new file in place. Set `DB_ENCRYPTION=1` and start the services. Delete the
plaintext original, and any Litestream replica of it, only after the encrypted
database has run and snapshotted correctly. `decrypt-db` does the reverse to roll
back (original kept as `mitchpro.db.encrypted-<ms>`).

A new database (no file yet) simply starts encrypted.

## Backups

Each web server process writes an encrypted snapshot every
`DB_SNAPSHOT_INTERVAL_MIN` minutes (default 15) to `DB_SNAPSHOT_DIR` (default
`<data dir>/db-snapshots`, on the host disk, not the ramdisk) using SQLite's
backup API on its own read connection, so the application's writes aren't
blocked. The newest `DB_SNAPSHOT_KEEP` (default 96) are kept, plus the first
snapshot of each UTC day for `DB_SNAPSHOT_DAILY_DAYS` (default 14). A failing
snapshot logs an error and sends one ntfy notice per streak.

If the database file is missing at start (a cleared ramdisk), the newest snapshot
that opens with the key and passes `quick_check` is restored first, which replaces
Litestream's `restore-if-db-not-exists`.

**This is weaker than Litestream.** Litestream loses about a second on a crash; this
loses up to the snapshot interval. Lower the interval, or keep the database on
disk instead of the ramdisk (`MITCH_DB_PATH=/app/data/mitchpro.db`), if that
matters. Snapshots are encrypted with the same key, so they are useless without it.
Test a restore before relying on it: stop the services, delete the database file,
start one, and check the log for "restored from snapshot".

## Compose changes needed

When turning this on, stop using the sidecar: drop `sql-sync` and the
`depends_on` entries that wait for it, and either point `MITCH_DB_PATH` at the disk
or keep the ramdisk volume with the restore-from-snapshot behaviour above. Leaving
the Litestream sidecar running against an encrypted database makes it fail to start
replicating, and its `restore-if-db-not-exists` would put the old plaintext
replica back on an empty ramdisk, which the server then refuses to open.

## Key rotation

Put the new key in `DATA_ENCRYPTION_KEY` and the old one in
`DATA_ENCRYPTION_KEY_PREVIOUS`. The server opens the database with the old key and
re-keys it to the new one (it briefly leaves WAL mode to do that). Old snapshots
stay under the old key until they are pruned, so keep the old key until they are
gone.

## Build

`rusqlite` uses `bundled-sqlcipher-vendored-openssl`: SQLCipher plus OpenSSL
compiled from source, which needs `perl` and `make` (the Dockerfile's builder stage
already installs them) and adds a few minutes to a cold build.
