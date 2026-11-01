//! Support tickets for people who can't (or can't safely) use their account:
//! locked out, account compromised, email changes. See `docs/support.md`.
//!
//! Three SQL tables: `support_tickets` (one per request; the creator holds a
//! secret link whose token is stored only as a SHA-256 hash),
//! `support_messages` (the user/staff chat), and `support_email_requests` (a
//! staff-requested email change that an admin must approve).
//!
//! The rules about who may do what live here, not in the route handlers, so no
//! caller can skip them: one pending email request per ticket, and a request
//! can only be decided by someone other than the person who made it.

use crate::data::DataStore;

/// Tickets and chats of resolved tickets are deleted after this long.
pub const RETENTION_MS: i64 = 90 * 24 * 60 * 60 * 1000;
/// How long an emailed confirmation code for an approved change stays valid.
pub const CODE_TTL_MS: i64 = 30 * 60 * 1000;
pub const MAX_CODE_ATTEMPTS: i64 = 5;

pub const CATEGORIES: &[&str] = &["signin", "compromised", "email", "banned", "other"];
pub const STATUSES: &[&str] = &["open", "pending", "resolved"];

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    pub id: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub status: String,
    pub category: String,
    pub subject: String,
    /// What the person typed as their username/email.
    pub claimed_account: String,
    /// Normalized email of the account that text resolved to, or empty.
    pub account_email: String,
    pub contact_email: String,
    pub compromised: bool,
    pub assigned_to: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub id: i64,
    pub sender: String,
    pub staff_email: String,
    pub body: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailRequest {
    pub id: i64,
    pub ticket_id: String,
    pub old_email: String,
    pub new_email: String,
    pub reason: String,
    pub requested_by: String,
    pub requested_at: i64,
    /// pending, approved, denied, applied, expired
    pub status: String,
    pub decided_by: String,
    pub decided_at: Option<i64>,
    pub decision_note: String,
}

pub struct NewTicket<'a> {
    pub token_hash: &'a str,
    pub ip_hash: &'a str,
    pub category: &'a str,
    pub subject: &'a str,
    pub claimed_account: &'a str,
    pub account_email: &'a str,
    pub contact_email: &'a str,
}

fn ticket_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Ticket> {
    Ok(Ticket {
        id: r.get(0)?,
        created_at: r.get(1)?,
        updated_at: r.get(2)?,
        status: r.get(3)?,
        category: r.get(4)?,
        subject: r.get(5)?,
        claimed_account: r.get(6)?,
        account_email: r.get(7)?,
        contact_email: r.get(8)?,
        compromised: r.get::<_, i64>(9)? != 0,
        assigned_to: r.get(10)?,
    })
}

const TICKET_COLS: &str = "id, created_at, updated_at, status, category, subject, claimed_account, account_email, contact_email, compromised, assigned_to";

fn request_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<EmailRequest> {
    Ok(EmailRequest {
        id: r.get(0)?,
        ticket_id: r.get(1)?,
        old_email: r.get(2)?,
        new_email: r.get(3)?,
        reason: r.get(4)?,
        requested_by: r.get(5)?,
        requested_at: r.get(6)?,
        status: r.get(7)?,
        decided_by: r.get(8)?,
        decided_at: r.get(9)?,
        decision_note: r.get(10)?,
    })
}

const REQUEST_COLS: &str = "id, ticket_id, old_email, new_email, reason, requested_by, requested_at, status, decided_by, decided_at, decision_note";

/// Creates a ticket and returns its id.
pub fn create_ticket(store: &DataStore, t: &NewTicket<'_>) -> Option<String> {
    let id = crate::crypto::random_bytes_hex(8);
    let now = now_millis();
    store
        .conn()
        .execute(
            "INSERT INTO support_tickets (id, token_hash, ip_hash, created_at, updated_at, category, subject, claimed_account, account_email, contact_email)
             VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                id,
                t.token_hash,
                t.ip_hash,
                now,
                t.category,
                t.subject,
                t.claimed_account,
                t.account_email,
                t.contact_email
            ],
        )
        .ok()?;
    Some(id)
}

/// Tickets created from one network address since `since_ms`; used to cap abuse.
pub fn count_recent_by_ip(store: &DataStore, ip_hash: &str, since_ms: i64) -> i64 {
    store
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM support_tickets WHERE ip_hash = ?1 AND created_at >= ?2",
            rusqlite::params![ip_hash, since_ms],
            |r| r.get(0),
        )
        .unwrap_or(0)
}

/// Tickets created by anyone since `since_ms`.
pub fn count_recent(store: &DataStore, since_ms: i64) -> i64 {
    store
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM support_tickets WHERE created_at >= ?1",
            rusqlite::params![since_ms],
            |r| r.get(0),
        )
        .unwrap_or(0)
}

pub fn ticket(store: &DataStore, id: &str) -> Option<Ticket> {
    store
        .conn()
        .query_row(
            &format!("SELECT {TICKET_COLS} FROM support_tickets WHERE id = ?1"),
            rusqlite::params![id],
            ticket_from_row,
        )
        .ok()
}

/// True when `token_hash` is the hash stored for the ticket.
pub fn token_matches(store: &DataStore, id: &str, token_hash: &str) -> bool {
    let stored: Option<String> = store
        .conn()
        .query_row(
            "SELECT token_hash FROM support_tickets WHERE id = ?1",
            rusqlite::params![id],
            |r| r.get(0),
        )
        .ok();
    stored.is_some_and(|s| !s.is_empty() && s == token_hash)
}

/// Appends a chat message (`sender` is `user`, `staff` or `system`).
pub fn add_message(
    store: &DataStore,
    ticket_id: &str,
    sender: &str,
    staff_email: &str,
    body: &str,
) -> Option<i64> {
    let now = now_millis();
    let conn = store.conn();
    conn.execute(
        "INSERT INTO support_messages (ticket_id, sender, staff_email, body, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![ticket_id, sender, staff_email, body, now],
    )
    .ok()?;
    let id = conn.last_insert_rowid();
    let _ = conn.execute(
        "UPDATE support_tickets SET updated_at = ?2 WHERE id = ?1",
        rusqlite::params![ticket_id, now],
    );
    Some(id)
}

pub fn messages(store: &DataStore, ticket_id: &str) -> Vec<Message> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, sender, staff_email, body, created_at FROM support_messages WHERE ticket_id = ?1 ORDER BY id",
    ) else {
        return Vec::new();
    };
    let rows = stmt.query_map(rusqlite::params![ticket_id], |r| {
        Ok(Message {
            id: r.get(0)?,
            sender: r.get(1)?,
            staff_email: r.get(2)?,
            body: r.get(3)?,
            created_at: r.get(4)?,
        })
    });
    match rows {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

pub fn message_count(store: &DataStore, ticket_id: &str, sender: &str) -> i64 {
    store
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM support_messages WHERE ticket_id = ?1 AND sender = ?2",
            rusqlite::params![ticket_id, sender],
            |r| r.get(0),
        )
        .unwrap_or(0)
}

/// Time of the newest message from `sender`, or 0.
pub fn last_message_at(store: &DataStore, ticket_id: &str, sender: &str) -> i64 {
    store
        .conn()
        .query_row(
            "SELECT COALESCE(MAX(created_at), 0) FROM support_messages WHERE ticket_id = ?1 AND sender = ?2",
            rusqlite::params![ticket_id, sender],
            |r| r.get(0),
        )
        .unwrap_or(0)
}

/// Newest-activity first. `status` of `None` lists every ticket.
pub fn list_tickets(store: &DataStore, status: Option<&str>, limit: i64) -> Vec<Ticket> {
    let conn = store.conn();
    let (sql, filter) = match status {
        Some(s) => (
            format!("SELECT {TICKET_COLS} FROM support_tickets WHERE status = ?1 ORDER BY updated_at DESC LIMIT ?2"),
            s.to_string(),
        ),
        None => (
            format!("SELECT {TICKET_COLS} FROM support_tickets WHERE ?1 = '' ORDER BY updated_at DESC LIMIT ?2"),
            String::new(),
        ),
    };
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let rows = stmt.query_map(rusqlite::params![filter, limit], ticket_from_row);
    match rows {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

pub fn set_status(store: &DataStore, id: &str, status: &str) -> bool {
    STATUSES.contains(&status)
        && store
            .conn()
            .execute(
                "UPDATE support_tickets SET status = ?2, updated_at = ?3 WHERE id = ?1",
                rusqlite::params![id, status, now_millis()],
            )
            .unwrap_or(0)
            > 0
}

pub fn set_compromised(store: &DataStore, id: &str, compromised: bool) -> bool {
    store
        .conn()
        .execute(
            "UPDATE support_tickets SET compromised = ?2, updated_at = ?3 WHERE id = ?1",
            rusqlite::params![id, compromised as i64, now_millis()],
        )
        .unwrap_or(0)
        > 0
}

pub fn set_assigned(store: &DataStore, id: &str, staff_email: &str) -> bool {
    store
        .conn()
        .execute(
            "UPDATE support_tickets SET assigned_to = ?2 WHERE id = ?1",
            rusqlite::params![id, staff_email],
        )
        .unwrap_or(0)
        > 0
}

/// Deletes resolved tickets (with their messages and requests) untouched for
/// [`RETENTION_MS`].
pub fn prune_old(store: &DataStore, now: i64) {
    let cutoff = now - RETENTION_MS;
    let conn = store.conn();
    for sql in [
        "DELETE FROM support_messages WHERE ticket_id IN (SELECT id FROM support_tickets WHERE status = 'resolved' AND updated_at < ?1)",
        "DELETE FROM support_email_requests WHERE ticket_id IN (SELECT id FROM support_tickets WHERE status = 'resolved' AND updated_at < ?1)",
        "DELETE FROM support_tickets WHERE status = 'resolved' AND updated_at < ?1",
    ] {
        let _ = conn.execute(sql, rusqlite::params![cutoff]);
    }
}

// ── Email change requests ───────────────────────────────────────────────────

/// Why a request couldn't be created or decided.
#[derive(Debug, PartialEq, Eq)]
pub enum RequestError {
    NoAccount,
    AlreadyPending,
    NotPending,
    OwnRequest,
    Database,
}

impl RequestError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::NoAccount => "This ticket isn't linked to an account.",
            Self::AlreadyPending => "This ticket already has an email change in progress.",
            Self::NotPending => "That request has already been decided.",
            Self::OwnRequest => "You can't decide your own request. Another admin must.",
            Self::Database => "Could not save that. Try again.",
        }
    }
}

/// Records a staff member's request to move the ticket's account to
/// `new_email`. Only one request per ticket may be open (pending or approved
/// and awaiting the user's confirmation).
pub fn create_email_request(
    store: &DataStore,
    ticket_id: &str,
    new_email: &str,
    reason: &str,
    requested_by: &str,
) -> Result<i64, RequestError> {
    let t = ticket(store, ticket_id).ok_or(RequestError::NoAccount)?;
    if t.account_email.is_empty() {
        return Err(RequestError::NoAccount);
    }
    let conn = store.conn();
    let open: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM support_email_requests WHERE ticket_id = ?1 AND status IN ('pending', 'approved')",
            rusqlite::params![ticket_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if open > 0 {
        return Err(RequestError::AlreadyPending);
    }
    conn.execute(
        "INSERT INTO support_email_requests (ticket_id, old_email, new_email, reason, requested_by, requested_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            ticket_id,
            t.account_email,
            new_email,
            reason,
            requested_by,
            now_millis()
        ],
    )
    .map_err(|_| RequestError::Database)?;
    Ok(conn.last_insert_rowid())
}

pub fn email_request(store: &DataStore, id: i64) -> Option<EmailRequest> {
    store
        .conn()
        .query_row(
            &format!("SELECT {REQUEST_COLS} FROM support_email_requests WHERE id = ?1"),
            rusqlite::params![id],
            request_from_row,
        )
        .ok()
}

/// Requests for one ticket, oldest first.
pub fn email_requests_for_ticket(store: &DataStore, ticket_id: &str) -> Vec<EmailRequest> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {REQUEST_COLS} FROM support_email_requests WHERE ticket_id = ?1 ORDER BY id"
    )) else {
        return Vec::new();
    };
    let rows = stmt.query_map(rusqlite::params![ticket_id], request_from_row);
    match rows {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Requests with the given status, oldest first.
pub fn email_requests_with_status(store: &DataStore, status: &str) -> Vec<EmailRequest> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {REQUEST_COLS} FROM support_email_requests WHERE status = ?1 ORDER BY id"
    )) else {
        return Vec::new();
    };
    let rows = stmt.query_map(rusqlite::params![status], request_from_row);
    match rows {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Approves or denies a pending request. `decided_by` must differ from the
/// requester, so nobody approves their own request. Approving stores the hash
/// of the confirmation code that was emailed to the new address.
pub fn decide_email_request(
    store: &DataStore,
    id: i64,
    approve: bool,
    decided_by: &str,
    note: &str,
    code_hash: &str,
) -> Result<(), RequestError> {
    let req = email_request(store, id).ok_or(RequestError::NotPending)?;
    if req.status != "pending" {
        return Err(RequestError::NotPending);
    }
    if req.requested_by.eq_ignore_ascii_case(decided_by) {
        return Err(RequestError::OwnRequest);
    }
    let status = if approve { "approved" } else { "denied" };
    let now = now_millis();
    let changed = store
        .conn()
        .execute(
            "UPDATE support_email_requests
             SET status = ?2, decided_by = ?3, decided_at = ?4, decision_note = ?5, code_hash = ?6, code_expires = ?7, code_attempts = 0
             WHERE id = ?1 AND status = 'pending'",
            rusqlite::params![
                id,
                status,
                decided_by,
                now,
                note,
                if approve { code_hash } else { "" },
                if approve { now + CODE_TTL_MS } else { 0 }
            ],
        )
        .map_err(|_| RequestError::Database)?;
    if changed == 0 {
        return Err(RequestError::NotPending);
    }
    Ok(())
}

/// Result of the user entering the code emailed to the new address.
#[derive(Debug, PartialEq, Eq)]
pub enum CodeCheck {
    /// No approved request is waiting for a code.
    Nothing,
    Wrong,
    TooManyAttempts,
    Expired,
    Ok(Box<EmailRequest>),
}

/// Checks `code_hash` against the approved request for the ticket. Counts the
/// attempt; five wrong tries expire the request.
pub fn check_code(store: &DataStore, ticket_id: &str, code_hash: &str) -> CodeCheck {
    let now = now_millis();
    let conn = store.conn();
    let row: Option<(i64, String, i64, i64)> = conn
        .query_row(
            "SELECT id, code_hash, code_expires, code_attempts FROM support_email_requests
             WHERE ticket_id = ?1 AND status = 'approved' ORDER BY id DESC LIMIT 1",
            rusqlite::params![ticket_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .ok();
    let Some((id, stored, expires, attempts)) = row else {
        return CodeCheck::Nothing;
    };
    if attempts >= MAX_CODE_ATTEMPTS {
        return CodeCheck::TooManyAttempts;
    }
    if expires < now {
        let _ = conn.execute(
            "UPDATE support_email_requests SET status = 'expired' WHERE id = ?1",
            rusqlite::params![id],
        );
        return CodeCheck::Expired;
    }
    if stored.is_empty() || stored != code_hash {
        let _ = conn.execute(
            "UPDATE support_email_requests SET code_attempts = code_attempts + 1 WHERE id = ?1",
            rusqlite::params![id],
        );
        return CodeCheck::Wrong;
    }
    drop(conn);
    match email_request(store, id) {
        Some(req) => CodeCheck::Ok(Box::new(req)),
        None => CodeCheck::Nothing,
    }
}

/// Closes an approved request that can no longer be applied (the account
/// became staff or banned, or the new address was taken in the meantime).
pub fn mark_expired(store: &DataStore, id: i64) {
    let _ = store.conn().execute(
        "UPDATE support_email_requests SET status = 'expired', code_hash = '' WHERE id = ?1",
        rusqlite::params![id],
    );
}

pub fn mark_applied(store: &DataStore, id: i64) {
    let _ = store.conn().execute(
        "UPDATE support_email_requests SET status = 'applied', applied_at = ?2, code_hash = '' WHERE id = ?1",
        rusqlite::params![id, now_millis()],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-support-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    fn new_ticket(store: &DataStore, account_email: &str) -> String {
        create_ticket(
            store,
            &NewTicket {
                token_hash: "hash",
                ip_hash: "ip",
                category: "email",
                subject: "help",
                claimed_account: "someone",
                account_email,
                contact_email: "",
            },
        )
        .unwrap()
    }

    #[test]
    fn tickets_check_their_token_and_collect_messages() {
        let store = temp_store("basic");
        let id = new_ticket(&store, "a@x.test");
        assert!(token_matches(&store, &id, "hash"));
        assert!(!token_matches(&store, &id, "other"));
        assert!(!token_matches(&store, "missing", "hash"));
        add_message(&store, &id, "user", "", "hello").unwrap();
        add_message(&store, &id, "staff", "mod@x.test", "hi").unwrap();
        assert_eq!(messages(&store, &id).len(), 2);
        assert_eq!(count_recent_by_ip(&store, "ip", 0), 1);
        assert!(set_status(&store, &id, "resolved"));
        assert!(!set_status(&store, &id, "bogus"));
        assert_eq!(list_tickets(&store, Some("open"), 10).len(), 0);
        assert_eq!(list_tickets(&store, None, 10).len(), 1);
    }

    #[test]
    fn email_requests_follow_the_rules() {
        let store = temp_store("rules");
        let unlinked = new_ticket(&store, "");
        assert_eq!(
            create_email_request(&store, &unlinked, "n@x.test", "r", "mod@x.test"),
            Err(RequestError::NoAccount)
        );
        let id = new_ticket(&store, "a@x.test");
        let req = create_email_request(&store, &id, "n@x.test", "r", "mod@x.test").unwrap();
        assert_eq!(
            create_email_request(&store, &id, "m@x.test", "r", "mod@x.test"),
            Err(RequestError::AlreadyPending)
        );
        // Nobody decides their own request.
        assert_eq!(
            decide_email_request(&store, req, true, "MOD@x.test", "", "h"),
            Err(RequestError::OwnRequest)
        );
        decide_email_request(&store, req, true, "admin@x.test", "ok", "codehash").unwrap();
        assert_eq!(
            decide_email_request(&store, req, false, "admin2@x.test", "", ""),
            Err(RequestError::NotPending)
        );
        // Still "in progress" until applied, so no second request.
        assert_eq!(
            create_email_request(&store, &id, "m@x.test", "r", "mod@x.test"),
            Err(RequestError::AlreadyPending)
        );
    }

    #[test]
    fn code_check_counts_attempts_and_applies_once() {
        let store = temp_store("code");
        let id = new_ticket(&store, "a@x.test");
        assert_eq!(check_code(&store, &id, "x"), CodeCheck::Nothing);
        let req = create_email_request(&store, &id, "n@x.test", "r", "mod@x.test").unwrap();
        assert_eq!(
            check_code(&store, &id, "x"),
            CodeCheck::Nothing,
            "pending, not approved"
        );
        decide_email_request(&store, req, true, "admin@x.test", "", "right").unwrap();
        assert_eq!(check_code(&store, &id, "wrong"), CodeCheck::Wrong);
        match check_code(&store, &id, "right") {
            CodeCheck::Ok(r) => assert_eq!(r.new_email, "n@x.test"),
            other => panic!("expected Ok, got {other:?}"),
        }
        mark_applied(&store, req);
        assert_eq!(check_code(&store, &id, "right"), CodeCheck::Nothing);
        // A denied request leaves the ticket free for a new one.
        let req2 = create_email_request(&store, &id, "q@x.test", "r", "mod@x.test").unwrap();
        decide_email_request(&store, req2, false, "admin@x.test", "no", "").unwrap();
        assert!(create_email_request(&store, &id, "z@x.test", "r", "mod@x.test").is_ok());
    }

    #[test]
    fn five_wrong_codes_lock_the_request() {
        let store = temp_store("lock");
        let id = new_ticket(&store, "a@x.test");
        let req = create_email_request(&store, &id, "n@x.test", "r", "mod@x.test").unwrap();
        decide_email_request(&store, req, true, "admin@x.test", "", "right").unwrap();
        for _ in 0..MAX_CODE_ATTEMPTS {
            assert_eq!(check_code(&store, &id, "bad"), CodeCheck::Wrong);
        }
        assert_eq!(check_code(&store, &id, "right"), CodeCheck::TooManyAttempts);
    }

    #[test]
    fn prune_removes_only_old_resolved_tickets() {
        let store = temp_store("prune");
        let old = new_ticket(&store, "a@x.test");
        let open = new_ticket(&store, "b@x.test");
        set_status(&store, &old, "resolved");
        prune_old(&store, now_millis() + RETENTION_MS + 1000);
        assert!(ticket(&store, &old).is_none());
        assert!(ticket(&store, &open).is_some());
    }
}
