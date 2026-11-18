# Database encryption

`mitchpro.db` is encrypted with SQLCipher using the key `MITCH_DB_KEY`, which
Infisical injects as an environment variable (see
[secrets-providers.md](secrets-providers.md)). Saves are encrypted separately;
see [encryption-at-rest.md](encryption-at-rest.md).

Code: `mitch_lib::dbcrypt` (key, keyed open, snapshot, export),
`rust/crates/mitch-dbsync` (the sidecar), `docker-compose.yml` (`sql-sync`).

## What it protects

The files on the HDD: the snapshots in `data/db-snapshots`, and a copy of the
`data/` folder, a backup, or a stolen disk. It does not protect against someone
with root on the running server: the live database is decrypted in the
application's memory and the key is in its environment. The live database
itself sits on a tmpfs and disappears on power loss.

## The key

64 hex characters (32 random bytes): `openssl rand -hex 32`. Store it in
Infisical as `MITCH_DB_KEY` and keep an offline copy. **Without the key the
database and every snapshot are unrecoverable.** Rotation is not implemented.

With `MITCH_DB_KEY` unset, everything runs on plain SQLite as before (local
development, tests). With it set, the server refuses a plain database and
refuses an encrypted database opened with a wrong key, rather than creating a
second database. The sidecar logs the key's fingerprint (first 8 hex characters
of its SHA-256) so two keys can be told apart.

## Durability: `mitch-dbsync`

The `sql-sync` service replaces the Litestream sidecar. Litestream parses the
database file itself and cannot replicate an encrypted one.

- **Every 60 seconds** (`DBSYNC_INTERVAL_SECS`) it copies the ramdisk database
  with SQLite's online-backup API, which is safe against concurrent writers.
  The copy is encrypted with the same key, checked with `PRAGMA quick_check`,
  fsynced, and renamed into `data/db-snapshots/mitchpro-<unix time>.db`.
  Unchanged databases (same size and modification time of the file and its WAL)
  are skipped, with a forced snapshot at least hourly.
- **No overlap.** Each cycle runs to completion before another starts. If a
  snapshot is still running after 60 seconds, the sidecar sends an ntfy notice
  ("Database sync is slow"), lets it finish, skips the ticks it missed, and
  sends a second notice when it catches up. A failed snapshot sends an urgent
  notice, again roughly every 30 minutes while it keeps failing, and one when
  it recovers. ntfy uses `NTFY_TOPIC`, `NTFY_USER`, `NTFY_PASS`.
- **Retention.** The newest 3 snapshots, the newest per hour for 12 hours, and
  the newest per day for 7 days: at most about 22 full copies, so budget about
  22 times the database size on the HDD.
- **Recovery point.** A crash or power loss loses up to about a minute of
  writes (Litestream lost about a second).
- **Restore.** If the ramdisk has no database at start, the newest snapshot
  that opens with the current key is restored. If snapshots exist but none
  opens, the sidecar refuses to start rather than quietly falling back to the
  stale legacy file. With no snapshots it seeds from `data/mitchpro.db` (the
  pre-ramdisk file), or creates an empty database.
- **Ready marker.** The healthcheck waits for `/ramdisk/.dbsync-ready`, written
  after the database is restored, checked, and handed to the app user (uid 1000).
  The webservers wait for it.

Sync time is roughly database size divided by the HDD's write speed, so a
database of a few GB fits in the minute. Much larger databases would need
incremental sync, which this design does not have.

## Switching over

Existing data is plain (the live ramdisk database, `data/mitchpro.db`, and the
Litestream replica in `data/litestream-replica`). Do these in order, on the
server, with the commands run by a person:

1. Deploy this change **without** `MITCH_DB_KEY`. The sidecar starts in plain
   mode and warns through ntfy that snapshots are unencrypted. Nothing else
   changes. Confirm snapshots appear in `data/db-snapshots`.
2. Add `MITCH_DB_KEY` to Infisical and take the offline copy.
3. Stop the writers and the sidecar (a short outage), then encrypt in place.
   `docker compose` here means the usual `infisical run -- docker compose ...`
   wrapper so the key is present:

   ```sh
   docker compose stop webserver-blue webserver-green mail-rs sql-sync
   docker compose run --rm --no-deps sql-sync migrate
   docker compose up -d
   ```

   `migrate` encrypts the ramdisk database (checking row counts afterwards),
   writes an encrypted snapshot, and deletes plain snapshots. If the ramdisk is
   empty it seeds from the legacy file instead.
4. Check the site, then remove the remaining plain copies:

   ```sh
   docker compose run --rm --no-deps sql-sync scrub-plaintext
   ```

   It refuses unless a verified encrypted snapshot exists and `MITCH_DB_PATH` is
   the ramdisk path (so the app no longer uses `data/mitchpro.db`). It deletes
   `data/mitchpro.db` and its `-wal`/`-shm`, and the Litestream replica. Deleted
   files can survive in free disk blocks until overwritten, so this does not
   defeat someone who already has an old image of the disk.

This procedure assumes the ramdisk setup (`MITCH_DB_PATH=/ramdisk/mitchpro.db`).
Encrypting the on-disk default (`/app/data/mitchpro.db`) is not covered: with
a key set the server refuses that plain file.

## Other tools that read the database

`newsletter.py` and `admin/admin_tools.py` open `mitchpro.db` with Python's
stock `sqlite3`, which cannot read an encrypted file. Both already catch the
error and fall back to their JSON files, so they degrade rather than crash, but
they will not see database content. The same applies to any manual `sqlite3`
session; use `sqlcipher` with `PRAGMA key = "x'<key>'";`.

## Testing

`cargo test -p mitch-lib dbcrypt` covers the key rules, the encrypted-file
checks, snapshots and the plain-to-encrypted export. `cargo test -p
mitch-dbsync` covers scheduling (overrun reported once, no overlap, realignment
after an overrun), retention, and a full snapshot cycle against an encrypted
database. The `wrong key` log lines from SQLCipher during those tests are expected.
