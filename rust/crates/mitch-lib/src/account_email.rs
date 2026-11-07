//! Email-keyed tables that `rename_email_references` (in the server's
//! `routes/me/account.rs`) doesn't cover: when an account changes email, every
//! row that belongs to it must move with it or the account silently loses that
//! data (companion chats, referral records, and settings).
//!
//! Not moved on purpose:
//! - `blacklist` / `banned_ips`: a banned account can't change email at all
//!   (checked by the caller), so a ban can't be shed by renaming.
//! - `account_chat_removals*`, `moderated_media`: historical
//!   records of what happened under the old address.
//!
//! Staff lists (`admins.json`, `moderators.json`) are not moved either; staff
//! accounts can't change email through the normal flows (see `docs/support.md`).

use crate::data::DataStore;

/// `(table, column)` pairs holding a normalized account email that should
/// follow the account. `UPDATE OR REPLACE` lets the migrating account's row
/// win over a stale row left behind for the new address.
const TABLES: &[(&str, &str)] = &[
    ("users", "email"),
    ("referrals", "email"),
    ("matrix_outside_approvals", "email"),
    ("newsletter_unsub", "email"),
    ("tester_settings", "email"),
    ("companion_characters", "creator_email"),
    ("companion_user_settings", "email"),
    ("companion_coin_requests", "email"),
    ("companion_bans", "email"),
    ("companion_chat_messages", "email"),
    ("companion_memories", "email"),
    ("companion_subscriptions", "email"),
    ("companion_character_unlocks", "email"),
    ("companion_ability_strain", "email"),
    ("companion_upgrades", "email"),
    ("reserved_usernames", "holder_email"),
];

/// Moves the rows listed in [`TABLES`] from `old_norm` to `new_norm`.
pub fn rename_remaining_tables(store: &DataStore, old_norm: &str, new_norm: &str) {
    let conn = store.conn();
    for (table, column) in TABLES {
        let sql = format!("UPDATE OR REPLACE {table} SET {column} = ?1 WHERE {column} = ?2");
        if let Err(e) = conn.execute(&sql, rusqlite::params![new_norm, old_norm]) {
            tracing::warn!("email rename: {table}.{column}: {e}");
        }
    }
}
