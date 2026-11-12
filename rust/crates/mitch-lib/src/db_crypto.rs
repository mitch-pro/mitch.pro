//! Whole-database encryption (SQLCipher) for `mitchpro.db`.
//! See `docs/database-encryption.md`.
//!
//! Turned on with `DB_ENCRYPTION=1`. The key is derived from the master key in
//! [`crate::keyring`]. An encrypted database needs the key to be opened by
//! everything that touches it: the web server, mail-rs, and any tool.
//!
//! The file is never converted behind a running service's back: with encryption
//! on, opening a plaintext database is an error that points at the explicit
//! `encrypt-db` command ([`encrypt_file`]), which writes a new file and checks it
//! before anything is swapped in.
//!
//! Litestream can't replicate an encrypted database, so an encrypted database is
//! backed up by periodic encrypted snapshots ([`snapshot`], pruned by
//! [`prune_snapshots`]); [`restore_latest`] brings back the newest one when the
//! database file is missing, e.g. after a ramdisk is cleared.

use crate::keyring::KeyRing;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// Purpose string for the database key derived from the master key.
pub const PURPOSE: &str = "database";

const PLAINTEXT_HEADER: &[u8; 16] = b"SQLite format 3\0";
const DAY_MS: i64 = 86_400_000;

#[derive(Debug)]
pub enum DbCryptoError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    /// Encryption is on but the file is a plaintext database.
    NeedsMigration(PathBuf),
    /// The file is encrypted (or not a database) and no ring key opens it.
    Unreadable(PathBuf),
    /// Encryption is off but the file is encrypted.
    EncryptedWithoutKey(PathBuf),
    /// A copy didn't match its source.
    Mismatch(String),
}

impl std::fmt::Display for DbCryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbCryptoError::Sqlite(e) => write!(f, "sqlite: {e}"),
            DbCryptoError::Io(e) => write!(f, "io: {e}"),
            DbCryptoError::NeedsMigration(p) => write!(
                f,
                "DB_ENCRYPTION is on but {} is a plaintext database. Stop every service that uses it, then run `mitch-server encrypt-db` (docs/database-encryption.md)",
                p.display()
            ),
            DbCryptoError::Unreadable(p) => write!(
                f,
                "{} can't be opened with the data encryption key (wrong key, or not a database)",
                p.display()
            ),
            DbCryptoError::EncryptedWithoutKey(p) => write!(
                f,
                "{} is encrypted but DB_ENCRYPTION is not on. Set DB_ENCRYPTION=1 with the key, or run `mitch-server decrypt-db`",
                p.display()
            ),
            DbCryptoError::Mismatch(m) => write!(f, "verification failed: {m}"),
        }
    }
}

impl std::error::Error for DbCryptoError {}

impl From<rusqlite::Error> for DbCryptoError {
    fn from(e: rusqlite::Error) -> Self {
        DbCryptoError::Sqlite(e)
    }
}

impl From<std::io::Error> for DbCryptoError {
    fn from(e: std::io::Error) -> Self {
        DbCryptoError::Io(e)
    }
}

/// Whether `DB_ENCRYPTION` asks for an encrypted database.
pub fn enabled() -> bool {
    std::env::var("DB_ENCRYPTION")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "on" | "true" | "yes"
            )
        })
        .unwrap_or(false)
}

fn hex(key: &[u8; 32]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

/// The SQLCipher raw-key form, which skips its passphrase hashing.
fn raw_key(key: &[u8; 32]) -> String {
    format!("x'{}'", hex(key))
}

fn sql_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Applies `key` (it must be the first thing done on the connection) and
/// reports whether the database can then be read.
fn set_key(conn: &Connection, key: &[u8; 32]) -> Result<bool, rusqlite::Error> {
    conn.pragma_update(None, "key", raw_key(key))?;
    Ok(conn
        .query_row("SELECT count(*) FROM sqlite_master", [], |r| {
            r.get::<_, i64>(0)
        })
        .is_ok())
}

/// A file starting with the standard SQLite header is plaintext. An empty or
/// missing file is neither.
pub fn is_plaintext_file(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 16];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && &head == PLAINTEXT_HEADER
}

fn non_empty(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.len() > 0)
        .unwrap_or(false)
}

/// Opens the database at `path`. With a ring, the file is encrypted (a new file
/// starts encrypted). A file encrypted under an older key of the ring is
/// re-keyed to the primary key. Without a ring the file must be plaintext.
pub fn open(path: &Path, ring: Option<&KeyRing>) -> Result<Connection, DbCryptoError> {
    let Some(ring) = ring else {
        if non_empty(path) && !is_plaintext_file(path) {
            return Err(DbCryptoError::EncryptedWithoutKey(path.to_path_buf()));
        }
        return Ok(Connection::open(path)?);
    };
    if non_empty(path) && is_plaintext_file(path) {
        return Err(DbCryptoError::NeedsMigration(path.to_path_buf()));
    }
    for (index, key) in ring.derive_keys(PURPOSE).iter().enumerate() {
        let conn = Connection::open(path)?;
        if !set_key(&conn, key)? {
            continue;
        }
        if index > 0 {
            // Written under a previous key. SQLCipher can't re-key a WAL database.
            conn.pragma_update(None, "journal_mode", "DELETE")?;
            conn.pragma_update(None, "rekey", raw_key(&ring.derive_key(PURPOSE)))?;
        }
        return Ok(conn);
    }
    Err(DbCryptoError::Unreadable(path.to_path_buf()))
}

/// Names of the user tables in a database.
fn table_names(conn: &Connection) -> Result<Vec<String>, rusqlite::Error> {
    conn.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?
    .query_map([], |r| r.get(0))?
    .collect()
}

fn row_counts(conn: &Connection) -> Result<Vec<(String, i64)>, rusqlite::Error> {
    table_names(conn)?
        .into_iter()
        .map(|t| {
            let n = conn.query_row(&format!("SELECT count(*) FROM \"{t}\""), [], |r| r.get(0))?;
            Ok((t, n))
        })
        .collect()
}

/// Copies every table of `src` into a new database `dst` that has `dst_key`
/// (or is plaintext when `None`), then checks the copy: same tables, same row
/// counts, and it opens. `src_key` is `None` for a plaintext source. Refuses
/// when `dst` exists, and when `src` looks like it is in use.
fn convert(
    src: &Path,
    src_key: Option<&[u8; 32]>,
    dst: &Path,
    dst_key: Option<&[u8; 32]>,
) -> Result<Vec<(String, i64)>, DbCryptoError> {
    if dst.exists() {
        return Err(DbCryptoError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} already exists", dst.display()),
        )));
    }
    for suffix in ["-wal", "-shm"] {
        let side = PathBuf::from(format!("{}{suffix}", src.display()));
        if non_empty(&side) {
            return Err(DbCryptoError::Io(std::io::Error::other(format!(
                "{} exists, so the database looks in use: stop every service that uses it first",
                side.display()
            ))));
        }
    }
    let result = (|| {
        let conn = Connection::open(src)?;
        if let Some(key) = src_key {
            if !set_key(&conn, key)? {
                return Err(DbCryptoError::Unreadable(src.to_path_buf()));
            }
        }
        let expected = row_counts(&conn)?;
        let user_version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let target_key = dst_key.map(raw_key).unwrap_or_default();
        conn.execute_batch(&format!(
            "ATTACH DATABASE {} AS converted KEY {};",
            sql_quote(&dst.to_string_lossy()),
            sql_quote(&target_key)
        ))?;
        conn.query_row("SELECT sqlcipher_export('converted')", [], |_| Ok(()))?;
        conn.execute_batch(&format!(
            "PRAGMA converted.user_version = {user_version}; DETACH DATABASE converted;"
        ))?;
        drop(conn);

        let check = Connection::open(dst)?;
        if let Some(key) = dst_key {
            if !set_key(&check, key)? {
                return Err(DbCryptoError::Mismatch("copy can't be opened".into()));
            }
        }
        let actual = row_counts(&check)?;
        if actual != expected {
            return Err(DbCryptoError::Mismatch(format!(
                "row counts differ: {expected:?} vs {actual:?}"
            )));
        }
        let quick: String = check.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if quick != "ok" {
            return Err(DbCryptoError::Mismatch(format!("quick_check: {quick}")));
        }
        Ok(actual)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(dst);
    }
    result
}

/// Writes an encrypted copy of the plaintext database `src` to `dst` and
/// verifies it. Returns the per-table row counts. Does not touch `src`.
pub fn encrypt_file(
    src: &Path,
    dst: &Path,
    ring: &KeyRing,
) -> Result<Vec<(String, i64)>, DbCryptoError> {
    if non_empty(src) && !is_plaintext_file(src) {
        return Err(DbCryptoError::Io(std::io::Error::other(format!(
            "{} is not a plaintext database",
            src.display()
        ))));
    }
    convert(src, None, dst, Some(&ring.derive_key(PURPOSE)))
}

/// The reverse of [`encrypt_file`], for rolling back. Tries every key in the ring.
pub fn decrypt_file(
    src: &Path,
    dst: &Path,
    ring: &KeyRing,
) -> Result<Vec<(String, i64)>, DbCryptoError> {
    let mut last = DbCryptoError::Unreadable(src.to_path_buf());
    for key in ring.derive_keys(PURPOSE) {
        match convert(src, Some(&key), dst, None) {
            Err(DbCryptoError::Unreadable(p)) => last = DbCryptoError::Unreadable(p),
            other => return other,
        }
    }
    Err(last)
}

/// Writes a consistent snapshot of the database at `db_path` into `dir` as
/// `mitchpro-<ms>.db`, encrypted with `key` when given. It reads through its own
/// connection in one transaction, so the application's writes aren't blocked.
pub fn snapshot(
    db_path: &Path,
    key: Option<&[u8; 32]>,
    dir: &Path,
    now_ms: i64,
) -> Result<PathBuf, DbCryptoError> {
    std::fs::create_dir_all(dir)?;
    let name = format!("mitchpro-{now_ms:015}.db");
    let target = dir.join(&name);
    let tmp = dir.join(format!("{name}.partial"));
    let _ = std::fs::remove_file(&tmp);
    let result = (|| {
        let src = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        if let Some(key) = key {
            if !set_key(&src, key)? {
                return Err(DbCryptoError::Unreadable(db_path.to_path_buf()));
            }
        }
        let mut dst = Connection::open(&tmp)?;
        if let Some(key) = key {
            set_key(&dst, key)?;
        }
        {
            let backup = rusqlite::backup::Backup::new(&src, &mut dst)?;
            // All pages in one step: one read transaction, so a concurrent writer
            // can't force the copy to restart.
            match backup.step(-1)? {
                rusqlite::backup::StepResult::Done => {}
                other => {
                    return Err(DbCryptoError::Mismatch(format!(
                        "backup did not finish: {other:?}"
                    )))
                }
            }
        }
        drop(dst);
        std::fs::rename(&tmp, &target)?;
        Ok(target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn snapshot_time(path: &Path) -> Option<i64> {
    path.file_name()?
        .to_str()?
        .strip_prefix("mitchpro-")?
        .strip_suffix(".db")?
        .parse()
        .ok()
}

fn snapshots_newest_first(dir: &Path) -> Vec<(i64, PathBuf)> {
    let mut found: Vec<(i64, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| snapshot_time(&e.path()).map(|t| (t, e.path())))
        .collect();
    found.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
    found
}

/// Keeps the newest `keep` snapshots plus the first snapshot of each UTC day for
/// `daily_days` days, and deletes the rest. Returns how many were deleted.
pub fn prune_snapshots(dir: &Path, keep: usize, daily_days: i64, now_ms: i64) -> usize {
    let all = snapshots_newest_first(dir);
    let oldest_daily = now_ms - daily_days * DAY_MS;
    let mut first_of_day: std::collections::BTreeMap<i64, i64> = std::collections::BTreeMap::new();
    for (t, _) in &all {
        let entry = first_of_day.entry(t / DAY_MS).or_insert(*t);
        *entry = (*entry).min(*t);
    }
    let mut removed = 0;
    for (index, (t, path)) in all.iter().enumerate() {
        let kept =
            index < keep || (*t >= oldest_daily && first_of_day.get(&(t / DAY_MS)) == Some(t));
        if !kept && std::fs::remove_file(path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// When `db_path` doesn't exist, copies the newest snapshot in `dir` that opens
/// with the ring's key into place. Returns the snapshot used.
pub fn restore_latest(
    dir: &Path,
    db_path: &Path,
    ring: &KeyRing,
) -> Result<Option<PathBuf>, DbCryptoError> {
    if db_path.exists() {
        return Ok(None);
    }
    for (_, path) in snapshots_newest_first(dir) {
        let Ok(conn) = open(&path, Some(ring)) else {
            continue;
        };
        let ok = conn
            .query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))
            .is_ok_and(|v| v == "ok");
        drop(conn);
        if !ok {
            continue;
        }
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = db_path.with_extension("db.restoring");
        std::fs::copy(&path, &tmp)?;
        std::fs::rename(&tmp, db_path)?;
        return Ok(Some(path));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "dbcrypto-test-{}",
            crate::crypto::random_bytes_hex(8)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn plaintext_db(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE people(email TEXT PRIMARY KEY, note TEXT);
             INSERT INTO people VALUES('a@example.com','needle-5521'),('b@example.com','x');
             CREATE TABLE empty_one(id INTEGER);
             PRAGMA user_version = 7;",
        )
        .unwrap();
    }

    #[test]
    fn encrypt_then_decrypt_round_trips() {
        let d = dir();
        let ring = KeyRing::for_tests([9; 32]);
        let plain = d.join("plain.db");
        let enc = d.join("enc.db");
        plaintext_db(&plain);

        let counts = encrypt_file(&plain, &enc, &ring).unwrap();
        assert_eq!(counts, vec![("empty_one".into(), 0), ("people".into(), 2)]);
        assert!(!is_plaintext_file(&enc));
        let raw = std::fs::read(&enc).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("needle-5521"));
        // Without the key it isn't a database; with it, the data and user_version are there.
        assert!(matches!(
            open(&enc, None),
            Err(DbCryptoError::EncryptedWithoutKey(_))
        ));
        let conn = open(&enc, Some(&ring)).unwrap();
        let note: String = conn
            .query_row(
                "SELECT note FROM people WHERE email='a@example.com'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(note, "needle-5521");
        let uv: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(uv, 7);
        drop(conn);
        // Another key can't open it.
        assert!(matches!(
            open(&enc, Some(&KeyRing::for_tests([1; 32]))),
            Err(DbCryptoError::Unreadable(_))
        ));

        let back = d.join("back.db");
        decrypt_file(&enc, &back, &ring).unwrap();
        assert!(is_plaintext_file(&back));
        assert_eq!(
            row_counts(&Connection::open(&back).unwrap()).unwrap(),
            vec![("empty_one".into(), 0), ("people".into(), 2)]
        );
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn plaintext_is_refused_when_encryption_is_on() {
        let d = dir();
        let plain = d.join("p.db");
        plaintext_db(&plain);
        assert!(matches!(
            open(&plain, Some(&KeyRing::for_tests([2; 32]))),
            Err(DbCryptoError::NeedsMigration(_))
        ));
        // A new database starts encrypted.
        let fresh = d.join("new.db");
        let ring = KeyRing::for_tests([2; 32]);
        let conn = open(&fresh, Some(&ring)).unwrap();
        conn.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES(1);")
            .unwrap();
        drop(conn);
        assert!(!is_plaintext_file(&fresh));
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn conversion_refuses_an_in_use_or_existing_target() {
        let d = dir();
        let ring = KeyRing::for_tests([4; 32]);
        let plain = d.join("p.db");
        plaintext_db(&plain);
        std::fs::write(d.join("p.db-wal"), b"x").unwrap();
        assert!(encrypt_file(&plain, &d.join("o.db"), &ring).is_err());
        std::fs::remove_file(d.join("p.db-wal")).unwrap();
        std::fs::write(d.join("o.db"), b"x").unwrap();
        assert!(encrypt_file(&plain, &d.join("o.db"), &ring).is_err());
        assert_eq!(
            std::fs::read(d.join("o.db")).unwrap(),
            b"x",
            "existing file untouched"
        );
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn rotation_rekeys_to_the_new_primary() {
        let d = dir();
        let old = KeyRing::for_tests([5; 32]);
        let path = d.join("db.db");
        let conn = open(&path, Some(&old)).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES(42);")
            .unwrap();
        drop(conn);

        // A ring whose primary is new and which still knows the old key.
        let ring = KeyRing::for_tests_with_previous([6; 32], [5; 32]);
        let conn = open(&path, Some(&ring)).unwrap();
        let x: i64 = conn.query_row("SELECT x FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(x, 42);
        drop(conn);
        // Now only the new key opens it.
        assert!(open(&path, Some(&KeyRing::for_tests([5; 32]))).is_err());
        assert!(open(&path, Some(&KeyRing::for_tests([6; 32]))).is_ok());
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn snapshots_restore_and_prune() {
        let d = dir();
        let ring = KeyRing::for_tests([8; 32]);
        let db = d.join("live").join("mitchpro.db");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        let conn = open(&db, Some(&ring)).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES(1);")
            .unwrap();

        let snaps = d.join("snaps");
        let key = ring.derive_key(PURPOSE);
        let first = snapshot(&db, Some(&key), &snaps, 1_000).unwrap();
        // The live connection keeps working and later snapshots see later rows.
        conn.execute_batch("INSERT INTO t VALUES(2);").unwrap();
        let second = snapshot(&db, Some(&key), &snaps, 2_000).unwrap();
        assert!(!is_plaintext_file(&first) && !is_plaintext_file(&second));
        drop(conn);

        // Lose the live database; the newest snapshot comes back.
        std::fs::remove_dir_all(db.parent().unwrap()).unwrap();
        let restored = restore_latest(&snaps, &db, &ring).unwrap();
        assert_eq!(restored.as_deref(), Some(second.as_path()));
        let conn = open(&db, Some(&ring)).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
        // An existing database is never replaced.
        assert_eq!(restore_latest(&snaps, &db, &ring).unwrap(), None);

        // Pruning keeps the newest N plus the first of each recent day.
        let now = 40 * DAY_MS;
        for i in 0..6 {
            snapshot(&db, Some(&key), &snaps, 30 * DAY_MS + i * 1_000).unwrap();
        }
        snapshot(&db, Some(&key), &snaps, 39 * DAY_MS).unwrap();
        snapshot(&db, Some(&key), &snaps, 39 * DAY_MS + 5_000).unwrap();
        let before = snapshots_newest_first(&snaps).len();
        let removed = prune_snapshots(&snaps, 2, 5, now);
        let left = snapshots_newest_first(&snaps);
        assert_eq!(before - removed, left.len());
        assert!(
            left.iter().any(|(t, _)| *t == 39 * DAY_MS),
            "first snapshot of a recent day kept"
        );
        assert!(
            left.iter().any(|(t, _)| *t == 39 * DAY_MS + 5_000),
            "newest kept"
        );
        assert!(
            !left.iter().any(|(t, _)| *t == 30 * DAY_MS + 3_000),
            "old extras pruned"
        );
        assert!(
            !left.iter().any(|(t, _)| *t == 30 * DAY_MS),
            "older than the daily window pruned"
        );
        std::fs::remove_dir_all(d).unwrap();
    }
}
