//! `mitch-dbsync`: keeps the encrypted ramdisk database durable.
//!
//! The live `mitchpro.db` sits on a shared tmpfs. This sidecar copies it to
//! disk once a minute as an encrypted SQLCipher snapshot, restores the newest
//! good snapshot if the ramdisk comes up empty, and notifies through ntfy when
//! a snapshot overruns its slot or fails. Replaces the Litestream sidecar,
//! which cannot replicate an encrypted database.
//!
//! Commands: `run` (default), `migrate`, `scrub-plaintext`.
//! Full guide: `docs/database-encryption.md`.

// Tests may unwrap; production code may not (Cargo.toml lints).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod config;
mod cycle;
mod ntfy;

use config::Config;
use cycle::{chown_db_files, list_snapshots, next_delay, run_cycle, sync_once, Outcome, SyncState};
use mitch_lib::dbcrypt::{self, FileKind};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

/// Compose's healthcheck waits for this file, so the application never starts
/// before the database is restored, checked and owned by the app user.
const READY_MARKER: &str = ".dbsync-ready";

fn log(msg: &str) {
    println!("[dbsync] {msg}");
}

fn io<T>(r: std::io::Result<T>, what: &str) -> Result<T, String> {
    r.map_err(|e| format!("{what}: {e}"))
}

fn crypt<T>(r: Result<T, dbcrypt::DbCryptError>, what: &str) -> Result<T, String> {
    r.map_err(|e| format!("{what}: {e}"))
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Moves a finished database file into place as the live database.
fn publish(cfg: &Config, finished: &Path) -> Result<(), String> {
    io(
        dbcrypt::remove_sidecars(&cfg.db_path),
        "remove stale -wal/-shm",
    )?;
    io(
        std::fs::rename(finished, &cfg.db_path),
        "move database into place",
    )?;
    chown_db_files(cfg);
    Ok(())
}

/// The ramdisk is empty: restore the newest snapshot that opens with the
/// current key, or seed from the legacy database, or start a fresh one.
fn restore_or_seed(cfg: &Config) -> Result<(), String> {
    let key = cfg.key.as_ref();
    let incoming = sibling(&cfg.db_path, ".incoming");
    let _ = std::fs::remove_file(&incoming);

    let snapshots = io(list_snapshots(&cfg.snapshot_dir), "list snapshots")?;
    if !snapshots.is_empty() {
        for (ts, path) in &snapshots {
            match dbcrypt::verify_file(path, key) {
                Ok(()) => {
                    io(std::fs::copy(path, &incoming), "copy snapshot")?;
                    crypt(dbcrypt::verify_file(&incoming, key), "restored copy")?;
                    publish(cfg, &incoming)?;
                    log(&format!("restored database from snapshot {ts}"));
                    return Ok(());
                }
                Err(e) => log(&format!("skipping snapshot {ts}: {e}")),
            }
        }
        // Seeding from the legacy file here would silently roll back to
        // whatever it held before the snapshots began.
        return Err(
            "snapshots exist but none can be opened; not seeding from the legacy database".into(),
        );
    }

    match io(
        dbcrypt::file_kind(&cfg.legacy_db),
        "inspect legacy database",
    )? {
        FileKind::Missing => {
            let conn = crypt(dbcrypt::open_db(&incoming, key), "create database")?;
            io(
                conn.execute_batch("PRAGMA journal_mode = WAL;")
                    .map_err(std::io::Error::other),
                "initialise database",
            )?;
            drop(conn);
            log("no snapshot or legacy database: created an empty database");
        }
        FileKind::Plain => {
            let seed = |dst: &Path| -> Result<(), String> {
                let Some(key) = key else {
                    return crypt(
                        dbcrypt::snapshot(&cfg.legacy_db, dst, None),
                        "copy legacy database",
                    );
                };
                // Take a consistent plain copy first (the old webserver slot
                // may still be writing to the legacy file), then encrypt it.
                // The plain copy lives only in RAM and is removed at once.
                let plain = sibling(&cfg.db_path, ".seed-plain");
                let result = crypt(
                    dbcrypt::snapshot(&cfg.legacy_db, &plain, None),
                    "copy legacy database",
                )
                .and_then(|()| {
                    crypt(
                        dbcrypt::encrypt_plain_db(&plain, dst, key),
                        "encrypt legacy database",
                    )
                });
                let _ = std::fs::remove_file(&plain);
                let _ = dbcrypt::remove_sidecars(&plain);
                result
            };
            seed(&incoming)?;
            log(&format!(
                "seeded the ramdisk from {}",
                cfg.legacy_db.display()
            ));
        }
        FileKind::Encrypted => {
            if key.is_none() {
                return Err(format!(
                    "{} is encrypted but MITCH_DB_KEY is not set",
                    cfg.legacy_db.display()
                ));
            }
            crypt(
                dbcrypt::snapshot(&cfg.legacy_db, &incoming, key),
                "copy legacy database",
            )?;
            log(&format!(
                "seeded the ramdisk from {}",
                cfg.legacy_db.display()
            ));
        }
    }
    publish(cfg, &incoming)
}

/// Brings the ramdisk database into a state the loop can snapshot.
fn startup(cfg: &Config) -> Result<(), String> {
    io(
        std::fs::create_dir_all(&cfg.snapshot_dir),
        "create snapshot dir",
    )?;
    // Leftovers from a crash mid-snapshot.
    if let Ok(entries) = std::fs::read_dir(&cfg.snapshot_dir) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().ends_with(".tmp") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let kind = io(dbcrypt::file_kind(&cfg.db_path), "inspect database")?;
    match (kind, cfg.key.is_some()) {
        (FileKind::Plain, true) => Err(format!(
            "{} is plain but MITCH_DB_KEY is set; run `mitch-dbsync migrate` with every writer stopped",
            cfg.db_path.display()
        )),
        (FileKind::Encrypted, false) => Err(format!(
            "{} is encrypted but MITCH_DB_KEY is not set",
            cfg.db_path.display()
        )),
        (FileKind::Missing, _) => restore_or_seed(cfg),
        _ => Ok(()),
    }
}

fn run(cfg: Config) -> Result<(), String> {
    let ready = cfg
        .db_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(READY_MARKER);
    let _ = std::fs::remove_file(&ready);
    match &cfg.key {
        Some(k) => log(&format!("database encryption on (key {})", k.fingerprint())),
        None => {
            log("WARNING: MITCH_DB_KEY is not set; snapshots are written unencrypted");
            ntfy::send(
                &cfg.ntfy,
                "Database sync: not encrypted",
                "MITCH_DB_KEY is not set, so database snapshots are written unencrypted.",
                "high",
            );
        }
    }
    if let Err(e) = startup(&cfg) {
        ntfy::send(&cfg.ntfy, "Database sync cannot start", &e, "urgent");
        return Err(e);
    }

    let mut state = SyncState::default();
    let first = sync_once(&cfg, &mut state, true).map_err(|e| {
        let msg = format!("first snapshot failed: {e}");
        ntfy::send(&cfg.ntfy, "Database sync cannot start", &msg, "urgent");
        msg
    })?;
    log(&format!(
        "started; first snapshot {first:?}; interval {}s",
        cfg.interval.as_secs()
    ));
    io(std::fs::write(&ready, b"ok"), "write ready marker")?;
    chown_db_files(&cfg);

    let state = Arc::new(Mutex::new(state));
    let mut consecutive_failures: u32 = 0;
    // Each cycle starts on an interval boundary; `next_delay` realigns after
    // one that overran.
    std::thread::sleep(cfg.interval);
    loop {
        let (job_cfg, job_state) = (cfg.clone(), Arc::clone(&state));
        let overran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&overran);
        let notify_cfg = cfg.ntfy.clone();
        let interval = cfg.interval;
        let outcome = run_cycle(
            interval,
            move || {
                let mut guard = job_state
                    .lock()
                    .map_err(|_| "sync state poisoned".to_string())?;
                sync_once(&job_cfg, &mut guard, false).map_err(|e| e.to_string())
            },
            move || {
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
                let msg = format!(
                    "A database snapshot is still running after {}s. It will finish; the \
                     next one starts at the next interval after that, so none overlap.",
                    interval.as_secs()
                );
                log(&msg);
                ntfy::send(&notify_cfg, "Database sync is slow", &msg, "high");
            },
        );
        let (result, elapsed) = match outcome {
            Ok(v) => v,
            Err(e) => (Err(e), cfg.interval),
        };
        match &result {
            Ok(Outcome::Skipped) => {}
            Ok(Outcome::Snapshotted { name, bytes }) => {
                log(&format!(
                    "{name}: {bytes} bytes in {:.1}s",
                    elapsed.as_secs_f32()
                ));
            }
            Err(e) => log(&format!("snapshot failed: {e}")),
        }
        if overran.load(std::sync::atomic::Ordering::SeqCst) {
            let msg = format!(
                "The slow database snapshot finished after {:.0}s.",
                elapsed.as_secs_f32()
            );
            ntfy::send(&cfg.ntfy, "Database sync caught up", &msg, "default");
        }
        match result {
            Ok(_) => {
                if consecutive_failures > 0 {
                    ntfy::send(
                        &cfg.ntfy,
                        "Database sync recovered",
                        "Snapshots are working again.",
                        "default",
                    );
                }
                consecutive_failures = 0;
            }
            Err(e) => {
                consecutive_failures += 1;
                // First failure, then roughly every half hour while it lasts.
                if consecutive_failures == 1 || consecutive_failures.is_multiple_of(30) {
                    let msg = format!("Snapshot failed ({consecutive_failures} in a row): {e}");
                    ntfy::send(&cfg.ntfy, "Database sync failed", &msg, "urgent");
                }
            }
        }
        std::thread::sleep(next_delay(elapsed, cfg.interval));
    }
}

/// Encrypts the plain ramdisk database in place, takes an encrypted snapshot,
/// and removes plain snapshots. Run with every writer and the sidecar stopped.
fn migrate(cfg: Config) -> Result<(), String> {
    let key = cfg
        .key
        .as_ref()
        .ok_or("MITCH_DB_KEY must be set to migrate")?;
    io(
        std::fs::create_dir_all(&cfg.snapshot_dir),
        "create snapshot dir",
    )?;
    match io(dbcrypt::file_kind(&cfg.db_path), "inspect database")? {
        FileKind::Plain => {
            let incoming = sibling(&cfg.db_path, ".incoming");
            crypt(
                dbcrypt::encrypt_plain_db(&cfg.db_path, &incoming, key),
                "encrypt database",
            )?;
            publish(&cfg, &incoming)?;
            log("encrypted the ramdisk database in place");
        }
        FileKind::Encrypted => {
            crypt(
                dbcrypt::verify_file(&cfg.db_path, Some(key)),
                "existing database",
            )?;
            log("the ramdisk database is already encrypted");
        }
        FileKind::Missing => restore_or_seed(&cfg)?,
    }
    let mut state = SyncState::default();
    let outcome = crypt(sync_once(&cfg, &mut state, true), "snapshot")?;
    log(&format!("encrypted snapshot written: {outcome:?}"));
    for (_, path) in io(list_snapshots(&cfg.snapshot_dir), "list snapshots")? {
        if io(dbcrypt::file_kind(&path), "inspect snapshot")? == FileKind::Plain {
            io(std::fs::remove_file(&path), "remove plain snapshot")?;
            log(&format!("removed plain snapshot {}", path.display()));
        }
    }
    log("plain copies may remain: run `scrub-plaintext` once the application is running on the encrypted database");
    Ok(())
}

/// Deletes the plain legacy database and the old Litestream replica, but only
/// once a verified encrypted snapshot exists and the app no longer uses the
/// legacy file.
fn scrub_plaintext(cfg: Config) -> Result<(), String> {
    let key = cfg
        .key
        .as_ref()
        .ok_or("MITCH_DB_KEY must be set to scrub")?;
    let newest = io(list_snapshots(&cfg.snapshot_dir), "list snapshots")?
        .into_iter()
        .next()
        .ok_or("no snapshot exists; refusing to delete the only copy")?;
    crypt(
        dbcrypt::verify_file(&newest.1, Some(key)),
        "newest snapshot",
    )?;
    if cfg.app_db_path.as_deref() != Some(cfg.db_path.as_path()) {
        return Err(format!(
            "MITCH_DB_PATH is {:?}, not {}: the application may still be using the legacy database",
            cfg.app_db_path,
            cfg.db_path.display()
        ));
    }
    for path in [
        cfg.legacy_db.clone(),
        sibling(&cfg.legacy_db, "-wal"),
        sibling(&cfg.legacy_db, "-shm"),
    ] {
        if path.exists() {
            io(std::fs::remove_file(&path), "remove legacy database")?;
            log(&format!("removed {}", path.display()));
        }
    }
    if cfg.replica_dir.is_dir() {
        for entry in io(std::fs::read_dir(&cfg.replica_dir), "read replica dir")? {
            let path = io(entry, "read replica entry")?.path();
            if path.is_dir() {
                io(std::fs::remove_dir_all(&path), "remove replica data")?;
            } else {
                io(std::fs::remove_file(&path), "remove replica file")?;
            }
            log(&format!("removed {}", path.display()));
        }
    }
    log("done; deleted files can survive in free disk blocks until overwritten");
    Ok(())
}

fn main() -> ExitCode {
    let command = std::env::args().nth(1).unwrap_or_else(|| "run".into());
    let cfg = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[dbsync] {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = match command.as_str() {
        "run" => run(cfg),
        "migrate" => migrate(cfg),
        "scrub-plaintext" => scrub_plaintext(cfg),
        other => Err(format!(
            "unknown command {other:?}; use run, migrate or scrub-plaintext"
        )),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[dbsync] error: {e}");
            ExitCode::FAILURE
        }
    }
}
