//! Email-keyed tables that `rename_email_references` (in the server's
//! `routes/me/account.rs`) doesn't cover: when an account changes email, every
//! row that belongs to it must move with it or the account silently loses that
//! data (its computers, companion chats, referral record, ...).
//!
//! Not moved on purpose:
//! - `blacklist` / `banned_ips`: a banned account can't change email at all
//!   (checked by the caller), so a ban can't be shed by renaming.
//! - `vm_audit_logs`, `account_chat_removals*`, `moderated_media`: historical
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
    ("virtual_machines", "owner_email"),
    ("vm_usage_samples", "owner_email"),
    ("newsletter_unsub", "email"),
    ("tester_settings", "email"),
    ("companion_characters", "creator_email"),
    ("companion_user_settings", "email"),
    ("companion_coin_requests", "email"),
    ("companion_chat_messages", "email"),
    ("companion_memories", "email"),
    ("companion_subscriptions", "email"),
    ("companion_character_unlocks", "email"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_follow_the_new_email_and_old_rows_are_gone() {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-account-email-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        let store = DataStore::open(&base, &base.join("data")).unwrap();
        crate::vm::upsert_virtual_machine(
            &store,
            &serde_json::json!({ "id": "vm1", "ownerEmail": "old@x.test", "vmid": 300, "node": "n" }),
        )
        .unwrap();
        {
            let conn = store.conn();
            conn.execute(
                "INSERT INTO newsletter_unsub (email, created_at) VALUES ('old@x.test', 1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO newsletter_unsub (email, created_at) VALUES ('new@x.test', 2)",
                [],
            )
            .unwrap();
        }
        rename_remaining_tables(&store, "old@x.test", "new@x.test");
        let conn = store.conn();
        let vm_owner: String = conn
            .query_row(
                "SELECT owner_email FROM virtual_machines WHERE id = 'vm1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(vm_owner, "new@x.test");
        let unsub: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM newsletter_unsub WHERE email = 'old@x.test'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(unsub, 0);
    }
}
