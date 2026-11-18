//! One sync cycle, the scheduling around it, and snapshot retention.
//!
//! Scheduling contract (see `docs/database-encryption.md`): a snapshot starts
//! every interval. If one is still running when the next is due, the
//! overrun hook fires once, the running snapshot is allowed to finish, and the
//! missed ticks are skipped, so two snapshots never run at the same time.

use crate::config::Config;
use mitch_lib::dbcrypt::{self, DbCryptError};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A snapshot is taken at least this often even when nothing looks changed,
/// in case the change check misses a write.
const FORCE_EVERY: Duration = Duration::from_secs(3600);
/// Newest snapshots always kept.
const KEEP_RECENT: usize = 3;
/// One snapshot per hour is kept this far back...
const HOURLY_FOR: u64 = 12 * 3600;
/// ...and one per day this far back.
const DAILY_FOR: u64 = 7 * 86_400;

/// Runs `job` on its own thread. If it takes longer than `interval`, calls
/// `on_overrun` once and then keeps waiting for it. Returns the job's result
/// and its total running time, or an error if the job panicked.
pub fn run_cycle<T: Send + 'static>(
    interval: Duration,
    job: impl FnOnce() -> T + Send + 'static,
    on_overrun: impl FnOnce(),
) -> Result<(T, Duration), String> {
    let started = Instant::now();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(job());
    });
    let panicked = || "sync job panicked".to_string();
    let value = match rx.recv_timeout(interval) {
        Ok(v) => v,
        Err(RecvTimeoutError::Timeout) => {
            on_overrun();
            rx.recv().map_err(|_| panicked())?
        }
        Err(RecvTimeoutError::Disconnected) => return Err(panicked()),
    };
    Ok((value, started.elapsed()))
}

/// How long to sleep after a cycle that took `elapsed`, so the next one starts
/// on the next interval boundary. A cycle that overran skips the boundaries
/// it missed.
pub fn next_delay(elapsed: Duration, interval: Duration) -> Duration {
    let i = interval.as_nanos().max(1);
    let next = (elapsed.as_nanos() / i + 1) * i;
    Duration::from_nanos((next - elapsed.as_nanos()).min(u64::MAX as u128) as u64)
}

/// Snapshot file name for a Unix time. Zero-padded so names sort by time.
pub fn snapshot_name(ts: u64) -> String {
    format!("mitchpro-{ts:010}.db")
}

fn parse_snapshot_name(name: &str) -> Option<u64> {
    name.strip_prefix("mitchpro-")?
        .strip_suffix(".db")?
        .parse()
        .ok()
}

/// All snapshots in `dir`, newest first.
pub fn list_snapshots(dir: &Path) -> std::io::Result<Vec<(u64, PathBuf)>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if let Some(ts) = entry.file_name().to_str().and_then(parse_snapshot_name) {
            out.push((ts, entry.path()));
        }
    }
    out.sort_by_key(|b| std::cmp::Reverse(b.0));
    Ok(out)
}

/// Which of the snapshot times in `all` to delete at time `now`: keep the
/// newest few, the newest per hour for 12 hours, and the newest per day for a
/// week.
pub fn prune_plan(all: &[u64], now: u64) -> Vec<u64> {
    let mut sorted = all.to_vec();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    let (mut hours, mut days) = (
        std::collections::HashSet::new(),
        std::collections::HashSet::new(),
    );
    let mut delete = Vec::new();
    for (i, &ts) in sorted.iter().enumerate() {
        let age = now.saturating_sub(ts);
        let mut keep = i < KEEP_RECENT;
        if age < HOURLY_FOR && hours.insert(ts / 3600) {
            keep = true;
        }
        if age < DAILY_FOR && days.insert(ts / 86_400) {
            keep = true;
        }
        if !keep {
            delete.push(ts);
        }
    }
    delete
}

/// Cheap "did anything change" signal: size and modification time of the
/// database and its WAL. Every commit touches the WAL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    db: (u64, Option<SystemTime>),
    wal: Option<(u64, Option<SystemTime>)>,
}

fn stat(path: &Path) -> std::io::Result<Option<(u64, Option<SystemTime>)>> {
    match std::fs::metadata(path) {
        Ok(m) => Ok(Some((m.len(), m.modified().ok()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn fingerprint(db: &Path) -> std::io::Result<Fingerprint> {
    let mut wal = db.as_os_str().to_owned();
    wal.push("-wal");
    let db_stat = stat(db)?.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} is missing", db.display()),
        )
    })?;
    Ok(Fingerprint {
        db: db_stat,
        wal: stat(Path::new(&wal))?,
    })
}

#[derive(Default)]
pub struct SyncState {
    last_fingerprint: Option<Fingerprint>,
    last_snapshot: Option<Instant>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Skipped,
    Snapshotted { name: String, bytes: u64 },
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Hands files the sidecar (root) touched back to the application's user, so
/// the server can still open the database and its `-wal`/`-shm`.
pub fn chown_db_files(cfg: &Config) {
    let mut paths = vec![cfg.db_path.clone()];
    for suffix in ["-wal", "-shm"] {
        let mut p = cfg.db_path.as_os_str().to_owned();
        p.push(suffix);
        paths.push(PathBuf::from(p));
    }
    if let Some(parent) = cfg.db_path.parent() {
        paths.push(parent.to_path_buf());
    }
    for p in paths {
        if p.exists() {
            // Not root (local runs) or not permitted: nothing to fix.
            let _ = std::os::unix::fs::chown(&p, Some(cfg.owner_uid), Some(cfg.owner_uid));
        }
    }
}

/// One snapshot: copy, verify, atomically publish, prune.
pub fn sync_once(
    cfg: &Config,
    state: &mut SyncState,
    force: bool,
) -> Result<Outcome, DbCryptError> {
    let fp = fingerprint(&cfg.db_path)?;
    let overdue = state
        .last_snapshot
        .is_none_or(|t| t.elapsed() >= FORCE_EVERY);
    if !force && !overdue && state.last_fingerprint.as_ref() == Some(&fp) {
        return Ok(Outcome::Skipped);
    }

    let name = snapshot_name(unix_now());
    let tmp = cfg.snapshot_dir.join(format!(".{name}.tmp"));
    let result = dbcrypt::snapshot(&cfg.db_path, &tmp, cfg.key.as_ref());
    chown_db_files(cfg);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let bytes = std::fs::metadata(&tmp)?.len();
    std::fs::rename(&tmp, cfg.snapshot_dir.join(&name))?;
    std::fs::File::open(&cfg.snapshot_dir)?.sync_all()?;

    state.last_fingerprint = Some(fp);
    state.last_snapshot = Some(Instant::now());

    let existing = list_snapshots(&cfg.snapshot_dir)?;
    let times: Vec<u64> = existing.iter().map(|(t, _)| *t).collect();
    for ts in prune_plan(&times, unix_now()) {
        let _ = std::fs::remove_file(cfg.snapshot_dir.join(snapshot_name(ts)));
    }
    Ok(Outcome::Snapshotted { name, bytes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mitch_lib::dbcrypt::DbKey;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    const KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    #[test]
    fn delay_aligns_to_the_next_boundary() {
        let i = Duration::from_secs(60);
        assert_eq!(
            next_delay(Duration::from_secs(10), i),
            Duration::from_secs(50)
        );
        assert_eq!(
            next_delay(Duration::from_secs(0), i),
            Duration::from_secs(60)
        );
        // An overrun skips the boundaries it missed instead of firing at once.
        assert_eq!(
            next_delay(Duration::from_secs(130), i),
            Duration::from_secs(50)
        );
        assert_eq!(
            next_delay(Duration::from_secs(60), i),
            Duration::from_secs(60)
        );
    }

    #[test]
    fn a_fast_job_never_reports_an_overrun() {
        let fired = Arc::new(AtomicUsize::new(0));
        let f = fired.clone();
        let (v, _) = run_cycle(
            Duration::from_secs(5),
            || 7,
            move || {
                f.fetch_add(1, Ordering::SeqCst);
            },
        )
        .unwrap();
        assert_eq!(v, 7);
        assert_eq!(fired.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_slow_job_reports_once_and_is_waited_for() {
        let fired = Arc::new(AtomicUsize::new(0));
        let f = fired.clone();
        let (v, took) = run_cycle(
            Duration::from_millis(50),
            || {
                std::thread::sleep(Duration::from_millis(200));
                "done"
            },
            move || {
                f.fetch_add(1, Ordering::SeqCst);
            },
        )
        .unwrap();
        // The job ran to completion (so nothing else could overlap it) and
        // the hook fired exactly once.
        assert_eq!(v, "done");
        assert!(took >= Duration::from_millis(200));
        assert_eq!(fired.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_panicking_job_is_an_error() {
        let r = run_cycle(Duration::from_secs(1), || -> u8 { panic!("boom") }, || {});
        assert!(r.is_err());
    }

    #[test]
    fn retention_keeps_recent_hourly_and_daily() {
        let now = 20 * 86_400 + 5 * 3600; // day 20, 05:00
        let mut all: Vec<u64> = (0..10).map(|m| now - m * 60).collect(); // last 10 minutes
        let hour_newest = now - 2 * 3600 - 30 * 60; // 02:30 today: kept as its hour's newest
        all.push(hour_newest);
        all.push(hour_newest - 60); // 02:29, same hour, older: dropped
        all.push(now - 5 * 86_400); // 5 days ago: kept as that day's newest
        all.push(now - 5 * 86_400 - 60); // same day, older: dropped
        all.push(now - 9 * 86_400); // older than a week: dropped
        let del = prune_plan(&all, now);
        assert!(del.contains(&(hour_newest - 60)));
        assert!(del.contains(&(now - 5 * 86_400 - 60)));
        assert!(del.contains(&(now - 9 * 86_400)));
        assert!(!del.contains(&hour_newest));
        assert!(!del.contains(&(now - 5 * 86_400)));
        // The three newest always survive.
        for m in 0..3 {
            assert!(!del.contains(&(now - m * 60)));
        }
        // Minutes 3..9 are the same hour as the newest: only the newest in
        // the hour (already kept) and the newest-3 survive.
        assert!(del.contains(&(now - 9 * 60)));
    }

    #[test]
    fn snapshot_names_round_trip() {
        assert_eq!(
            parse_snapshot_name(&snapshot_name(1_700_000_000)),
            Some(1_700_000_000)
        );
        assert_eq!(parse_snapshot_name(".mitchpro-1.db.tmp"), None);
        assert!(snapshot_name(5) < snapshot_name(10));
    }

    fn test_cfg(tag: &str, key: Option<DbKey>) -> Config {
        let d = std::env::temp_dir().join(format!(
            "mitch-dbsync-{tag}-{}-{}",
            std::process::id(),
            unix_now()
        ));
        std::fs::create_dir_all(d.join("snaps")).unwrap();
        Config {
            db_path: d.join("live.db"),
            legacy_db: d.join("legacy.db"),
            snapshot_dir: d.join("snaps"),
            replica_dir: d.join("replica"),
            app_db_path: None,
            interval: Duration::from_secs(60),
            owner_uid: u32::MAX,
            key,
            ntfy: Default::default(),
        }
    }

    #[test]
    fn sync_once_snapshots_skips_when_unchanged_and_resumes_after_a_write() {
        let key = Some(DbKey::parse(KEY).unwrap());
        let cfg = test_cfg("once", key.clone());
        let live = dbcrypt::open_db(&cfg.db_path, key.as_ref()).unwrap();
        live.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE t(v); INSERT INTO t VALUES (1);")
            .unwrap();

        let mut state = SyncState::default();
        let first = sync_once(&cfg, &mut state, false).unwrap();
        assert!(matches!(first, Outcome::Snapshotted { .. }));
        assert_eq!(
            sync_once(&cfg, &mut state, false).unwrap(),
            Outcome::Skipped
        );

        live.execute("INSERT INTO t VALUES (2)", []).unwrap();
        // Same-second snapshot names are fine: the rename replaces the file.
        assert!(matches!(
            sync_once(&cfg, &mut state, false).unwrap(),
            Outcome::Snapshotted { .. }
        ));

        let snaps = list_snapshots(&cfg.snapshot_dir).unwrap();
        assert!(!snaps.is_empty());
        let copy = dbcrypt::open_db(&snaps[0].1, key.as_ref()).unwrap();
        let n: i64 = copy
            .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
        // Nothing but the snapshot itself is left in the directory.
        for e in std::fs::read_dir(&cfg.snapshot_dir).unwrap() {
            let name = e.unwrap().file_name().into_string().unwrap();
            assert!(parse_snapshot_name(&name).is_some(), "stray file {name}");
        }
    }
}
