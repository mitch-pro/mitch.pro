//! Newsletter-unsubscribe list — real `newsletter_unsub` table (one row
//! per email) instead of the old newsletter_unsub.json blob.
//!
//! The old blob had a documented quirk: one write path (`POST
//! /api/team/unsubscribe`) wrote only to the raw JSON file on disk,
//! never updating the DataStore-backed copy other reads went through —
//! a bug in the original server.js, faithfully preserved until now.
//! With everything routed through this one table there's no longer a
//! "disk" vs "DB" copy to diverge, so that inconsistency is gone as a
//! natural side effect of having a single source of truth (the same
//! kind of fix the achievements-table migration made for its own
//! double-payout race).

use crate::data::DataStore;

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn newsletter_unsub_exists(store: &DataStore, email: &str) -> bool {
    let conn = store.conn();
    conn.query_row(
        "SELECT 1 FROM newsletter_unsub WHERE email = ?1",
        rusqlite::params![email],
        |_| Ok(()),
    )
    .is_ok()
}

pub fn newsletter_unsub_add(store: &DataStore, email: &str) {
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT OR IGNORE INTO newsletter_unsub (email, created_at) VALUES (?1, ?2)",
        rusqlite::params![email, now_millis()],
    );
}

pub fn newsletter_unsub_remove(store: &DataStore, email: &str) -> bool {
    let conn = store.conn();
    conn.execute(
        "DELETE FROM newsletter_unsub WHERE email = ?1",
        rusqlite::params![email],
    )
    .unwrap_or(0)
        > 0
}

/// Every unsubscribed email, for call sites that used to load the whole
/// blob once and scan it.
pub fn newsletter_unsub_all_emails(store: &DataStore) -> Vec<String> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare("SELECT email FROM newsletter_unsub") else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) else {
        return Vec::new();
    };
    rows.filter_map(|r| r.ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-newsletter-unsub-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn add_exists_remove_round_trip() {
        let store = temp_store("round-trip");
        assert!(!newsletter_unsub_exists(&store, "a@gmail.com"));
        newsletter_unsub_add(&store, "a@gmail.com");
        assert!(newsletter_unsub_exists(&store, "a@gmail.com"));
        assert!(newsletter_unsub_remove(&store, "a@gmail.com"));
        assert!(!newsletter_unsub_exists(&store, "a@gmail.com"));
    }

    #[test]
    fn add_is_idempotent() {
        let store = temp_store("idempotent");
        newsletter_unsub_add(&store, "a@gmail.com");
        newsletter_unsub_add(&store, "a@gmail.com");
        assert_eq!(
            newsletter_unsub_all_emails(&store),
            vec!["a@gmail.com".to_string()]
        );
    }

    #[test]
    fn all_emails_lists_every_row() {
        let store = temp_store("all");
        newsletter_unsub_add(&store, "a@gmail.com");
        newsletter_unsub_add(&store, "b@gmail.com");
        let mut emails = newsletter_unsub_all_emails(&store);
        emails.sort();
        assert_eq!(
            emails,
            vec!["a@gmail.com".to_string(), "b@gmail.com".to_string()]
        );
    }
}
