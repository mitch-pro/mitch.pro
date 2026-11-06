//! Consent for staff access to a member's VM (`vm_admin_access_grants`).
//!
//! A VM is opened (desktop, power controls) only by its owner and the site
//! owner, unless the VM's owner explicitly allows site admins in. Allowing
//! is a deliberate action on the owner's own VM page, lasts
//! [`GRANT_DURATION_MS`], and can be turned off at any time. A grant is tied
//! to the owner it was made by: if the VM is reassigned, the old grant no
//! longer matches and stops working. Owner-locked VMs (`auth::is_vm_owner_locked`)
//! never honor a grant. See `docs/vm-access.md`.

use crate::auth::normalize_email;
use crate::data::DataStore;

/// How long one "allow admins" lasts.
pub const GRANT_DURATION_MS: i64 = 24 * 60 * 60 * 1000;

/// Records the owner's consent. Returns the expiry time.
pub fn grant(store: &DataStore, vm_record_id: &str, owner_email: &str, now: i64) -> i64 {
    let expires_at = now + GRANT_DURATION_MS;
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO vm_admin_access_grants (vm_record_id, owner_email, granted_at, expires_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(vm_record_id) DO UPDATE SET owner_email = ?2, granted_at = ?3, expires_at = ?4",
        rusqlite::params![vm_record_id, normalize_email(owner_email), now, expires_at],
    );
    expires_at
}

/// Withdraws consent. Returns whether a grant existed.
pub fn revoke(store: &DataStore, vm_record_id: &str) -> bool {
    let conn = store.conn();
    conn.execute(
        "DELETE FROM vm_admin_access_grants WHERE vm_record_id = ?1",
        rusqlite::params![vm_record_id],
    )
    .unwrap_or(0)
        > 0
}

/// Expiry of the live grant for this VM and its current owner, if any.
pub fn active_grant(
    store: &DataStore,
    vm_record_id: &str,
    owner_email: &str,
    now: i64,
) -> Option<i64> {
    if vm_record_id.is_empty() || owner_email.is_empty() {
        return None;
    }
    let conn = store.conn();
    let _ = conn.execute(
        "DELETE FROM vm_admin_access_grants WHERE expires_at <= ?1",
        rusqlite::params![now],
    );
    conn.query_row(
        "SELECT expires_at FROM vm_admin_access_grants WHERE vm_record_id = ?1 AND owner_email = ?2",
        rusqlite::params![vm_record_id, normalize_email(owner_email)],
        |r| r.get(0),
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-vm-access-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn grants_expire_follow_the_owner_and_can_be_revoked() {
        let store = temp_store("grant");
        let now = 1_000_000;
        let expires = grant(&store, "vm-1", "Student@Example.com", now);
        assert_eq!(expires, now + GRANT_DURATION_MS);
        assert_eq!(
            active_grant(&store, "vm-1", "student@example.com", now + 1),
            Some(expires)
        );
        // Reassigned VM: the old owner's consent doesn't carry over.
        assert_eq!(
            active_grant(&store, "vm-1", "someone-else@example.com", now + 1),
            None
        );
        // Expired.
        assert_eq!(
            active_grant(&store, "vm-1", "student@example.com", expires),
            None
        );
        grant(&store, "vm-1", "student@example.com", now);
        assert!(revoke(&store, "vm-1"));
        assert_eq!(
            active_grant(&store, "vm-1", "student@example.com", now + 1),
            None
        );
        assert!(!revoke(&store, "vm-1"));
    }
}
