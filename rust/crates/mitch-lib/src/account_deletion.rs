//! Permanent, immediate removal of one account's stored data.
//!
//! [`erase_account`] is the data half of "Delete account" (`POST
//! /api/delete-account`). It runs synchronously: when it returns, nothing in
//! SQLite or the JSON document store identifies the account any more. The
//! server route (`routes/me/delete_account.rs`) wraps it with the parts that
//! need the network: destroying the computer, redacting Matrix history and
//! deactivating the Matrix account. See `docs/account-deletion.md`.
//!
//! Two passes:
//!
//! 1. **Tables.** [`EMAIL_TABLES`] lists every table column that holds an
//!    account's normalized email. Rows are deleted outright. Adding a table
//!    that stores an email means adding it here, or the data survives deletion
//!    (a test checks that every column named `*email*` in the schema is either
//!    listed or deliberately kept).
//! 2. **Documents.** The legacy JSON store (`json_documents`,
//!    `jsonl_documents`) has no schema, so [`scrub_value`] walks every document
//!    that mentions the account and drops matching keys, array entries and
//!    records. The walk is deliberately blunt: a DM, friend request or
//!    session record that names the account on either side is removed whole.
//!
//! Kept on purpose, because deleting them would let a banned person shed a
//! ban by deleting their account: `blacklist` and `banned_ips`. They hold the
//! email (and a hashed IP) until the ban is lifted.

use crate::auth::normalize_email;
use crate::data::DataStore;
use serde_json::Value;
use std::collections::HashSet;

/// `(table, column)` pairs deleted by exact normalized-email match.
pub const EMAIL_TABLES: &[(&str, &str)] = &[
    ("users", "email"),
    ("friends", "user_email"),
    ("friends", "friend_email"),
    ("friend_lists", "email"),
    ("support_tickets", "account_email"),
    ("support_tickets", "contact_email"),
    ("support_email_requests", "old_email"),
    ("support_email_requests", "new_email"),
    ("feedback_entries", "account"),
    ("referrals", "email"),
    ("invite_codes", "email"),
    ("invite_claims", "email"),
    ("invite_sent", "sender_email"),
    ("invite_sent", "recipient_email"),
    ("mini_game_sessions", "email"),
    ("e2e_keys", "email"),
    ("dm_cleared", "email"),
    ("tester_settings", "email"),
    ("daily_logins", "email"),
    ("coin_gift_notices", "email"),
    ("push_subs", "email"),
    ("cosmetics", "email"),
    ("achievements", "email"),
    ("passwords", "email"),
    ("user_stats", "email"),
    ("profiles", "email"),
    ("coins", "email"),
    ("newsletter_unsub", "email"),
    ("auth_sessions", "norm_email"),
    ("auth_sessions", "email"),
    ("names", "norm_email"),
    ("names", "email"),
    ("companion_characters", "creator_email"),
    ("cheat_logs", "email"),
    ("admin_passphrase_resets", "email"),
    ("matrix_outside_approvals", "email"),
    ("matrix_reports", "reported_by"),
    ("reserved_usernames", "holder_email"),
    ("premium_email_requests", "email"),
    ("premium_email_prompt_dismissed", "email"),
    ("virtual_machines", "owner_email"),
    ("vm_usage_samples", "owner_email"),
    ("vm_admin_access_grants", "owner_email"),
    ("vm_audit_logs", "owner_email"),
    ("vm_audit_logs", "actor_email"),
    ("companion_user_settings", "email"),
    ("companion_coin_requests", "email"),
    ("companion_chat_messages", "email"),
    ("companion_memories", "email"),
    ("companion_subscriptions", "email"),
    ("companion_character_unlocks", "email"),
    ("companion_ability_strain", "email"),
    ("companion_upgrades", "email"),
    ("companion_bans", "email"),
];

/// Columns that hold an email but are intentionally not erased, with why.
/// Used only by the schema-coverage test.
pub const KEPT_EMAIL_COLUMNS: &[(&str, &str)] = &[
    // Ban evasion: see the module docs.
    ("blacklist", "email"),
    ("banned_ips", "email"),
    // Chat tombstones keep hiding Matrix history until redaction is confirmed;
    // the server removes them itself once the chat server has finished.
    ("account_chat_removals", "email"),
    ("account_chat_removal_ids", "email"),
    ("moderated_media", "email"),
    // Staff records. Staff accounts cannot delete themselves.
    ("moderators", "email"),
    ("moderators", "added_by"),
    // A keyed hash, not an address: see `mark_deleted`.
    ("deleted_account_marks", "email_hash"),
    ("virtual_machines", "owner_user_id"),
    // Other people's records that name a staff member, not the account holder.
    ("support_messages", "staff_email"),
    ("support_email_requests", "requested_by"),
    ("support_email_requests", "decided_by"),
    ("feedback_entries", "handled_by"),
    ("matrix_reports", "handled_by"),
    ("premium_email_requests", "decided_by"),
    ("matrix_outside_approvals", "decided_by"),
    ("reserved_usernames", "reserved_by"),
];

/// JSON object fields that name an account. A record (or array entry) whose
/// field matches is removed whole.
const IDENTITY_FIELDS: &[&str] = &[
    "email",
    "norm_email",
    "normEmail",
    "from",
    "to",
    "user",
    "userEmail",
    "owner",
    "ownerEmail",
    "author",
    "authorEmail",
    "sender",
    "recipient",
    "creator",
    "creatorEmail",
    "account",
    "accountEmail",
    "target",
    "targetEmail",
    "requester",
    "buyer",
    "seller",
    "mediator",
    "player",
    "host",
    "username",
    "uid",
    "sid",
    "userId",
];

/// Documents the generic scrub must never touch.
const PROTECTED_DOCUMENTS: &[&str] = &[
    "site.json",
    "admins.json",
    "moderators.json",
    "shop_catalog.json",
];

/// What [`erase_account`] removed. Counts only; never contains personal data.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ErasureReport {
    pub rows: usize,
    pub documents: usize,
    pub log_lines: usize,
}

/// Everything that identifies the account inside stored data.
pub struct Identity {
    /// Normalized email.
    pub norm: String,
    /// Other strings that identify the account: session ids, Matrix user ids
    /// and localparts. Compared case-insensitively, exact match only.
    pub aliases: HashSet<String>,
}

impl Identity {
    pub fn new(norm: &str, aliases: impl IntoIterator<Item = String>) -> Self {
        Self {
            norm: normalize_email(norm),
            aliases: aliases
                .into_iter()
                .map(|a| a.to_lowercase())
                .filter(|a| a.len() >= 3)
                .collect(),
        }
    }

    fn matches(&self, text: &str) -> bool {
        if text.contains('@') && !text.starts_with('@') {
            if normalize_email(text) == self.norm {
                return true;
            }
        }
        !self.aliases.is_empty() && self.aliases.contains(&text.to_lowercase())
    }
}

/// Keyed hash identifying a deleted address without storing it. `secret` is the
/// site's ID secret, so the hash cannot be reversed or matched by guessing
/// addresses without it.
pub fn deleted_mark_hash(secret: &[u8], norm: &str) -> String {
    crate::crypto::hmac_sha256_hex(secret, format!("deleted-account:{norm}").as_bytes())
}

/// Remembers that an address was deleted, so one-time rewards cannot be earned
/// again by deleting and re-registering (the referral bonus pays 2000 coins to
/// both sides of an invite). Holds only a keyed hash and a timestamp.
pub fn mark_deleted(store: &DataStore, secret: &[u8], norm: &str) {
    let _ = store.conn().execute(
        "INSERT OR IGNORE INTO deleted_account_marks(email_hash, deleted_at) VALUES (?1, ?2)",
        rusqlite::params![deleted_mark_hash(secret, norm), crate::school::now_millis()],
    );
}

/// Whether this address was ever deleted. New accounts on such an address get
/// no one-time rewards.
pub fn was_deleted(store: &DataStore, secret: &[u8], norm: &str) -> bool {
    store
        .conn()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM deleted_account_marks WHERE email_hash = ?1)",
            [deleted_mark_hash(secret, norm)],
            |r| r.get(0),
        )
        .unwrap_or(false)
}

/// Session ids that resolve to `norm`. Collect before [`erase_account`], which
/// deletes the `names` rows that map them.
pub fn session_ids_for(store: &DataStore, norm: &str) -> Vec<String> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare("SELECT sid FROM names WHERE norm_email = ?1") else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([norm], |r| r.get::<_, String>(0)) else {
        return Vec::new();
    };
    rows.filter_map(|r| r.ok()).collect()
}

/// Removes everything that names `identity` from `value`. Returns whether
/// anything changed.
pub fn scrub_value(value: &mut Value, identity: &Identity) -> bool {
    scrub_collect(value, identity, &mut Vec::new())
}

/// Like [`scrub_value`], and pushes each removed entry onto `removed` (map
/// entries as `{key: value}`). The data export uses it to gather what deletion
/// would remove.
pub fn scrub_collect(value: &mut Value, identity: &Identity, removed: &mut Vec<Value>) -> bool {
    let mut changed = false;
    match value {
        Value::Object(map) => {
            let doomed: Vec<String> = map
                .iter()
                .filter(|(key, child)| identity.matches(key) || names_identity(child, identity))
                .map(|(key, _)| key.clone())
                .collect();
            for key in doomed {
                if let Some(child) = map.remove(&key) {
                    removed.push(serde_json::json!({ key: child }));
                    changed = true;
                }
            }
            for child in map.values_mut() {
                changed |= scrub_collect(child, identity, removed);
            }
        }
        Value::Array(list) => {
            let mut kept = Vec::with_capacity(list.len());
            for child in list.drain(..) {
                if names_identity(&child, identity) {
                    removed.push(child);
                    changed = true;
                } else {
                    kept.push(child);
                }
            }
            *list = kept;
            for child in list.iter_mut() {
                changed |= scrub_collect(child, identity, removed);
            }
        }
        _ => {}
    }
    changed
}

/// True when `value` is the account itself: a string equal to the identity, or
/// a record with an identity field that is.
fn names_identity(value: &Value, identity: &Identity) -> bool {
    match value {
        Value::String(s) => identity.matches(s),
        Value::Object(map) => map.iter().any(|(key, field)| {
            IDENTITY_FIELDS.contains(&key.as_str())
                && field.as_str().is_some_and(|s| identity.matches(s))
        }),
        _ => false,
    }
}

/// Deletes the account's rows and document entries. Idempotent.
pub fn erase_account(store: &DataStore, identity: &Identity) -> ErasureReport {
    let mut report = ErasureReport::default();
    let norm = identity.norm.as_str();
    if norm.is_empty() {
        return report;
    }
    {
        let conn = store.conn();
        // Children first: they are found through their parent rows.
        let child_sql = [
            "DELETE FROM support_messages WHERE ticket_id IN \
             (SELECT id FROM support_tickets WHERE account_email = ?1 OR contact_email = ?1)",
            "DELETE FROM support_email_requests WHERE ticket_id IN \
             (SELECT id FROM support_tickets WHERE account_email = ?1 OR contact_email = ?1)",
            "DELETE FROM companion_character_images WHERE character_id IN \
             (SELECT id FROM companion_characters WHERE creator_email = ?1)",
            "DELETE FROM companion_characters WHERE creator_email = ?1",
            "DELETE FROM tokens WHERE json_valid(data) AND \
             (lower(json_extract(data, '$.email')) = ?1 OR lower(json_extract(data, '$.norm_email')) = ?1)",
        ];
        for sql in child_sql {
            report.rows += conn.execute(sql, [norm]).unwrap_or(0);
        }
        for (table, column) in EMAIL_TABLES {
            let sql = format!("DELETE FROM {table} WHERE {column} = ?1");
            report.rows += conn.execute(&sql, [norm]).unwrap_or(0);
        }
        // Matrix chat reports filed against the account's Matrix ids.
        for alias in identity.aliases.iter().filter(|a| a.starts_with('@')) {
            report.rows += conn
                .execute(
                    "DELETE FROM matrix_reports WHERE lower(sender) = ?1",
                    [alias],
                )
                .unwrap_or(0);
        }
        // Session ids resolve to nothing once their `names` rows are gone, but
        // drop the sid rows too.
        for alias in &identity.aliases {
            report.rows += conn
                .execute("DELETE FROM names WHERE lower(sid) = ?1", [alias])
                .unwrap_or(0);
            report.rows += conn
                .execute("DELETE FROM revoked_ids WHERE lower(sid) = ?1", [alias])
                .unwrap_or(0);
        }
        // Application logs may quote the address in a message or its details.
        let like = format!("%{}%", escape_like(norm));
        report.rows += conn
            .execute(
                "DELETE FROM app_logs WHERE message LIKE ?1 ESCAPE '\\' OR details LIKE ?1 ESCAPE '\\'",
                [&like],
            )
            .unwrap_or(0);
    }
    let (docs, lines) = scrub_documents(store, identity);
    report.documents = docs;
    report.log_lines = lines;
    report
}

fn escape_like(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Rewrites every JSON document and drops every JSONL line that names the
/// account. Returns `(documents changed, lines dropped)`.
fn scrub_documents(store: &DataStore, identity: &Identity) -> (usize, usize) {
    let mut needles: Vec<String> = vec![identity.norm.clone()];
    needles.extend(identity.aliases.iter().cloned());
    // The normalized form drops dots and "+tags", so a document may spell the
    // address differently. Match on the local part before the `@` as well.
    let local = identity.norm.split('@').next().unwrap_or("").to_string();
    let mut changed_docs = 0;
    let mut dropped = 0;

    let conn = store.conn();
    let paths: Vec<(String, String)> = {
        let Ok(mut stmt) = conn.prepare("SELECT path, content FROM json_documents") else {
            return (0, 0);
        };
        let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        else {
            return (0, 0);
        };
        rows.filter_map(|r| r.ok()).collect()
    };
    for (path, content) in paths {
        let file = path.rsplit('/').next().unwrap_or(&path);
        if PROTECTED_DOCUMENTS.contains(&file) {
            continue;
        }
        let lower = content.to_lowercase();
        let mentioned = needles.iter().any(|n| lower.contains(n.as_str()))
            || (local.len() >= 3 && lower.contains(&local));
        if !mentioned {
            continue;
        }
        let Ok(mut value) = serde_json::from_str::<Value>(&content) else {
            continue;
        };
        if scrub_value(&mut value, identity) {
            let _ = conn.execute(
                "UPDATE json_documents SET content = ?1, updated_at = ?2 WHERE path = ?3",
                rusqlite::params![
                    crate::data::js_stringify_pretty(&value),
                    crate::school::now_millis(),
                    path
                ],
            );
            changed_docs += 1;
        }
    }

    let lines: Vec<(String, String)> = {
        let Ok(mut stmt) = conn.prepare("SELECT path, content FROM jsonl_documents") else {
            return (changed_docs, 0);
        };
        let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        else {
            return (changed_docs, 0);
        };
        rows.filter_map(|r| r.ok()).collect()
    };
    for (path, content) in lines {
        let kept: Vec<&str> = content
            .lines()
            .filter(|line| {
                let lower = line.to_lowercase();
                !needles.iter().any(|n| lower.contains(n.as_str()))
            })
            .collect();
        let removed = content.lines().count() - kept.len();
        if removed > 0 {
            let mut text = kept.join("\n");
            if !text.is_empty() {
                text.push('\n');
            }
            let _ = conn.execute(
                "UPDATE jsonl_documents SET content = ?1, updated_at = ?2 WHERE path = ?3",
                rusqlite::params![text, crate::school::now_millis(), path],
            );
            dropped += removed;
        }
    }
    (changed_docs, dropped)
}

/// Tables whose rows are left out of the export because they hold secrets
/// (password hash, session tokens, push endpoints and keys, private keys) or
/// only mirror other data.
const EXPORT_SKIP_TABLES: &[&str] = &[
    "passwords",
    "auth_sessions",
    "names",
    "push_subs",
    "e2e_keys",
    "tokens",
];

/// Everything the site stores about the account, for the "Download your
/// information" page: each row of [`EMAIL_TABLES`] (minus secrets) plus the
/// entries of the JSON documents that [`erase_account`] would remove. Built
/// from the same lists as deletion so the two cannot drift apart.
pub fn export_account(store: &DataStore, identity: &Identity) -> Value {
    let norm = identity.norm.as_str();
    let mut tables = serde_json::Map::new();
    {
        let conn = store.conn();
        for (table, column) in EMAIL_TABLES {
            if EXPORT_SKIP_TABLES.contains(table) {
                continue;
            }
            let sql = format!("SELECT * FROM {table} WHERE {column} = ?1");
            let Ok(mut stmt) = conn.prepare(&sql) else {
                continue;
            };
            let names: Vec<String> = stmt.column_names().iter().map(|n| n.to_string()).collect();
            let Ok(rows) = stmt.query_map([norm], |row| {
                let mut object = serde_json::Map::new();
                for (index, name) in names.iter().enumerate() {
                    let value = match row.get_ref(index)? {
                        rusqlite::types::ValueRef::Null => Value::Null,
                        rusqlite::types::ValueRef::Integer(n) => Value::from(n),
                        rusqlite::types::ValueRef::Real(n) => Value::from(n),
                        rusqlite::types::ValueRef::Text(t) => {
                            let text = String::from_utf8_lossy(t).into_owned();
                            // JSON stored as text (profile blobs) reads better parsed.
                            serde_json::from_str::<Value>(&text)
                                .ok()
                                .filter(|v| v.is_object() || v.is_array())
                                .unwrap_or(Value::String(text))
                        }
                        rusqlite::types::ValueRef::Blob(_) => Value::Null,
                    };
                    object.insert(name.clone(), value);
                }
                Ok(Value::Object(object))
            }) else {
                continue;
            };
            let rows: Vec<Value> = rows.filter_map(|r| r.ok()).collect();
            if rows.is_empty() {
                continue;
            }
            let entry = tables
                .entry(table.to_string())
                .or_insert_with(|| Value::Array(Vec::new()));
            if let Some(list) = entry.as_array_mut() {
                for row in rows {
                    if !list.contains(&row) {
                        list.push(row);
                    }
                }
            }
        }
    }
    let mut documents = serde_json::Map::new();
    let conn = store.conn();
    if let Ok(mut stmt) = conn.prepare("SELECT path, content FROM json_documents") {
        let rows: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();
        for (path, content) in rows {
            let file = path.rsplit('/').next().unwrap_or(&path).to_string();
            if PROTECTED_DOCUMENTS.contains(&file.as_str())
                || EXPORT_SKIP_DOCUMENTS.contains(&file.as_str())
            {
                continue;
            }
            let Ok(mut value) = serde_json::from_str::<Value>(&content) else {
                continue;
            };
            let mut found = Vec::new();
            scrub_collect(&mut value, identity, &mut found);
            if !found.is_empty() {
                documents.insert(file, Value::Array(found));
            }
        }
    }
    serde_json::json!({ "tables": tables, "documents": documents })
}

/// Documents holding credentials or other people's data keyed by this account.
const EXPORT_SKIP_DOCUMENTS: &[&str] = &["passkeys.json", "tokens.json", "valid_hashes.json"];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn identity() -> Identity {
        Identity::new(
            "Gone+x@Example.com",
            ["uid-123".to_string(), "@gone:mitch.pro".to_string()],
        )
    }

    #[test]
    fn scrub_removes_keys_entries_and_records_but_keeps_other_people() {
        let mut doc = json!({
            "gone@example.com": {"coins": 5},
            "keep@example.com": {"coins": 9},
            "byUid": {"uid-123": 1, "uid-999": 2},
            "dms": [
                {"from": "gone@example.com", "to": "keep@example.com", "text": "a"},
                {"from": "keep@example.com", "to": "gone@example.com", "text": "b"},
                {"from": "keep@example.com", "to": "other@example.com", "text": "c"}
            ],
            "friends": ["gone@example.com", "keep@example.com"],
            "nested": {"owners": {"p1": "gone@example.com", "p2": "keep@example.com"}}
        });
        assert!(scrub_value(&mut doc, &identity()));
        assert!(doc.get("gone@example.com").is_none());
        assert!(doc["keep@example.com"].is_object());
        assert!(doc["byUid"].get("uid-123").is_none());
        assert_eq!(doc["byUid"]["uid-999"], 2);
        assert_eq!(doc["dms"].as_array().unwrap().len(), 1);
        assert_eq!(doc["dms"][0]["text"], "c");
        assert_eq!(doc["friends"], json!(["keep@example.com"]));
        assert!(doc["nested"]["owners"].get("p1").is_none());
        assert!(doc["nested"]["owners"].get("p2").is_some());
    }

    #[test]
    fn scrub_reports_no_change_for_unrelated_documents() {
        let mut doc = json!({"a@example.com": {"from": "b@example.com"}});
        assert!(!scrub_value(&mut doc, &identity()));
    }

    fn store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-account-deletion-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn erase_account_clears_tables_documents_logs_and_keeps_bans() {
        let store = store("erase");
        let id = Identity::new("gone@example.com", ["uid-123".to_string()]);
        {
            let conn = store.conn();
            for sql in [
                "INSERT INTO coins(email, balance, updated_at) VALUES ('gone@example.com', 5, 1)",
                "INSERT INTO coins(email, balance, updated_at) VALUES ('keep@example.com', 7, 1)",
                "INSERT INTO names(sid, email, norm_email, created_at) VALUES ('uid-123', 'gone@example.com', 'gone@example.com', 1)",
                "INSERT INTO app_logs(ts, level, category, message) VALUES (1, 'info', 'x', 'login gone@example.com')",
                "INSERT INTO app_logs(ts, level, category, message) VALUES (1, 'info', 'x', 'login keep@example.com')",
                "INSERT INTO blacklist(email, reason, banned_at) VALUES ('gone@example.com', 'r', 1)",
                "INSERT INTO json_documents(path, content, updated_at) VALUES ('data/dms.json', '[{\"from\":\"gone@example.com\",\"text\":\"x\"},{\"from\":\"keep@example.com\",\"text\":\"y\"}]', 1)",
                "INSERT INTO jsonl_documents(path, content, updated_at) VALUES ('data/log.jsonl', 'gone@example.com a\nkeep@example.com b\n', 1)",
            ] {
                conn.execute(sql, []).unwrap_or_else(|e| panic!("{sql}: {e}"));
            }
        }
        let report = erase_account(&store, &id);
        assert!(report.rows >= 3, "{report:?}");
        assert_eq!(report.documents, 1);
        assert_eq!(report.log_lines, 1);
        let conn = store.conn();
        let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(
            count("SELECT COUNT(*) FROM coins WHERE email='gone@example.com'"),
            0
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM coins WHERE email='keep@example.com'"),
            1
        );
        assert_eq!(count("SELECT COUNT(*) FROM names"), 0);
        assert_eq!(count("SELECT COUNT(*) FROM app_logs"), 1);
        assert_eq!(count("SELECT COUNT(*) FROM blacklist"), 1);
        let dms: String = conn
            .query_row(
                "SELECT content FROM json_documents WHERE path='data/dms.json'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!dms.contains("gone@example.com") && dms.contains("keep@example.com"));
        let log: String = conn
            .query_row(
                "SELECT content FROM jsonl_documents WHERE path='data/log.jsonl'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(log, "keep@example.com b\n");
    }

    #[test]
    fn export_returns_the_account_rows_without_secrets_or_other_people() {
        let store = store("export");
        let id = Identity::new("gone@example.com", []);
        {
            let conn = store.conn();
            for sql in [
                "INSERT INTO coins(email, balance, updated_at) VALUES ('gone@example.com', 5, 1)",
                "INSERT INTO coins(email, balance, updated_at) VALUES ('keep@example.com', 7, 1)",
                "INSERT INTO passwords(email, hash, updated_at) VALUES ('gone@example.com', 'secret-hash', 1)",
                "INSERT INTO json_documents(path, content, updated_at) VALUES ('data/dms.json', '[{\"from\":\"gone@example.com\",\"text\":\"x\"},{\"from\":\"keep@example.com\",\"text\":\"y\"}]', 1)",
            ] {
                conn.execute(sql, []).unwrap_or_else(|e| panic!("{sql}: {e}"));
            }
        }
        let out = export_account(&store, &id);
        let text = out.to_string();
        assert!(!text.contains("secret-hash") && !text.contains("keep@example.com"));
        assert_eq!(out["tables"]["coins"][0]["balance"], 5.0);
        assert_eq!(out["documents"]["dms.json"][0]["text"], "x");
        // Exporting does not modify anything.
        assert!(
            store
                .conn()
                .query_row("SELECT COUNT(*) FROM coins", [], |r| r.get::<_, i64>(0))
                .unwrap()
                == 2
        );
    }

    #[test]
    fn deleted_addresses_are_remembered_only_as_a_keyed_hash() {
        let store = store("marks");
        assert!(!was_deleted(&store, b"k", "gone@example.com"));
        mark_deleted(&store, b"k", "gone@example.com");
        assert!(was_deleted(&store, b"k", "gone@example.com"));
        assert!(!was_deleted(&store, b"other-key", "gone@example.com"));
        let stored: String = store
            .conn()
            .query_row("SELECT email_hash FROM deleted_account_marks", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(!stored.contains("gone"));
    }

    /// Fails when someone adds a table with an email column and forgets to
    /// decide whether deletion should remove it.
    #[test]
    fn every_email_column_in_the_schema_is_erased_or_deliberately_kept() {
        let store = store("schema");
        let conn = store.conn();
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        let mut missing = Vec::new();
        for table in tables {
            let cols: Vec<String> = conn
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap()
                .query_map([], |r| r.get::<_, String>(1))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect();
            for col in cols.iter().filter(|c| {
                let c = c.to_lowercase();
                c.contains("email") || c == "account" || c == "reported_by"
            }) {
                let known = EMAIL_TABLES
                    .iter()
                    .chain(KEPT_EMAIL_COLUMNS)
                    .any(|(t, c)| *t == table && c == col);
                if !known {
                    missing.push(format!("{table}.{col}"));
                }
            }
        }
        assert!(
            missing.is_empty(),
            "unclassified email columns: {missing:?}"
        );
    }
}
