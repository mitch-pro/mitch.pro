//! Who may look up an account's profile.
//!
//! An account can turn on "block profile lookups" in Preferences. Then nobody
//! but the account itself and staff (admins and moderators; **not** testers)
//! can open its profile (`GET /api/profile/<slug>`) or find it by searching the
//! members directory (`GET /api/members?q=`). The account's own contacts
//! (friends, pending requests, people it has messaged) still appear in each
//! other's friend and chat lists, since those are relationships the account
//! already has, and friend requests by exact username still work so people can
//! connect. See `docs/members.md`. One row per account that has it on; turning
//! it off deletes the row.

use crate::data::DataStore;

/// Whether `norm_email` has blocked profile lookups.
pub fn blocks_lookups(store: &DataStore, norm_email: &str) -> bool {
    store
        .conn()
        .query_row(
            "SELECT block_lookups FROM profile_privacy WHERE email = ?1",
            rusqlite::params![norm_email],
            |r| r.get::<_, i64>(0),
        )
        .map(|v| v != 0)
        .unwrap_or(false)
}

pub fn set_blocks_lookups(store: &DataStore, norm_email: &str, block: bool) {
    let conn = store.conn();
    if block {
        let _ = conn.execute(
            "INSERT INTO profile_privacy (email, block_lookups) VALUES (?1, 1) \
             ON CONFLICT(email) DO UPDATE SET block_lookups = 1",
            rusqlite::params![norm_email],
        );
    } else {
        let _ = conn.execute(
            "DELETE FROM profile_privacy WHERE email = ?1",
            rusqlite::params![norm_email],
        );
    }
}

/// An account changed its email: the setting moves with it.
pub fn rename_email(store: &DataStore, old_norm: &str, new_norm: &str) {
    let _ = store.conn().execute(
        "UPDATE OR REPLACE profile_privacy SET email = ?2 WHERE email = ?1",
        rusqlite::params![old_norm, new_norm],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::random_bytes_hex;

    #[test]
    fn the_setting_is_per_account_and_moves_with_a_rename() {
        let dir = std::env::temp_dir().join(format!("profile-privacy-{}", random_bytes_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        let s = DataStore::open(&dir, &dir).unwrap();
        assert!(!blocks_lookups(&s, "a@example.com"));
        set_blocks_lookups(&s, "a@example.com", true);
        assert!(blocks_lookups(&s, "a@example.com"));
        assert!(!blocks_lookups(&s, "b@example.com"));
        rename_email(&s, "a@example.com", "c@example.com");
        assert!(!blocks_lookups(&s, "a@example.com"));
        assert!(blocks_lookups(&s, "c@example.com"));
        set_blocks_lookups(&s, "c@example.com", false);
        assert!(!blocks_lookups(&s, "c@example.com"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
