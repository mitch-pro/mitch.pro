//! Emailed codes for resetting a forgotten admin passphrase. See
//! `docs/admin-panel.md` ("Forgotten passphrase").
//!
//! One row per admin in `admin_passphrase_resets`. Only a keyed hash of the
//! code is stored. A code is single use, expires after [`CODE_TTL_MS`], and
//! locks after [`MAX_ATTEMPTS`] wrong tries; asking for a new one is limited to
//! one per [`MIN_GAP_MS`]. The rules live here so no caller can skip them.

use crate::data::DataStore;

pub const CODE_TTL_MS: i64 = 15 * 60 * 1000;
pub const MIN_GAP_MS: i64 = 60 * 1000;
pub const MAX_ATTEMPTS: i64 = 5;

/// 8 characters without look-alikes (no 0/O/1/I): about 40 bits.
const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// A fresh random code, shown to the admin as `XXXX-XXXX`.
pub fn new_code() -> String {
    let bytes = crate::crypto::random_bytes_hex(8);
    let raw: Vec<u8> = (0..8)
        .map(|i| u8::from_str_radix(&bytes[i * 2..i * 2 + 2], 16).unwrap_or(0))
        .collect();
    let chars: String = raw
        .iter()
        .map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char)
        .collect();
    format!("{}-{}", &chars[..4], &chars[4..])
}

/// Upper-cases and drops spaces and dashes, so `abcd efgh` matches `ABCD-EFGH`.
pub fn normalize_code(input: &str) -> String {
    input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Keyed hash stored in place of the code.
pub fn code_hash(id_secret: &[u8], email: &str, code: &str) -> String {
    crate::crypto::hmac_sha256_hex(
        id_secret,
        format!("admin-passphrase-reset:{email}:{}", normalize_code(code)).as_bytes(),
    )
}

#[derive(Debug, PartialEq, Eq)]
pub enum Start {
    Started,
    /// A code was requested less than [`MIN_GAP_MS`] ago.
    TooSoon,
    Database,
}

/// Stores a new code for `email`, replacing any earlier one.
pub fn start(store: &DataStore, email: &str, hash: &str, now: i64) -> Start {
    let conn = store.conn();
    let last: Option<i64> = conn
        .query_row(
            "SELECT requested_at FROM admin_passphrase_resets WHERE email = ?1",
            rusqlite::params![email],
            |r| r.get(0),
        )
        .ok();
    if last.is_some_and(|t| now - t < MIN_GAP_MS) {
        return Start::TooSoon;
    }
    match conn.execute(
        "INSERT INTO admin_passphrase_resets (email, code_hash, requested_at, expires_at, attempts)
         VALUES (?1, ?2, ?3, ?4, 0)
         ON CONFLICT(email) DO UPDATE SET code_hash = ?2, requested_at = ?3, expires_at = ?4, attempts = 0",
        rusqlite::params![email, hash, now, now + CODE_TTL_MS],
    ) {
        Ok(_) => Start::Started,
        Err(_) => Start::Database,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Check {
    Ok,
    Wrong,
    TooManyAttempts,
    Expired,
    /// No code was requested.
    Nothing,
}

/// Checks `hash` against the stored code. A wrong try is counted; a correct
/// code is consumed so it cannot be used twice.
pub fn check(store: &DataStore, email: &str, hash: &str, now: i64) -> Check {
    let conn = store.conn();
    let row: Option<(String, i64, i64)> = conn
        .query_row(
            "SELECT code_hash, expires_at, attempts FROM admin_passphrase_resets WHERE email = ?1",
            rusqlite::params![email],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();
    let Some((stored, expires, attempts)) = row else {
        return Check::Nothing;
    };
    if attempts >= MAX_ATTEMPTS {
        return Check::TooManyAttempts;
    }
    if expires < now {
        let _ = conn.execute(
            "DELETE FROM admin_passphrase_resets WHERE email = ?1",
            rusqlite::params![email],
        );
        return Check::Expired;
    }
    if !crate::crypto::timing_safe_equal(stored.as_bytes(), hash.as_bytes()) {
        let _ = conn.execute(
            "UPDATE admin_passphrase_resets SET attempts = attempts + 1 WHERE email = ?1",
            rusqlite::params![email],
        );
        return Check::Wrong;
    }
    let _ = conn.execute(
        "DELETE FROM admin_passphrase_resets WHERE email = ?1",
        rusqlite::params![email],
    );
    Check::Ok
}

/// Drops codes that expired more than a day ago.
pub fn prune(store: &DataStore, now: i64) {
    let _ = store.conn().execute(
        "DELETE FROM admin_passphrase_resets WHERE expires_at < ?1",
        rusqlite::params![now - 24 * 60 * 60 * 1000],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-passphrase-reset-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn codes_look_right_and_normalize() {
        let code = new_code();
        assert_eq!(code.len(), 9);
        assert_eq!(&code[4..5], "-");
        assert_eq!(normalize_code("abcd efgh"), "ABCDEFGH");
        assert_eq!(normalize_code(&code), code.replace('-', ""));
        assert_eq!(
            code_hash(b"k", "a@b.c", "abcd-efgh"),
            code_hash(b"k", "a@b.c", "ABCDEFGH")
        );
        assert_ne!(
            code_hash(b"k", "a@b.c", "ABCDEFGH"),
            code_hash(b"k", "x@b.c", "ABCDEFGH")
        );
    }

    #[test]
    fn a_code_works_once_and_only_for_its_admin() {
        let store = temp_store("once");
        let h = code_hash(b"k", "a@b.c", "ABCDEFGH");
        assert_eq!(check(&store, "a@b.c", &h, 1), Check::Nothing);
        assert_eq!(start(&store, "a@b.c", &h, 1_000), Start::Started);
        assert_eq!(check(&store, "x@b.c", &h, 2_000), Check::Nothing);
        assert_eq!(check(&store, "a@b.c", &h, 2_000), Check::Ok);
        assert_eq!(check(&store, "a@b.c", &h, 3_000), Check::Nothing);
    }

    #[test]
    fn requests_are_spaced_and_codes_expire() {
        let store = temp_store("gap");
        let h = code_hash(b"k", "a@b.c", "ABCDEFGH");
        assert_eq!(start(&store, "a@b.c", &h, 1_000), Start::Started);
        assert_eq!(start(&store, "a@b.c", &h, 1_000 + 5_000), Start::TooSoon);
        assert_eq!(
            check(&store, "a@b.c", &h, 1_000 + CODE_TTL_MS + 1),
            Check::Expired
        );
        assert_eq!(
            start(&store, "a@b.c", &h, 1_000 + MIN_GAP_MS),
            Start::Started
        );
    }

    #[test]
    fn five_wrong_tries_lock_the_code_even_for_the_right_one() {
        let store = temp_store("lock");
        let good = code_hash(b"k", "a@b.c", "ABCDEFGH");
        let bad = code_hash(b"k", "a@b.c", "ZZZZZZZZ");
        assert_eq!(start(&store, "a@b.c", &good, 1_000), Start::Started);
        for _ in 0..MAX_ATTEMPTS {
            assert_eq!(check(&store, "a@b.c", &bad, 2_000), Check::Wrong);
        }
        assert_eq!(check(&store, "a@b.c", &good, 2_000), Check::TooManyAttempts);
        // A new code starts fresh.
        assert_eq!(
            start(&store, "a@b.c", &good, 1_000 + MIN_GAP_MS),
            Start::Started
        );
        assert_eq!(check(&store, "a@b.c", &good, 2_000 + MIN_GAP_MS), Check::Ok);
    }
}
