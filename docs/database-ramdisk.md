# Database placement

The default application database remains `/app/data/mitchpro.db`. `MITCH_DB_PATH` can point both webserver slots and mail-rs at a shared alternative, including `/ramdisk/mitchpro.db`. It must be identical for every writer. The `sql-sync` sidecar (`mitch-dbsync`) snapshots it to disk every minute; see [database-encryption.md](database-encryption.md).

RAM operation is opt-in. Do not enable it during an ordinary blue/green swap while the active slot is still writing the disk database: the seed and active database would diverge. A coordinated migration must stop every writer, take a final consistent SQLite backup, ensure the RAM database and its directory are writable by UID 1000, switch all writers together, and verify snapshots before reopening traffic. Rollback after accepting writes must carry the latest data back to disk. Existing disk data and replicas must be backed up and verified before that operation.

The sidecar's initial seed uses SQLite's online backup API, so committed WAL rows are included. Its legacy mount allows WAL shared-memory coordination. Do not use `immutable=1` against a live WAL database. A snapshot produced before RAM operation is enabled is only a seed, not an ongoing backup of the still-active disk database; it must not be used as the source of a later migration without refreshing and verifying it after stopping writers.
