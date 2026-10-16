#!/bin/sh
# Litestream sidecar entrypoint.
#
# One-time bootstrap: the very first time this ever runs, there is no
# existing Litestream replica yet (nothing under $REPLICA_DIR) and no
# database on the ramdisk - only the pre-Litestream database on the old
# HDD-backed path ($LEGACY_DB). Without this seed step, Litestream would
# start replicating an empty database on that first run, silently losing
# every pre-existing row. Seed the ramdisk from the legacy file first; from
# then on the Litestream replica itself is the source of truth and this
# branch never fires again (both $RAMDISK_DB and $REPLICA_DIR/ltx exist on
# every later boot, restored by `restore-if-db-not-exists` below).
#
# Verified directly against a real v0.5.14 replica: this version stores
# its replica as $REPLICA_DIR/ltx/<level>/<txid-range>.ltx, NOT the older
# "generations/" layout some docs and older versions describe - checking
# for the wrong directory name here would make this seed step fire again
# on every single boot even once a real replica exists, re-running the
# (unnecessary, if harmless) legacy backup every time instead of just once.
set -eu

RAMDISK_DB=/ramdisk/mitchpro.db
REPLICA_DIR=/replica
LEGACY_DB=/legacy/mitchpro.db

if [ ! -f "$RAMDISK_DB" ] && [ ! -d "$REPLICA_DIR/ltx" ] && [ -f "$LEGACY_DB" ]; then
    echo "[sql-sync] No ramdisk DB or existing Litestream replica found - seeding ramdisk from legacy $LEGACY_DB"
    # NOT a plain `cp`: this deploy's old (still-active, still-serving-
    # traffic) webserver slot is writing to $LEGACY_DB for the entire
    # duration of this seed step - a raw file copy of a live WAL-mode
    # database can capture a torn, inconsistent snapshot. sqlite3's
    # `.backup` uses SQLite's own online-backup API, which is explicitly
    # safe against concurrent writers (the same mechanism Litestream's own
    # periodic snapshots use internally) and produces one clean, complete
    # file - no separate -wal/-shm copying needed or wanted alongside it.
    #
    # `?immutable=1`: $LEGACY_DB is mounted read-only, and plain SQLite opens
    # - even read-only ones - need to create a -shm file alongside the main
    # db for WAL coordination, which fails hard on a read-only mount
    # (verified directly: a plain open against the :ro mount errors with
    # "unable to open database file"). immutable=1 tells SQLite not to
    # expect or check for concurrent changes, skipping that requirement -
    # exactly right for this one-time bootstrap read, which only ever needs
    # to run once anyway (every load thereafter restores from Litestream's
    # own replica, written with full consistency guarantees, not this path).
    sqlite3 "file:$LEGACY_DB?immutable=1" ".backup '$RAMDISK_DB'"
    echo "[sql-sync] Seed complete ($(wc -c < "$RAMDISK_DB") bytes)."
fi

exec litestream replicate -config /etc/litestream.yml
