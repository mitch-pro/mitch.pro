//! SQLCipher helpers for `mitchpro.db`: key handling, keyed open, and the
//! snapshot/export primitives the `mitch-dbsync` sidecar is built on.
//! Operations guide: `docs/database-encryption.md`.
//!
//! The key is `MITCH_DB_KEY`: 64 hex characters (32 random bytes), passed to
//! SQLCipher as a raw key so opening is fast. With no key set everything here
//! works on plain SQLite files, which keeps local development and the test
//! suite unchanged.
//!
//! Safety rails ([`open_db`]): a key with a plaintext file, or an encrypted
//! file with no key, is refused with an explanatory error. Opening it anyway
//! would either fail obscurely or, worse, create a second database next to the
//! real one.

use rusqlite::{backup::Backup, Connection};
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Environment variable holding the database key.
pub const KEY_ENV: &str = "MITCH_DB_KEY";

/// First 16 bytes of every plain SQLite file. An encrypted file starts with
/// random-looking salt instead.
const PLAIN_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// A validated database key. `Debug` never prints the value.
#[derive(Clone, PartialEq, Eq)]
pub struct DbKey(String);

impl fmt::Debug for DbKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DbKey(<redacted>)")
    }
}

/// What a file on disk looks like, judged by its first bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// Missing or zero length: SQLite will create it fresh.
    Missing,
    /// A plain SQLite database.
    Plain,
    /// Anything else, which for this application means SQLCipher.
    Encrypted,
}

#[derive(Debug)]
pub enum DbCryptError {
    BadKey(String),
    /// A key is configured but the file is a plain database.
    PlainWithKey(PathBuf),
    /// The file is encrypted but no key is configured.
    EncryptedWithoutKey(PathBuf),
    /// The key doesn't decrypt the file (or the file is damaged).
    Unreadable(PathBuf, rusqlite::Error),
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    /// A produced copy didn't match its source.
    Verify(String),
}

impl fmt::Display for DbCryptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadKey(why) => write!(f, "{KEY_ENV} is invalid: {why}"),
            Self::PlainWithKey(p) => write!(
                f,
                "{} is a plain database but {KEY_ENV} is set; encrypt it first \
                 (docs/database-encryption.md, `mitch-dbsync migrate`)",
                p.display()
            ),
            Self::EncryptedWithoutKey(p) => {
                write!(f, "{} is encrypted but {KEY_ENV} is not set", p.display())
            }
            Self::Unreadable(p, e) => write!(
                f,
                "cannot read {} (wrong {KEY_ENV} or damaged file): {e}",
                p.display()
            ),
            Self::Sqlite(e) => write!(f, "sqlite: {e}"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Verify(why) => write!(f, "verification failed: {why}"),
        }
    }
}

impl std::error::Error for DbCryptError {}

impl From<rusqlite::Error> for DbCryptError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

impl From<std::io::Error> for DbCryptError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl DbKey {
    /// Accepts exactly 64 hex characters; surrounding whitespace is ignored.
    pub fn parse(s: &str) -> Result<Self, DbCryptError> {
        let s = s.trim();
        if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(DbCryptError::BadKey(
                "expected 64 hex characters (generate with `openssl rand -hex 32`)".into(),
            ));
        }
        Ok(Self(s.to_ascii_lowercase()))
    }

    /// `Ok(None)` when [`KEY_ENV`] is unset or empty.
    pub fn from_env() -> Result<Option<Self>, DbCryptError> {
        match std::env::var(KEY_ENV) {
            Ok(v) if !v.trim().is_empty() => Self::parse(&v).map(Some),
            _ => Ok(None),
        }
    }

    /// First 8 hex characters of SHA-256 of the key: safe to log, lets an
    /// operator tell two keys apart.
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(self.0.as_bytes());
        digest.iter().take(4).map(|b| format!("{b:02x}")).collect()
    }
}

/// Classifies `path` by its first bytes.
pub fn file_kind(path: &Path) -> std::io::Result<FileKind> {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(FileKind::Missing),
        Err(e) => return Err(e),
    };
    let mut head = [0u8; 16];
    let mut got = 0;
    while got < head.len() {
        let n = file.read(&mut head[got..])?;
        if n == 0 {
            break;
        }
        got += n;
    }
    Ok(match got {
        0 => FileKind::Missing,
        16 if &head == PLAIN_HEADER => FileKind::Plain,
        _ => FileKind::Encrypted,
    })
}

/// Applies `key` to a fresh connection. Must be the first statement run on it.
pub fn apply_key(conn: &Connection, key: &DbKey) -> rusqlite::Result<()> {
    // The key is validated hex, so interpolating it cannot inject SQL. A
    // PRAGMA can't take a bound parameter.
    conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", key.0))
}

/// Reads the schema table, which fails with "file is not a database" if the
/// key is wrong. SQLCipher doesn't notice a bad key until the first read.
fn prove_readable(conn: &Connection) -> rusqlite::Result<()> {
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
        r.get::<_, i64>(0)
    })
    .map(|_| ())
}

/// Opens (or creates) the database at `path`, keyed when `key` is given.
/// Enforces the key/file agreement described in the module docs.
pub fn open_db(path: &Path, key: Option<&DbKey>) -> Result<Connection, DbCryptError> {
    match (file_kind(path)?, key) {
        (FileKind::Plain, Some(_)) => return Err(DbCryptError::PlainWithKey(path.into())),
        (FileKind::Encrypted, None) => return Err(DbCryptError::EncryptedWithoutKey(path.into())),
        _ => {}
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_millis(15_000))?;
    if let Some(key) = key {
        apply_key(&conn, key)?;
    }
    prove_readable(&conn).map_err(|e| DbCryptError::Unreadable(path.into(), e))?;
    Ok(conn)
}

/// `PRAGMA quick_check` must report exactly `ok`.
pub fn quick_check(conn: &Connection) -> Result<(), DbCryptError> {
    let rows: Vec<String> = conn
        .prepare("PRAGMA quick_check")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<_, _>>()?;
    if rows.len() == 1 && rows[0] == "ok" {
        Ok(())
    } else {
        Err(DbCryptError::Verify(format!(
            "quick_check: {}",
            rows.join("; ")
        )))
    }
}

/// Removes `<path>-wal` and `<path>-shm`, ignoring "not found".
pub fn remove_sidecars(path: &Path) -> std::io::Result<()> {
    for suffix in ["-wal", "-shm"] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        match std::fs::remove_file(PathBuf::from(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Copies a consistent snapshot of the database at `src` to `dst`, encrypted
/// with the same key (or plain when `key` is `None`), and checks the copy.
///
/// Uses SQLite's online-backup API in a single step, so a WAL-mode database
/// being written by other processes is copied as of one moment. `dst` is
/// replaced if present, and is left as a single self-contained file (no
/// `-wal`/`-shm`). The caller renames it into place.
pub fn snapshot(src: &Path, dst: &Path, key: Option<&DbKey>) -> Result<(), DbCryptError> {
    let src_conn = open_db(src, key)?;
    let _ = std::fs::remove_file(dst);
    remove_sidecars(dst)?;
    {
        let mut dst_conn = Connection::open(dst)?;
        if let Some(key) = key {
            apply_key(&dst_conn, key)?;
        }
        // i32::MAX pages per step: everything in one step, one read transaction.
        Backup::new(&src_conn, &mut dst_conn)?.run_to_completion(
            i32::MAX,
            Duration::from_millis(0),
            None,
        )?;
        // The backup copies the source's WAL flag. A snapshot is a cold file:
        // rollback-journal mode means opening it never creates -wal/-shm files
        // next to it. The application switches back to WAL when it opens it.
        dst_conn.query_row("PRAGMA journal_mode = DELETE", [], |_| Ok(()))?;
        quick_check(&dst_conn)?;
    }
    remove_sidecars(dst)?;
    std::fs::File::open(dst)?.sync_all()?;
    Ok(())
}

/// Verifies that the file at `path` opens with `key` and passes `quick_check`.
pub fn verify_file(path: &Path, key: Option<&DbKey>) -> Result<(), DbCryptError> {
    quick_check(&open_db(path, key)?)
}

fn table_counts(conn: &Connection) -> Result<Vec<(String, i64)>, DbCryptError> {
    let names: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<_, _>>()?;
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let n: i64 = conn.query_row(
            &format!("SELECT count(*) FROM \"{}\"", name.replace('"', "\"\"")),
            [],
            |r| r.get(0),
        )?;
        out.push((name, n));
    }
    Ok(out)
}

/// Encrypts the plain database at `src` into a new file `dst` (`sqlcipher_export`),
/// then proves the copy: it must open with `key`, pass `quick_check`, and hold
/// the same tables with the same row counts. `src` is not modified beyond a
/// WAL checkpoint, so run this with every writer stopped.
pub fn encrypt_plain_db(src: &Path, dst: &Path, key: &DbKey) -> Result<(), DbCryptError> {
    if file_kind(src)? != FileKind::Plain {
        return Err(DbCryptError::Verify(format!(
            "{} is not a plain SQLite database",
            src.display()
        )));
    }
    let _ = std::fs::remove_file(dst);
    remove_sidecars(dst)?;
    {
        let plain = Connection::open(src)?;
        plain.busy_timeout(Duration::from_millis(15_000))?;
        plain.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        let user_version: i64 = plain.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let dst_sql = dst.to_string_lossy().replace('\'', "''");
        plain.execute_batch(&format!(
            "ATTACH DATABASE '{dst_sql}' AS enc KEY \"x'{}'\";\n\
             SELECT sqlcipher_export('enc');\n\
             PRAGMA enc.user_version = {user_version};\n\
             DETACH DATABASE enc;",
            key.0
        ))?;
    }
    let plain = Connection::open(src)?;
    let enc = open_db(dst, Some(key))?;
    quick_check(&enc)?;
    let (before, after) = (table_counts(&plain)?, table_counts(&enc)?);
    if before != after {
        return Err(DbCryptError::Verify(format!(
            "row counts differ after export: {before:?} vs {after:?}"
        )));
    }
    drop(enc);
    remove_sidecars(dst)?;
    std::fs::File::open(dst)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
    const KEY_B: &str = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "mitch-dbcrypt-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn fill(path: &Path, key: Option<&DbKey>) {
        let conn = open_db(path, key).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT);
             INSERT INTO t (v) VALUES ('secret-marker-1'), ('secret-marker-2');
             PRAGMA user_version = 7;",
        )
        .unwrap();
    }

    #[test]
    fn key_parsing() {
        assert!(DbKey::parse(KEY_A).is_ok());
        assert!(DbKey::parse(&format!("  {KEY_A}\n")).is_ok());
        assert!(DbKey::parse("short").is_err());
        assert!(DbKey::parse(&"g".repeat(64)).is_err());
        assert_eq!(
            format!("{:?}", DbKey::parse(KEY_A).unwrap()),
            "DbKey(<redacted>)"
        );
        assert_ne!(
            DbKey::parse(KEY_A).unwrap().fingerprint(),
            DbKey::parse(KEY_B).unwrap().fingerprint()
        );
    }

    #[test]
    fn encrypted_file_has_no_plaintext_and_needs_the_key() {
        let d = dir("enc");
        let path = d.join("a.db");
        let key = DbKey::parse(KEY_A).unwrap();
        fill(&path, Some(&key));
        // Force the WAL into the main file so the check covers everything.
        open_db(&path, Some(&key))
            .unwrap()
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.windows(13).any(|w| w == b"secret-marker"));
        assert_eq!(file_kind(&path).unwrap(), FileKind::Encrypted);

        assert!(matches!(
            open_db(&path, None),
            Err(DbCryptError::EncryptedWithoutKey(_))
        ));
        let other = DbKey::parse(KEY_B).unwrap();
        assert!(matches!(
            open_db(&path, Some(&other)),
            Err(DbCryptError::Unreadable(..))
        ));
    }

    #[test]
    fn plain_file_with_key_is_refused() {
        let d = dir("plainkey");
        let path = d.join("a.db");
        fill(&path, None);
        let key = DbKey::parse(KEY_A).unwrap();
        assert!(matches!(
            open_db(&path, Some(&key)),
            Err(DbCryptError::PlainWithKey(_))
        ));
    }

    #[test]
    fn snapshot_round_trips_encrypted_and_plain() {
        let d = dir("snap");
        for key in [Some(DbKey::parse(KEY_A).unwrap()), None] {
            let src = d.join("src.db");
            let dst = d.join("dst.db");
            let _ = std::fs::remove_file(&src);
            fill(&src, key.as_ref());
            // The source keeps a live connection, like the running server.
            let live = open_db(&src, key.as_ref()).unwrap();
            snapshot(&src, &dst, key.as_ref()).unwrap();
            live.execute("INSERT INTO t (v) VALUES ('later')", [])
                .unwrap();
            assert!(!d.join("dst.db-wal").exists());
            assert!(!d.join("dst.db-shm").exists());

            let copy = open_db(&dst, key.as_ref()).unwrap();
            let n: i64 = copy
                .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 2, "snapshot is a point-in-time copy");
            if key.is_some() {
                assert!(!std::fs::read(&dst)
                    .unwrap()
                    .windows(13)
                    .any(|w| w == b"secret-marker"));
            }
        }
    }

    #[test]
    fn encrypt_plain_db_preserves_everything() {
        let d = dir("migrate");
        let (src, dst) = (d.join("plain.db"), d.join("enc.db"));
        fill(&src, None);
        let key = DbKey::parse(KEY_A).unwrap();
        encrypt_plain_db(&src, &dst, &key).unwrap();

        let enc = open_db(&dst, Some(&key)).unwrap();
        let uv: i64 = enc
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(uv, 7);
        assert_eq!(table_counts(&enc).unwrap(), vec![("t".to_string(), 2)]);
        // Encrypting something already encrypted is an error, not a no-op.
        assert!(encrypt_plain_db(&dst, &d.join("x.db"), &key).is_err());
    }

    #[test]
    fn missing_and_empty_files_are_missing() {
        let d = dir("kind");
        assert_eq!(file_kind(&d.join("nope")).unwrap(), FileKind::Missing);
        std::fs::write(d.join("empty"), b"").unwrap();
        assert_eq!(file_kind(&d.join("empty")).unwrap(), FileKind::Missing);
    }
}
