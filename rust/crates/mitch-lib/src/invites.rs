//! The peer-referral invite system's data layer — real `invite_codes`/
//! `invite_claims`/`invite_sent` tables instead of the old
//! invite_codes.json/invite_claims.json/invite_sent.json blobs.
//!
//! The old claim flow was a read-the-whole-blob, check-if-already-claimed,
//! write-the-whole-blob-back — the exact lost-update race the coins
//! migration fixed, but here a race pays out the 2000-coin referral bonus
//! twice instead of just corrupting a balance. `invite_claim_insert_if_new`
//! closes that with one atomic `INSERT OR IGNORE`. `invite_code_set`
//! likewise closes a race where two users could end up holding the same
//! code (the old check-then-write could pass the uniqueness check for both
//! before either wrote) via a `UNIQUE` index on `code` plus a
//! `BEGIN IMMEDIATE` transaction.

use crate::data::DataStore;

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Sets (or changes) a user's invite code. Returns `false` (and leaves the
/// code untouched) if it's already held by a *different* user — setting
/// your own existing code again is a no-op success.
pub fn invite_code_set(store: &DataStore, norm_email: &str, code: &str) -> bool {
    let conn = store.conn();
    if conn.execute_batch("BEGIN IMMEDIATE").is_err() {
        return false;
    }
    let taken_by_other: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM invite_codes WHERE code = ?1 AND email != ?2)",
            rusqlite::params![code, norm_email],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if taken_by_other {
        let _ = conn.execute_batch("ROLLBACK");
        return false;
    }
    let result = conn.execute(
        "INSERT INTO invite_codes (email, code, created_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(email) DO UPDATE SET code = ?2, created_at = ?3",
        rusqlite::params![norm_email, code, now_millis()],
    );
    if result.is_err() {
        let _ = conn.execute_batch("ROLLBACK");
        return false;
    }
    let _ = conn.execute_batch("COMMIT");
    true
}

pub fn invite_code_get(store: &DataStore, norm_email: &str) -> Option<String> {
    let conn = store.conn();
    conn.query_row(
        "SELECT code FROM invite_codes WHERE email = ?1",
        rusqlite::params![norm_email],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// The email that owns a given code, case/whitespace-insensitive (codes are
/// always stored upper-cased, but this normalizes the lookup key too).
pub fn invite_code_find_owner(store: &DataStore, code: &str) -> Option<String> {
    let normalized = code.trim().to_uppercase();
    let conn = store.conn();
    conn.query_row(
        "SELECT email FROM invite_codes WHERE code = ?1",
        rusqlite::params![normalized],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// Atomically records a referral claim for `norm_email` if one doesn't
/// already exist. Returns `true` only the first time — callers should only
/// pay out the referral bonus when this returns `true`.
pub fn invite_claim_insert_if_new(store: &DataStore, norm_email: &str, ref_norm: &str) -> bool {
    let conn = store.conn();
    let changed = conn
        .execute(
            "INSERT OR IGNORE INTO invite_claims (email, ref_norm, claimed_at, paid) VALUES (?1, ?2, ?3, 1)",
            rusqlite::params![norm_email, ref_norm, now_millis()],
        )
        .unwrap_or(0);
    changed > 0
}

/// Records that `sender_norm` invited `recipient_norm`, if not already
/// sent. Returns `true` if this is a newly recorded invite.
pub fn invite_sent_insert_if_new(store: &DataStore, sender_norm: &str, recipient_norm: &str) -> bool {
    let conn = store.conn();
    let changed = conn
        .execute(
            "INSERT OR IGNORE INTO invite_sent (sender_email, recipient_email, sent_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![sender_norm, recipient_norm, now_millis()],
        )
        .unwrap_or(0);
    changed > 0
}

/// Moves one user's invite-system rows onto their new normalized email.
/// Mirrors the old generic rename sweep's behavior exactly: only the
/// renamed user's own rows move — a referral's `ref_norm` or a recipient
/// who isn't the renamed user is left as-is, same as before.
pub fn rename_invite_email(store: &DataStore, old_norm: &str, new_norm: &str) {
    let conn = store.conn();
    let _ = conn.execute(
        "UPDATE invite_codes SET email = ?2 WHERE email = ?1",
        rusqlite::params![old_norm, new_norm],
    );
    let _ = conn.execute(
        "UPDATE invite_claims SET email = ?2 WHERE email = ?1",
        rusqlite::params![old_norm, new_norm],
    );
    let _ = conn.execute(
        "UPDATE invite_sent SET sender_email = ?2 WHERE sender_email = ?1",
        rusqlite::params![old_norm, new_norm],
    );
}

pub fn invite_sent_contains(store: &DataStore, sender_norm: &str, recipient_norm: &str) -> bool {
    let conn = store.conn();
    conn.query_row(
        "SELECT 1 FROM invite_sent WHERE sender_email = ?1 AND recipient_email = ?2",
        rusqlite::params![sender_norm, recipient_norm],
        |_| Ok(()),
    )
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-invites-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn invite_code_set_rejects_a_code_taken_by_someone_else() {
        let store = temp_store("code-conflict");
        assert!(invite_code_set(&store, "alice@gmail.com", "ALICE123"));
        assert!(!invite_code_set(&store, "bob@gmail.com", "ALICE123"));
        // Setting your own code to the same value again is fine.
        assert!(invite_code_set(&store, "alice@gmail.com", "ALICE123"));
        assert_eq!(invite_code_get(&store, "alice@gmail.com"), Some("ALICE123".to_string()));
        assert_eq!(invite_code_find_owner(&store, "alice123"), Some("alice@gmail.com".to_string()));
    }

    #[test]
    fn invite_claim_insert_if_new_only_pays_once() {
        let store = temp_store("claim-once");
        assert!(invite_claim_insert_if_new(&store, "newbie@gmail.com", "referrer@gmail.com"));
        // Second attempt (simulating a retried/duplicate request) must not re-claim.
        assert!(!invite_claim_insert_if_new(&store, "newbie@gmail.com", "someone-else@gmail.com"));
    }

    #[test]
    fn invite_sent_dedupes() {
        let store = temp_store("sent-dedupe");
        assert!(invite_sent_insert_if_new(&store, "sender@gmail.com", "friend@gmail.com"));
        assert!(!invite_sent_insert_if_new(&store, "sender@gmail.com", "friend@gmail.com"));
        assert!(invite_sent_contains(&store, "sender@gmail.com", "friend@gmail.com"));
        assert!(!invite_sent_contains(&store, "sender@gmail.com", "nobody@gmail.com"));
    }
}
