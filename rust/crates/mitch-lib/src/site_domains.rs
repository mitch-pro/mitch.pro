//! Backup domains for when the main domain is blocked on a network.
//!
//! The pool lives in the `backup_domains` table: any number of origins, at
//! most one flagged `active`. Only the active origin is ever sent to visitors
//! (`GET /api/site-info`, cached by `webserver/sw.js`); the pool itself is only
//! visible to admins. Admins add origins and can rotate, which makes a random
//! *other* origin in the pool the active one.
//!
//! `data/site.json` (untracked, per-machine) still supplies `primary`,
//! `alternate` and `name`; when the pool has no active origin the backup falls
//! back to `alternate`.

use crate::data::DataStore;

/// Used when `data/site.json` is missing or lacks the field.
pub const DEFAULT_PRIMARY: &str = "https://mitch.pro";
pub const DEFAULT_ALTERNATE: &str = "https://mitchdog.com";

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// One pool entry, as listed in the admin panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupDomain {
    pub origin: String,
    pub active: bool,
    pub added_by: String,
    pub added_at: i64,
}

/// Reduces `raw` to a lowercase `https://host` origin, or `None` when it is not
/// a plain site address (other schemes, credentials, paths, ports and
/// single-label hosts are rejected so a typo can't redirect visitors oddly).
pub fn normalize_origin(raw: &str) -> Option<String> {
    let t = raw.trim().trim_end_matches('/').to_lowercase();
    let host = t
        .strip_prefix("https://")
        .or_else(|| (!t.contains("://")).then_some(t.as_str()))?;
    let valid = host.contains('.')
        && host.len() <= 253
        && host
            .split('.')
            .all(|l| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-'))
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.');
    valid.then(|| format!("https://{host}"))
}

/// Validates the owner-chosen secondary ("alternate") address: any http(s) URL
/// with a host and no embedded credentials. Returned without a trailing `/`.
pub fn normalize_alternate(raw: &str) -> Option<String> {
    let t = raw.trim();
    let t = if t.contains("://") {
        t.to_string()
    } else {
        format!("https://{t}")
    };
    let u = url::Url::parse(&t).ok()?;
    let ok = matches!(u.scheme(), "http" | "https")
        && u.host_str().is_some_and(|h| !h.is_empty())
        && u.username().is_empty()
        && u.password().is_none();
    ok.then(|| u.as_str().trim_end_matches('/').to_string())
}

/// Adds an origin to the pool (no-op if present). False on a database error.
pub fn add(store: &DataStore, origin: &str, added_by: &str) -> bool {
    store
        .conn()
        .execute(
            "INSERT OR IGNORE INTO backup_domains (origin, active, added_by, added_at)
             VALUES (?1, 0, ?2, ?3)",
            rusqlite::params![origin, added_by, now_millis()],
        )
        .is_ok()
}

/// Removes an origin; true when it existed.
pub fn remove(store: &DataStore, origin: &str) -> bool {
    store
        .conn()
        .execute(
            "DELETE FROM backup_domains WHERE origin = ?1",
            rusqlite::params![origin],
        )
        .unwrap_or(0)
        > 0
}

/// Every pool entry, active one first.
pub fn list(store: &DataStore) -> Vec<BackupDomain> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(
        "SELECT origin, active, added_by, added_at FROM backup_domains
         ORDER BY active DESC, origin",
    ) else {
        return Vec::new();
    };
    let rows = stmt.query_map([], |r| {
        Ok(BackupDomain {
            origin: r.get(0)?,
            active: r.get::<_, i64>(1)? != 0,
            added_by: r.get(2)?,
            added_at: r.get(3)?,
        })
    });
    match rows {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// The origin visitors are told to fall back to, if one is active.
pub fn active(store: &DataStore) -> Option<String> {
    store
        .conn()
        .query_row(
            "SELECT origin FROM backup_domains WHERE active = 1 LIMIT 1",
            [],
            |r| r.get(0),
        )
        .ok()
}

/// Makes one specific pool entry the active one. False if it isn't in the pool.
pub fn set_active(store: &DataStore, origin: &str) -> bool {
    let conn = store.conn();
    let _ = conn.execute("BEGIN", []);
    let ok = conn
        .execute("UPDATE backup_domains SET active = 0", [])
        .is_ok()
        && conn
            .execute(
                "UPDATE backup_domains SET active = 1 WHERE origin = ?1",
                rusqlite::params![origin],
            )
            .unwrap_or(0)
            > 0;
    let _ = conn.execute(if ok { "COMMIT" } else { "ROLLBACK" }, []);
    ok
}

/// Switches the active backup to a random other origin in the pool and returns
/// it. `None` when the pool has no other origin to switch to.
pub fn rotate(store: &DataStore) -> Option<String> {
    let current = active(store);
    let others: Vec<String> = list(store)
        .into_iter()
        .map(|d| d.origin)
        .filter(|o| Some(o) != current.as_ref())
        .collect();
    if others.is_empty() {
        return None;
    }
    let pick = others[rand::random::<u32>() as usize % others.len()].clone();
    set_active(store, &pick).then_some(pick)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-site-domains-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn origins_are_normalized_and_junk_is_rejected() {
        assert_eq!(
            normalize_origin("MitchDog.com/"),
            Some("https://mitchdog.com".into())
        );
        assert_eq!(
            normalize_origin("https://a.b-c.example"),
            Some("https://a.b-c.example".into())
        );
        for bad in [
            "",
            "http://x.com",
            "https://x.com/path",
            "https://u@x.com",
            "localhost",
            "x.com:8080",
            "ftp://x.com",
            "-a.com",
        ] {
            assert_eq!(normalize_origin(bad), None, "{bad}");
        }
    }

    #[test]
    fn alternate_accepts_any_http_url_but_not_credentials() {
        assert_eq!(
            normalize_alternate("example.org"),
            Some("https://example.org".into())
        );
        assert_eq!(
            normalize_alternate("http://a.b:8080/x/"),
            Some("http://a.b:8080/x".into())
        );
        for bad in [
            "",
            "ftp://a.b",
            "https://u:p@a.b",
            "javascript:alert(1)",
            "https://",
        ] {
            assert_eq!(normalize_alternate(bad), None, "{bad}");
        }
    }

    #[test]
    fn rotate_picks_a_different_origin_and_needs_two() {
        let store = temp_store("rotate");
        assert_eq!(rotate(&store), None);
        assert!(add(&store, "https://a.example", "x"));
        assert_eq!(active(&store), None);
        assert_eq!(rotate(&store).as_deref(), Some("https://a.example"));
        assert_eq!(
            rotate(&store),
            None,
            "only one origin: nothing else to pick"
        );
        assert!(add(&store, "https://b.example", "x"));
        assert!(add(&store, "https://c.example", "x"));
        for _ in 0..10 {
            let before = active(&store);
            let next = rotate(&store).unwrap();
            assert_ne!(Some(next.clone()), before);
            assert_eq!(active(&store), Some(next));
            assert_eq!(list(&store).iter().filter(|d| d.active).count(), 1);
        }
        assert!(remove(&store, &active(&store).unwrap()));
        assert_eq!(active(&store), None);
        assert!(!set_active(&store, "https://gone.example"));
    }
}
