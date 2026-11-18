//! Sidecar configuration, read from the environment (compose sets it).

use mitch_lib::dbcrypt::DbKey;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    /// The live database on the shared tmpfs.
    pub db_path: PathBuf,
    /// The pre-ramdisk database on the HDD, used once to seed an empty ramdisk.
    pub legacy_db: PathBuf,
    /// Where encrypted snapshots are written (HDD-backed).
    pub snapshot_dir: PathBuf,
    /// The old Litestream replica, which holds plaintext pages. Only touched by
    /// `scrub-plaintext`.
    pub replica_dir: PathBuf,
    /// The path the application itself was told to use (`MITCH_DB_PATH`), to
    /// refuse deleting the legacy file while the app still depends on it.
    pub app_db_path: Option<PathBuf>,
    /// Time between snapshot starts.
    pub interval: Duration,
    /// Owner of the files the application opens (the image's `app` user).
    pub owner_uid: u32,
    pub key: Option<DbKey>,
    pub ntfy: Ntfy,
}

#[derive(Debug, Clone, Default)]
pub struct Ntfy {
    pub topic: String,
    pub user: String,
    pub pass: String,
}

fn env_or(name: &str, default: &str) -> String {
    match std::env::var(name) {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => default.to_string(),
    }
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let interval: u64 = env_or("DBSYNC_INTERVAL_SECS", "60")
            .parse()
            .map_err(|_| "DBSYNC_INTERVAL_SECS must be a whole number of seconds".to_string())?;
        if interval == 0 {
            return Err("DBSYNC_INTERVAL_SECS must be at least 1".into());
        }
        let owner_uid: u32 = env_or("DBSYNC_OWNER_UID", "1000")
            .parse()
            .map_err(|_| "DBSYNC_OWNER_UID must be a number".to_string())?;
        Ok(Self {
            db_path: env_or("DBSYNC_DB_PATH", "/ramdisk/mitchpro.db").into(),
            legacy_db: env_or("DBSYNC_LEGACY_DB", "/legacy/mitchpro.db").into(),
            snapshot_dir: env_or("DBSYNC_SNAPSHOT_DIR", "/snapshots").into(),
            replica_dir: env_or("DBSYNC_REPLICA_DIR", "/replica").into(),
            app_db_path: std::env::var("MITCH_DB_PATH")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .map(PathBuf::from),
            interval: Duration::from_secs(interval),
            owner_uid,
            key: DbKey::from_env().map_err(|e| e.to_string())?,
            ntfy: Ntfy {
                topic: env_or("NTFY_TOPIC", ""),
                user: env_or("NTFY_USER", ""),
                pass: env_or("NTFY_PASS", ""),
            },
        })
    }
}
