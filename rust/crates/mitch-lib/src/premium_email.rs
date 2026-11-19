//! Free `@student.mitch.pro` addresses for Premium members.
//!
//! A Premium member asks for an address (`premium_email_requests`); an admin
//! creates the mailbox with the mail host and approves the request here, which
//! emails the member. This module only records the request and its decision. It
//! does not create mailboxes.
//!
//! `premium_email_prompt_dismissed` remembers who closed the "you can get a
//! free email" popup, so it never comes back for them.

use crate::data::DataStore;
use rusqlite::{params, OptionalExtension};

/// Domain the addresses live on.
pub const DOMAIN: &str = "student.mitch.pro";

/// Local parts nobody may request.
const RESERVED: &[&str] = &[
    "admin",
    "administrator",
    "support",
    "noreply",
    "mitch",
    "root",
    "postmaster",
    "abuse",
    "info",
    "help",
    "webmaster",
    "security",
    "owner",
    "staff",
    "moderator",
    "mod",
    "contact",
    "mail",
    "hostmaster",
    "billing",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub id: i64,
    pub email: String,
    pub address: String,
    pub full_name: String,
    pub reasons: String,
    /// pending, approved or denied
    pub status: String,
    pub created_at: i64,
    pub decided_by: String,
    pub decided_at: Option<i64>,
    pub note: String,
}

impl Request {
    /// The full address, e.g. `alex@student.mitch.pro`.
    pub fn full_address(&self) -> String {
        format!("{}@{DOMAIN}", self.address)
    }
}

const COLS: &str =
    "id, email, address, full_name, reasons, status, created_at, decided_by, decided_at, note";

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Request> {
    Ok(Request {
        id: r.get(0)?,
        email: r.get(1)?,
        address: r.get(2)?,
        full_name: r.get(3)?,
        reasons: r.get(4)?,
        status: r.get(5)?,
        created_at: r.get(6)?,
        decided_by: r.get(7)?,
        decided_at: r.get(8)?,
        note: r.get(9)?,
    })
}

/// Checks a requested local part: 3 to 20 lowercase letters and digits, not
/// reserved. Returns the reason in plain words when it is not acceptable.
pub fn validate_address(address: &str) -> Result<(), &'static str> {
    if !(3..=20).contains(&address.len())
        || !address
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        return Err("Use 3 to 20 lowercase letters or numbers.");
    }
    if RESERVED.contains(&address) {
        return Err("That address isn't available.");
    }
    Ok(())
}

/// A suggested address from a username: lowercase letters and digits only,
/// clipped to 20. Empty when nothing usable is left.
pub fn suggest_address(username: &str) -> String {
    let cleaned: String = username
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .take(20)
        .collect();
    if validate_address(&cleaned).is_ok() {
        cleaned
    } else {
        String::new()
    }
}

/// The member's most recent request that is still alive (pending or approved).
/// A denied request does not count, so the member may apply again.
pub fn active_for(store: &DataStore, email: &str) -> Option<Request> {
    store
        .conn()
        .query_row(
            &format!(
                "SELECT {COLS} FROM premium_email_requests
                 WHERE email = ?1 AND status IN ('pending', 'approved')
                 ORDER BY id DESC LIMIT 1"
            ),
            [email],
            from_row,
        )
        .optional()
        .ok()
        .flatten()
}

/// Whether the member has ever been refused. They are not nagged about the
/// popup afterwards.
pub fn was_denied(store: &DataStore, email: &str) -> bool {
    store
        .conn()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM premium_email_requests WHERE email = ?1 AND status = 'denied')",
            [email],
            |r| r.get(0),
        )
        .unwrap_or(false)
}

/// Why a request could not be recorded.
#[derive(Debug, PartialEq, Eq)]
pub enum CreateError {
    Invalid(&'static str),
    AlreadyApplied,
    AddressTaken,
    Storage,
}

/// Records a pending request. One live request per member; an address can be
/// held by only one pending or approved request.
pub fn create(
    store: &DataStore,
    email: &str,
    address: &str,
    full_name: &str,
    reasons: &str,
) -> Result<i64, CreateError> {
    validate_address(address).map_err(CreateError::Invalid)?;
    if full_name.trim().is_empty() || reasons.trim().is_empty() {
        return Err(CreateError::Invalid("All fields are required."));
    }
    let conn = store.conn();
    let tx = conn
        .unchecked_transaction()
        .map_err(|_| CreateError::Storage)?;
    let live: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM premium_email_requests WHERE email = ?1 AND status IN ('pending','approved'))",
            [email],
            |r| r.get(0),
        )
        .map_err(|_| CreateError::Storage)?;
    if live {
        return Err(CreateError::AlreadyApplied);
    }
    let taken: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM premium_email_requests WHERE address = ?1 AND status IN ('pending','approved'))",
            [address],
            |r| r.get(0),
        )
        .map_err(|_| CreateError::Storage)?;
    if taken {
        return Err(CreateError::AddressTaken);
    }
    tx.execute(
        "INSERT INTO premium_email_requests (email, address, full_name, reasons, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            email,
            address,
            full_name.trim().chars().take(100).collect::<String>(),
            reasons.trim().chars().take(1000).collect::<String>(),
            crate::school::now_millis()
        ],
    )
    .map_err(|_| CreateError::Storage)?;
    let id = tx.last_insert_rowid();
    tx.commit().map_err(|_| CreateError::Storage)?;
    Ok(id)
}

/// Requests for the staff queue: pending first, then the most recent decisions.
pub fn list(store: &DataStore, limit: usize) -> Vec<Request> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {COLS} FROM premium_email_requests
         ORDER BY (status = 'pending') DESC, id DESC LIMIT ?1"
    )) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([limit as i64], from_row) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok).collect()
}

pub fn get(store: &DataStore, id: i64) -> Option<Request> {
    store
        .conn()
        .query_row(
            &format!("SELECT {COLS} FROM premium_email_requests WHERE id = ?1"),
            [id],
            from_row,
        )
        .optional()
        .ok()
        .flatten()
}

pub fn pending_count(store: &DataStore) -> i64 {
    store
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM premium_email_requests WHERE status = 'pending'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0)
}

/// Approves or denies a pending request. Returns the updated request, or
/// `None` when it does not exist or was already decided.
pub fn decide(
    store: &DataStore,
    id: i64,
    approve: bool,
    decided_by: &str,
    note: &str,
) -> Option<Request> {
    let changed = store
        .conn()
        .execute(
            "UPDATE premium_email_requests
             SET status = ?1, decided_by = ?2, decided_at = ?3, note = ?4
             WHERE id = ?5 AND status = 'pending'",
            params![
                if approve { "approved" } else { "denied" },
                decided_by,
                crate::school::now_millis(),
                note.chars().take(500).collect::<String>(),
                id
            ],
        )
        .unwrap_or(0);
    if changed == 0 {
        return None;
    }
    get(store, id)
}

/// Marks the popup as dismissed for good.
pub fn dismiss_prompt(store: &DataStore, email: &str) {
    let _ = store.conn().execute(
        "INSERT OR IGNORE INTO premium_email_prompt_dismissed (email, dismissed_at) VALUES (?1, ?2)",
        params![email, crate::school::now_millis()],
    );
}

pub fn prompt_dismissed(store: &DataStore, email: &str) -> bool {
    store
        .conn()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM premium_email_prompt_dismissed WHERE email = ?1)",
            [email],
            |r| r.get(0),
        )
        .unwrap_or(false)
}

/// Whether the popup should be shown to a Premium member right now.
pub fn should_prompt(store: &DataStore, email: &str) -> bool {
    active_for(store, email).is_none()
        && !was_denied(store, email)
        && !prompt_dismissed(store, email)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-premium-email-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn addresses_are_validated_and_suggested() {
        assert!(validate_address("alex99").is_ok());
        assert!(validate_address("ab").is_err());
        assert!(validate_address("Alex").is_err());
        assert!(validate_address("a.lex").is_err());
        assert!(validate_address("admin").is_err());
        assert_eq!(suggest_address("Alex_Smith-99"), "alexsmith99");
        assert_eq!(suggest_address("ad"), "");
    }

    #[test]
    fn request_lifecycle_and_prompt_rules() {
        let s = store("life");
        let me = "test.student1@student.rjuhsd.us";
        assert!(should_prompt(&s, me));

        let id = create(&s, me, "myname", "My Name", "I like it").unwrap();
        assert!(!should_prompt(&s, me), "a pending request ends the prompt");
        assert_eq!(
            create(&s, me, "other", "My Name", "x"),
            Err(CreateError::AlreadyApplied)
        );
        assert_eq!(
            create(&s, "you@x.test", "myname", "You", "x"),
            Err(CreateError::AddressTaken)
        );
        assert_eq!(pending_count(&s), 1);

        let approved = decide(&s, id, true, "admin@x.test", "").unwrap();
        assert_eq!(approved.full_address(), "myname@student.mitch.pro");
        assert!(decide(&s, id, false, "admin@x.test", "").is_none());
        assert_eq!(active_for(&s, me).unwrap().status, "approved");
    }

    #[test]
    fn denied_members_may_reapply_but_are_not_prompted() {
        let s = store("deny");
        let me = "test.student1@student.rjuhsd.us";
        let id = create(&s, me, "myname", "My Name", "x").unwrap();
        decide(&s, id, false, "admin@x.test", "no").unwrap();
        assert!(!should_prompt(&s, me));
        assert!(create(&s, me, "myname", "My Name", "x").is_ok());
    }

    #[test]
    fn dismissing_the_prompt_is_permanent() {
        let s = store("dismiss");
        let me = "test.student1@student.rjuhsd.us";
        assert!(should_prompt(&s, me));
        dismiss_prompt(&s, me);
        dismiss_prompt(&s, me);
        assert!(!should_prompt(&s, me));
        assert!(prompt_dismissed(&s, me));
    }
}
