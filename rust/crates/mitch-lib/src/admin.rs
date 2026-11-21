//! Shared admin-layer helpers — port of server.js's admin support code:
//! logAdminAction (3149-3159), maskEmail (1352), admin passphrase store
//! (2602-2635), publicSessionId (3144), publicProfileReports (3161),
//! buildAdvancedAdminData (3180-3274), blog contributors (6253), moderator
//! panel config (6596-6613), the moderator action vocabulary (6615-6732),
//! moderator request store (6734-6786), and canGrantPremium* (7008-7024).

use crate::auth::normalize_email;
use crate::data::DataStore;
use serde_json::{json, Value};
use std::path::Path;

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `logAdminAction(actor, action, details)` — one row in the `admin_actions`
/// table (formerly admin_actions.json): who did what, when, with what
/// parameters. Every moderation and admin action is kept; there is no cap.
/// Details must not include IP addresses or user agents (docs/privacy.md).
pub fn log_admin_action(
    store: &DataStore,
    _data_dir: &Path,
    actor: &str,
    action: &str,
    details: Value,
) {
    insert_admin_action(
        store,
        &crate::crypto::random_bytes_hex(8),
        now_millis(),
        if actor.is_empty() { "admin" } else { actor },
        action,
        &details,
    );
}

fn insert_admin_action(
    store: &DataStore,
    id: &str,
    ts: i64,
    actor: &str,
    action: &str,
    details: &Value,
) -> bool {
    let conn = store.conn();
    conn.execute(
        "INSERT OR IGNORE INTO admin_actions (id, ts, actor, action, details) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![id, ts, actor, action, details.to_string()],
    )
    .unwrap_or(0)
        > 0
}

fn admin_action_rows(store: &DataStore, sql: &str, params: &[&dyn rusqlite::ToSql]) -> Vec<Value> {
    let conn = store.conn();
    conn.prepare(sql)
        .and_then(|mut stmt| {
            stmt.query_map(params, |r| {
                let details: String = r.get(4)?;
                Ok(json!({
                    "id": r.get::<_, String>(0)?,
                    "ts": r.get::<_, i64>(1)?,
                    "actor": r.get::<_, String>(2)?,
                    "action": r.get::<_, String>(3)?,
                    "details": serde_json::from_str::<Value>(&details).unwrap_or_else(|_| json!({})),
                }))
            })
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
        })
        .unwrap_or_default()
}

/// Newest staff actions first, `{id, ts, actor, action, details}`.
pub fn admin_actions_recent(store: &DataStore, limit: i64) -> Vec<Value> {
    admin_action_rows(
        store,
        "SELECT id, ts, actor, action, details FROM admin_actions ORDER BY ts DESC, rowid DESC LIMIT ?1",
        &[&limit],
    )
}

/// Filters for [`search_admin_actions`]. Empty text means "any". `person` matches
/// the staff member who acted or anything in the action's details (so an affected
/// account's email is found too); `action` matches part of the action name.
#[derive(Default)]
pub struct ActionQuery<'a> {
    pub person: &'a str,
    pub action: &'a str,
    /// Inclusive bounds in epoch milliseconds.
    pub from: Option<i64>,
    pub to: Option<i64>,
}

/// `%text%` for a `LIKE ... ESCAPE '\'` clause, with `\`, `%` and `_` literal.
fn like_contains(text: &str) -> String {
    let mut out = String::from("%");
    for c in text.trim().chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

/// Staff actions matching `q`, newest first, plus how many matched in total (the
/// list is capped at `limit`). Backs the dashboard's activity log search.
pub fn search_admin_actions(
    store: &DataStore,
    q: &ActionQuery<'_>,
    limit: i64,
) -> (Vec<Value>, i64) {
    let person = like_contains(q.person);
    let action = like_contains(q.action);
    let from = q.from.unwrap_or(i64::MIN);
    let to = q.to.unwrap_or(i64::MAX);
    let filter = "(actor LIKE ?1 ESCAPE '\\' OR details LIKE ?1 ESCAPE '\\')
                  AND action LIKE ?2 ESCAPE '\\' AND ts >= ?3 AND ts <= ?4";
    let total: i64 = store
        .conn()
        .query_row(
            &format!("SELECT COUNT(*) FROM admin_actions WHERE {filter}"),
            rusqlite::params![person, action, from, to],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let rows = admin_action_rows(
        store,
        &format!(
            "SELECT id, ts, actor, action, details FROM admin_actions WHERE {filter}
              ORDER BY ts DESC, rowid DESC LIMIT ?5"
        ),
        &[&person, &action, &from, &to, &limit],
    );
    (rows, total)
}

/// Distinct action names, for the search's action picker.
pub fn admin_action_names(store: &DataStore) -> Vec<String> {
    let conn = store.conn();
    conn.prepare("SELECT DISTINCT action FROM admin_actions ORDER BY action")
        .and_then(|mut stmt| {
            stmt.query_map([], |r| r.get::<_, String>(0))
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
        })
        .unwrap_or_default()
}

/// Newest staff actions whose action name starts with `prefix`.
pub fn admin_actions_with_prefix(store: &DataStore, prefix: &str, limit: i64) -> Vec<Value> {
    let pattern = format!("{}%", prefix.replace('%', "").replace('_', "\\_"));
    admin_action_rows(
        store,
        "SELECT id, ts, actor, action, details FROM admin_actions WHERE action LIKE ?1 ESCAPE '\\'
          ORDER BY ts DESC, rowid DESC LIMIT ?2",
        &[&pattern, &limit],
    )
}

/// One staff action by id.
pub fn admin_action_by_id(store: &DataStore, id: &str) -> Option<Value> {
    admin_action_rows(
        store,
        "SELECT id, ts, actor, action, details FROM admin_actions WHERE id = ?1",
        &[&id],
    )
    .into_iter()
    .next()
}

/// Startup migration: moves admin_actions.json into the `admin_actions`
/// table, dropping the staff IP / user agent older entries carried, then
/// deletes the document. Idempotent. Returns the number imported.
pub fn migrate_admin_actions_from_json(store: &DataStore) -> usize {
    let file = store.data_dir().join("admin_actions.json");
    let legacy = store.read_document(&file, Value::Null);
    let mut imported = 0;
    if let Some(entries) = legacy.as_array() {
        for e in entries {
            let mut details = e.get("details").cloned().unwrap_or_else(|| json!({}));
            if let Some(obj) = details.as_object_mut() {
                obj.remove("ip");
                obj.remove("userAgent");
            }
            let id = e
                .get("id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| crate::crypto::random_bytes_hex(8));
            let actor = e.get("actor").and_then(Value::as_str).unwrap_or("admin");
            let action = e
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("admin_action");
            let ts = e.get("ts").and_then(Value::as_i64).unwrap_or(0);
            if insert_admin_action(store, &id, ts, actor, action, &details) {
                imported += 1;
            }
        }
    }
    crate::privacy::delete_document(store, &file);
    imported
}

/// How long anti-cheat flags are kept, and the most rows kept at once.
pub const CHEAT_LOG_RETENTION_MS: i64 = 90 * 86_400_000;
pub const CHEAT_LOG_MAX_ROWS: i64 = 1000;

/// `logCheat(email, game, details)` — one row in the `cheat_logs` table
/// (formerly cheat_logs.json). Records which account tripped which check and
/// why; no IP address or device details. Rows older than
/// [`CHEAT_LOG_RETENTION_MS`] and beyond the newest [`CHEAT_LOG_MAX_ROWS`] are
/// deleted on every write.
pub fn log_cheat(store: &DataStore, email: &str, game: &str, details: &str) {
    let now = now_millis();
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO cheat_logs (ts, email, game, details) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            now,
            if email.is_empty() { "unknown" } else { email },
            if game.is_empty() { "unknown" } else { game },
            details,
        ],
    );
    prune_cheat_logs(&conn, now);
}

fn prune_cheat_logs(conn: &rusqlite::Connection, now: i64) {
    let _ = conn.execute(
        "DELETE FROM cheat_logs WHERE ts < ?1",
        rusqlite::params![now - CHEAT_LOG_RETENTION_MS],
    );
    let _ = conn.execute(
        "DELETE FROM cheat_logs WHERE id NOT IN (SELECT id FROM cheat_logs ORDER BY ts DESC, id DESC LIMIT ?1)",
        rusqlite::params![CHEAT_LOG_MAX_ROWS],
    );
}

/// Newest cheat-log rows (`{email, game, details, ts}`) and the total count.
pub fn cheat_logs_recent(store: &DataStore, limit: i64) -> (Vec<Value>, usize) {
    let conn = store.conn();
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM cheat_logs", [], |r| r.get(0))
        .unwrap_or(0);
    let rows = conn
        .prepare(
            "SELECT ts, email, game, details FROM cheat_logs ORDER BY ts DESC, id DESC LIMIT ?1",
        )
        .and_then(|mut stmt| {
            stmt.query_map(rusqlite::params![limit], |r| {
                Ok(json!({
                    "ts": r.get::<_, i64>(0)?,
                    "email": r.get::<_, String>(1)?,
                    "game": r.get::<_, String>(2)?,
                    "details": r.get::<_, String>(3)?,
                }))
            })
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
        })
        .unwrap_or_default();
    (rows, total.max(0) as usize)
}

/// Startup migration: moves cheat_logs.json into the `cheat_logs` table,
/// dropping the IP addresses it held, then deletes the JSON document.
/// Returns the number of rows imported.
pub fn migrate_cheat_logs_from_json(store: &DataStore) -> usize {
    let file = store.data_dir().join("cheat_logs.json");
    let legacy = store.read_document(&file, Value::Null);
    let mut imported = 0;
    if let Some(entries) = legacy.as_array() {
        let conn = store.conn();
        for e in entries {
            let ts = e.get("ts").and_then(Value::as_i64).unwrap_or(0);
            let text = |k: &str| e.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            if conn
                .execute(
                    "INSERT INTO cheat_logs (ts, email, game, details) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![ts, text("email"), text("game"), text("details")],
                )
                .is_ok()
            {
                imported += 1;
            }
        }
        prune_cheat_logs(&conn, now_millis());
    }
    crate::privacy::delete_document(store, &file);
    imported
}

/// `maskEmail(email)` — server.js:1352. Despite the name this is an IDENTITY
/// function (the JS predates masking); empty input returns 'anonymous'.
pub fn mask_email(email: &str) -> String {
    if email.is_empty() {
        return "anonymous".to_string();
    }
    email.to_string()
}

/// `loadAdminPassphrase()` — data/admin_passphrase.json map.
pub fn load_admin_passphrase(store: &DataStore, data_dir: &Path) -> Value {
    store.read_document(&data_dir.join("admin_passphrase.json"), json!({}))
}

/// `saveAdminPassphraseForUser(norm, entry)`.
pub fn save_admin_passphrase_for_user(
    store: &DataStore,
    data_dir: &Path,
    norm: &str,
    entry: Value,
) {
    let mut data = load_admin_passphrase(store, data_dir);
    if let Some(map) = data.as_object_mut() {
        map.insert(norm.to_string(), entry);
    }
    let _ = store.write_document(&data_dir.join("admin_passphrase.json"), &data);
}

/// The stored Argon2 hash for the admin behind `sid`: the per-user entry,
/// falling back to the shared `admin@mitch.pro` one. Empty when none is set.
pub fn admin_passphrase_hash_for_sid(
    store: &DataStore,
    id_secret: &[u8],
    data_dir: &Path,
    sid: &str,
) -> String {
    let email =
        crate::auth::email_from_sid(store, id_secret, sid).unwrap_or_else(|| "admin".to_string());
    let norm = normalize_email(&email);
    let data = load_admin_passphrase(store, data_dir);
    let hash_of = |key: &str| {
        data.get(key)
            .and_then(|e| e.get("hash"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let hash = hash_of(&norm);
    if hash.is_empty() {
        hash_of("admin@mitch.pro")
    } else {
        hash
    }
}

/// `verifyAdminPassphraseRaw(sid, passphrase)` — per-user hash with the
/// admin@mitch.pro fallback; argon2 PHC verify. Returns false on any failure.
pub fn verify_admin_passphrase_raw(
    store: &DataStore,
    id_secret: &[u8],
    data_dir: &Path,
    sid: &str,
    passphrase: &str,
) -> bool {
    let hash = admin_passphrase_hash_for_sid(store, id_secret, data_dir, sid);
    if hash.is_empty() {
        return false;
    }
    let pass = passphrase.trim();
    if pass.is_empty() {
        return false;
    }
    crate::crypto::argon2_verify(&hash, pass)
}

/// Name of the cookie that keeps the admin dashboard unlocked.
pub const ADMIN_UNLOCK_COOKIE: &str = "mitch_admin_unlock";

/// How long one passphrase entry keeps the admin pages unlocked.
pub const ADMIN_UNLOCK_TTL_MS: i64 = 8 * 60 * 60 * 1000;

/// MAC over what an unlock grant is bound to: the session id, the current
/// passphrase hash (so changing the passphrase revokes every grant) and the
/// expiry. Keyed with the server's id secret.
fn admin_unlock_mac(id_secret: &[u8], sid: &str, hash: &str, expires: i64) -> String {
    let fingerprint = crate::crypto::sha256_hex(hash.as_bytes());
    let data = format!("admin-unlock|{sid}|{fingerprint}|{expires}");
    crate::crypto::hmac_sha256_hex(id_secret, data.as_bytes())
}

/// Cookie value proving `sid` entered the admin passphrase recently:
/// `<expires-ms>.<mac>`. Stateless, so nothing is stored server-side;
/// logging out (new sid) or changing the passphrase invalidates it.
pub fn make_admin_unlock_token(
    store: &DataStore,
    id_secret: &[u8],
    data_dir: &Path,
    sid: &str,
) -> Option<(String, i64)> {
    let hash = admin_passphrase_hash_for_sid(store, id_secret, data_dir, sid);
    if hash.is_empty() {
        return None;
    }
    let expires = now_millis() + ADMIN_UNLOCK_TTL_MS;
    let mac = admin_unlock_mac(id_secret, sid, &hash, expires);
    Some((format!("{expires}.{mac}"), expires))
}

/// Returns the unlock's expiry (ms) when `token` is a live grant for `sid`.
pub fn verify_admin_unlock_token(
    store: &DataStore,
    id_secret: &[u8],
    data_dir: &Path,
    sid: &str,
    token: &str,
) -> Option<i64> {
    let (expires, mac) = token.split_once('.')?;
    let expires: i64 = expires.parse().ok()?;
    if expires <= now_millis() {
        return None;
    }
    let hash = admin_passphrase_hash_for_sid(store, id_secret, data_dir, sid);
    if hash.is_empty() {
        return None;
    }
    let expected = admin_unlock_mac(id_secret, sid, &hash, expires);
    if expected.len() != mac.len() {
        return None;
    }
    // Constant-time compare.
    let diff = expected
        .bytes()
        .zip(mac.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    (diff == 0).then_some(expires)
}

/// Admin actions that must be confirmed with the passphrase every time,
/// even while the dashboard is unlocked: destructive or hard-to-undo work.
/// Everything else under `/api/admin/` is covered by the unlock cookie.
pub fn is_sensitive_admin_action(method: &str, path: &str) -> bool {
    if !matches!(method, "POST" | "PUT" | "PATCH" | "DELETE") {
        return false;
    }
    const EXACT: &[&str] = &[
        "/api/admin/ban-account",
        "/api/admin/unban-account",
        "/api/admin/bans",
        "/api/admin/vm-ban",
        "/api/admin/byo-os-ban",
        "/api/admin/admins",
        "/api/admin/co-owners",
        "/api/admin/moderators",
        "/api/admin/change-passphrase",
        "/api/admin/reset-other-passphrase",
        "/api/admin/audit/revert",
        "/api/admin/broadcast",
        "/api/admin/newsletters/send",
        "/api/admin/maintenance-toggle",
        "/api/admin/restricted-mode",
        "/api/admin/economy/burn",
        "/api/admin/economy/cap-coins",
        "/api/admin/economy/set-coins",
        "/api/admin/economy/multiplier",
        "/api/admin/gift-coins",
        "/api/admin/grant-premium",
        "/api/admin/revoke-premium",
        "/api/admin/matrix/purge-user",
        "/api/admin/owner-container",
        "/api/admin/lxc-attach-sshd-hook",
        "/api/admin/vms/delete",
        "/api/admin/vms/fleet/delete-chromeos",
        "/api/admin/vms/fleet/power",
        "/api/admin/terminate-vm",
        "/api/admin/blog-deletions",
    ];
    EXACT.contains(&path)
}

/// `publicSessionId(id)` — sha256(id).hex[0..12] or 'unknown'.
pub fn public_session_id(id: &str) -> String {
    if id.is_empty() {
        return "unknown".to_string();
    }
    crate::crypto::sha256_hex(id.as_bytes())[..12].to_string()
}

/// `publicProfileReports(limit)` — masked profile-report view.
pub fn public_profile_reports(store: &DataStore, data_dir: &Path, limit: usize) -> Vec<Value> {
    let reports = store
        .read_document(&data_dir.join("profile_reports.json"), json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let start = reports.len().saturating_sub(limit);
    reports[start..]
        .iter()
        .rev()
        .map(|report| {
            json!({
                "id": report.get("id").map(|v| v.as_str().unwrap_or("").to_string()).unwrap_or_default(),
                "ts": report.get("ts").and_then(|v| v.as_i64()).unwrap_or(0),
                "status": report.get("status").and_then(|v| v.as_str()).unwrap_or("Needs review"),
                "reporter": mask_email(report.get("reporterEmail").and_then(|v| v.as_str()).unwrap_or("")),
                "target": mask_email(report.get("targetEmail").and_then(|v| v.as_str()).unwrap_or("")),
                "targetHandle": report.get("targetHandle").map(|v| v.as_str().unwrap_or("").to_string()).unwrap_or_default(),
                "reason": report.get("reason").map(|v| v.as_str().unwrap_or("").chars().take(140).collect::<String>()).unwrap_or_default(),
                "details": report.get("details").map(|v| v.as_str().unwrap_or("").chars().take(800).collect::<String>()).unwrap_or_default(),
                "resolvedBy": report.get("resolvedBy").and_then(|v| v.as_str()).map(mask_email).unwrap_or_default(),
                "resolvedAt": report.get("resolvedAt").and_then(|v| v.as_i64()).unwrap_or(0),
                "resolution": report.get("resolution").map(|v| v.as_str().unwrap_or("").chars().take(300).collect::<String>()).unwrap_or_default(),
            })
        })
        .collect()
}

/// Notifications staff sent: admin_notice rows deduped on batchId+targetEmail,
/// newest first.
pub fn sent_notifications(store: &DataStore) -> Vec<Value> {
    let mut sent_notifications: Vec<Value> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (target_email, notice) in crate::coins::admin_notice_rows(store) {
        let key = format!(
            "{}:{}",
            notice
                .get("batchId")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| notice.get("id").and_then(|v| v.as_str()).unwrap_or("")),
            target_email
        );
        if !seen.insert(key) {
            continue;
        }
        sent_notifications.push(json!({
            "id": notice.get("id").map(|v| v.as_str().unwrap_or("").to_string()).unwrap_or_default(),
            "batchId": notice.get("batchId").map(|v| v.as_str().unwrap_or("").to_string()).unwrap_or_default(),
            "targetEmail": target_email,
            "title": notice.get("title").and_then(|v| v.as_str()).unwrap_or("Admin notification"),
            "from": notice.get("from").and_then(|v| v.as_str()).unwrap_or("admin"),
            "source": notice.get("source").and_then(|v| v.as_str()).unwrap_or("mitchdog.com"),
            "ts": notice.get("ts").and_then(|v| v.as_i64()).unwrap_or(0),
            "read": notice.get("read").and_then(|v| v.as_bool()).unwrap_or(false),
        }));
    }
    sent_notifications.sort_by(|a, b| {
        b.get("ts")
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            .cmp(&a.get("ts").and_then(|v| v.as_i64()).unwrap_or(0))
    });
    sent_notifications
}

/// `buildAdvancedAdminData()` — server.js:3180-3274, the admin dashboard
/// payload. Pure document reads (websockets not needed for any field).
/// Admin "advanced data" payload. `active_users` is the number of accounts
/// online right now, from the server's in-memory presence. It's a count only:
/// staff don't get a list of who is online or what they are doing.
pub fn build_advanced_admin_data(store: &DataStore, data_dir: &Path, active_users: usize) -> Value {
    let now = now_millis();

    let admin_actions = admin_actions_recent(store, 250);

    let chat_reports_raw = store
        .read_document(&data_dir.join("chat_reports.json"), json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let chat_reports_all: Vec<Value> = chat_reports_raw
        .iter()
        .rev()
        .take(150)
        .map(|report| {
            json!({
                "type": "chat",
                "ts": report.get("ts").and_then(|v| v.as_i64()).unwrap_or(0),
                "reportedBy": report.get("reportedBy").or_else(|| report.get("reporter")).and_then(|v| v.as_str()).unwrap_or(""),
                "reason": report.get("reason").and_then(|v| v.as_str()).unwrap_or("Chat report"),
                "context": report.get("context").cloned().unwrap_or(json!([])),
                "status": report.get("status").and_then(|v| v.as_str()).unwrap_or("Needs review"),
            })
        })
        .collect();
    let profile_reports = public_profile_reports(store, data_dir, 250);

    let sent_notifications = sent_notifications(store);

    let mut banned_accounts: Vec<Value> = crate::bans::blacklist_all(store);
    banned_accounts.sort_by(|a, b| {
        b.get("bannedAt")
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            .cmp(&a.get("bannedAt").and_then(|v| v.as_i64()).unwrap_or(0))
    });

    let chat_reports_top: Vec<Value> = {
        let mut sorted = chat_reports_all.clone();
        sorted.sort_by(|a, b| {
            b.get("ts")
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                .cmp(&a.get("ts").and_then(|v| v.as_i64()).unwrap_or(0))
        });
        sorted.into_iter().take(250).collect()
    };
    let (cheat_logs, cheat_logs_count) = cheat_logs_recent(store, 250);
    let coin_logs = crate::coins::read_coin_log(data_dir, 250);

    json!({
        "generatedAt": now,
        "policy": {
            "adminActions": "Admin actions are logged so privileged changes can be reviewed later.",
            "chatReports": "Staff should use explicit user reports for moderation context.",
            "consent": "This monitoring should match the site terms and be limited to authorized admins with a security or moderation need.",
        },
        "stats": {
            "adminActions": admin_actions.len(),
            "reports": chat_reports_all.len(),
            "profileReports": profile_reports.len(),
            "sentNotifications": sent_notifications.len(),
            "bannedAccounts": banned_accounts.len(),
                "cheatLogs": cheat_logs_count,
            "coinLogs": coin_logs.len(),
        },
        "activeUsers": active_users,
        "bannedAccounts": banned_accounts,
        "adminActions": admin_actions,
        "reports": chat_reports_top,
        "canvasReports": [],
        "chatReports": chat_reports_top,
        "profileReports": profile_reports,
        "sentNotifications": sent_notifications.iter().take(250).cloned().collect::<Vec<_>>(),
        "heatmap": store.read_document(&data_dir.join("heatmap.json"), json!({})),
        "cheatLogs": cheat_logs,
        "coinLogs": coin_logs,
    })
}

/// `blogContributorEmails()` — accepts an array or an `{email: active}` map.
pub fn blog_contributor_emails(store: &DataStore, data_dir: &Path) -> Vec<String> {
    let raw = store.read_document(&data_dir.join("blog_contributors.json"), json!([]));
    match &raw {
        Value::Array(a) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .filter(|s| !s.is_empty())
            .collect(),
        Value::Object(o) => o
            .iter()
            .filter(|(_, active)| active.as_bool() != Some(false))
            .map(|(email, _)| email.clone())
            .collect(),
        _ => Vec::new(),
    }
}

/// `isBlogContributorEmail(email)` — used by /api/me.
pub fn is_blog_contributor_email(store: &DataStore, data_dir: &Path, email: &str) -> bool {
    if email.is_empty() {
        return false;
    }
    let norm = normalize_email(email);
    blog_contributor_emails(store, data_dir)
        .iter()
        .any(|c| normalize_email(c) == norm)
}

/// `moderatorPanelConfig()` + `sanitizeModeratorPanelLinks`.
pub fn sanitize_moderator_panel_links(links: &Value) -> Vec<Value> {
    let Some(arr) = links.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .map(|link| {
            json!({
                "label": link.get("label").and_then(|v| v.as_str()).unwrap_or("").trim().chars().take(60).collect::<String>(),
                "href": link.get("href").and_then(|v| v.as_str()).unwrap_or("").trim().chars().take(180).collect::<String>(),
            })
        })
        .filter(|link| {
            let label = link.get("label").and_then(|v| v.as_str()).unwrap_or("");
            let href = link.get("href").and_then(|v| v.as_str()).unwrap_or("");
            !label.is_empty()
                && href.starts_with('/')
                && !href.starts_with("//")
                && !href.contains('\\')
        })
        .take(20)
        .collect()
}

pub fn moderator_panel_config(store: &DataStore, data_dir: &Path) -> Value {
    let raw = store.read_document(
        &data_dir.join("moderator_panel.json"),
        json!({ "links": [] }),
    );
    let links = raw.get("links").cloned().unwrap_or(json!([]));
    json!({ "links": sanitize_moderator_panel_links(&links) })
}

// ── Moderator action requests ────────────────────────────────────────────────

/// `MODERATOR_ACTION_LABELS` — verbatim.
pub const MODERATOR_ACTION_LABELS: &[(&str, &str)] = &[
    ("grant_premium", "Grant premium"),
    ("revoke_premium", "Revoke premium"),
    ("send_notification", "Send notification"),
    ("unsend_notification", "Unsend notification"),
    ("gift_coins", "Give coins"),
    ("burn_coins", "Burn coins"),
    ("economy_multiplier", "Update coin multiplier"),
    ("broadcast", "Global broadcast"),
    ("shadow_ban", "Toggle shadow-ban"),
    ("ban_account", "Ban account"),
    ("unban_account", "Unban account"),
    ("prox_block", "Block mitch.prox domain"),
    ("content_mirror", "Update mirror link"),
    ("content_featured", "Set featured game"),
    ("moderator_role", "Update moderator role"),
    ("moderator_panel", "Update moderator panel"),
    ("admin_role", "Update administrator role"),
];

pub fn moderator_action_label(action: &str) -> Option<&'static str> {
    MODERATOR_ACTION_LABELS
        .iter()
        .find(|(k, _)| *k == action)
        .map(|(_, v)| *v)
}

/// `MODERATOR_ACTION_BY_URL` — verbatim.
pub fn moderator_action_by_url(url: &str) -> &'static str {
    match url {
        "/api/admin/grant-premium" => "grant_premium",
        "/api/admin/revoke-premium" => "revoke_premium",
        "/api/admin/send-notification" => "send_notification",
        "/api/admin/unsend-notification" => "unsend_notification",
        "/api/admin/gift-coins" => "gift_coins",
        "/api/admin/economy/burn" => "burn_coins",
        "/api/admin/economy/multiplier" => "economy_multiplier",
        "/api/admin/broadcast" => "broadcast",
        "/api/admin/shadow-ban" => "shadow_ban",
        "/api/admin/restricted-mode" => "shadow_ban",
        "/api/admin/ban-account" => "ban_account",
        "/api/admin/unban-account" => "unban_account",
        "/api/admin/prox/block" => "prox_block",
        "/api/admin/content/mirror" => "content_mirror",
        "/api/admin/content/featured" => "content_featured",
        "/api/admin/moderators" => "moderator_role",
        "/api/admin/moderator-panel" => "moderator_panel",
        "/api/admin/admins" => "admin_role",
        _ => "",
    }
}

/// `moderatorActionFromBody(body)` — explicit action or targetUrl fallback.
pub fn moderator_action_from_body(body: &Value) -> String {
    let explicit = body
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if !explicit.is_empty() && moderator_action_label(explicit).is_some() {
        return explicit.to_string();
    }
    let target_url = body
        .get("targetUrl")
        .or_else(|| body.get("url"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    moderator_action_by_url(target_url).to_string()
}

/// Error carrying an HTTP status (mirrors `adminActionError`).
#[derive(Debug, Clone)]
pub struct AdminActionError {
    pub status: u16,
    pub message: String,
}

impl AdminActionError {
    pub fn new(status: u16, message: &str) -> Self {
        Self {
            status,
            message: message.to_string(),
        }
    }
}

fn canonical_email(raw: Option<&Value>, label: &str) -> Result<String, AdminActionError> {
    let email = raw
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase()
        .trim()
        .to_string();
    let ok = {
        let mut parts = email.split('@');
        let local = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("");
        !local.is_empty()
            && !rest.is_empty()
            && rest.contains('.')
            && !email.contains(|c: char| c.is_whitespace())
            && parts.next().is_none()
    };
    if !ok {
        return Err(AdminActionError::new(
            400,
            &format!("valid {label} required"),
        ));
    }
    Ok(email)
}

fn trim_slice(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// `cleanModeratorActionPayload(action, payload)` — canonicalizes a payload,
/// rejecting invalid input with a 400.
pub fn clean_moderator_action_payload(
    action: &str,
    payload: &Value,
) -> Result<Value, AdminActionError> {
    if moderator_action_label(action).is_none() {
        return Err(AdminActionError::new(400, "unknown moderator action"));
    }
    let p = if payload.is_object() {
        payload
    } else {
        &json!({})
    };
    let s = |k: &str| p.get(k).and_then(|v| v.as_str()).unwrap_or("");
    match action {
        "grant_premium" => Ok(json!({
            "targetEmail": canonical_email(p.get("targetEmail"), "target email")?,
            "reason": trim_slice(s("reason").trim(), 200),
        })),
        "revoke_premium" => Ok(json!({
            "email": canonical_email(
                p.get("email").or_else(|| p.get("targetEmail")),
                "email"
            )?,
            "reason": trim_slice(s("reason").trim(), 200),
        })),
        "send_notification" => {
            let all_users = p.get("allUsers").and_then(|v| v.as_bool()) == Some(true);
            Ok(json!({
                "allUsers": all_users,
                "targetEmail": if all_users { json!("") } else { json!(canonical_email(p.get("targetEmail"), "target email")?) },
                "title": trim_slice(s("title").trim(), 80),
                "message": trim_slice(s("message").trim(), 1000),
            }))
        }
        "unsend_notification" => Ok(json!({
            "id": trim_slice(s("id").trim(), 80),
            "batchId": trim_slice(s("batchId").trim(), 80),
        })),
        "gift_coins" => Ok(json!({
            "targetEmail": canonical_email(p.get("targetEmail"), "target email")?,
            "amount": p.get("amount").and_then(|v| v.as_f64()).unwrap_or(f64::NAN),
            "reason": trim_slice(s("reason").trim(), 160),
        })),
        "burn_coins" => Ok(json!({
            "email": canonical_email(p.get("email"), "email")?,
            "amount": p.get("amount").and_then(|v| v.as_f64()).unwrap_or(f64::NAN),
        })),
        "economy_multiplier" => Ok(json!({
            "multiplier": p.get("multiplier").and_then(|v| v.as_f64()).unwrap_or(f64::NAN),
        })),
        "broadcast" => Ok(json!({
            "msg": trim_slice(s("msg").trim(), 500),
            "type": trim_slice(s("type").trim(), 30),
        })),
        "shadow_ban" => Ok(json!({ "email": canonical_email(p.get("email"), "email")? })),
        "ban_account" => {
            crate::bans::duration_millis(p.get("duration"))
                .map_err(|error| AdminActionError::new(400, error))?;
            let reason = crate::bans::required_reason(s("reason"))
                .map_err(|error| AdminActionError::new(400, error))?;
            Ok(json!({
                "email": canonical_email(p.get("email"), "email")?,
                "reason": reason,
                "duration": p.get("duration").and_then(Value::as_str).unwrap_or("permanent"),
            }))
        }
        "unban_account" => Ok(json!({ "email": canonical_email(p.get("email"), "email")? })),
        "prox_block" => {
            let domain_raw = s("domain").to_lowercase().trim().to_string();
            let no_scheme = domain_raw
                .strip_prefix("http://")
                .or_else(|| domain_raw.strip_prefix("https://"))
                .unwrap_or(&domain_raw);
            let domain: String = no_scheme
                .split(['/', '?', '#'])
                .next()
                .unwrap_or("")
                .chars()
                .take(180)
                .collect();
            Ok(json!({ "domain": domain }))
        }
        "content_mirror" => Ok(json!({ "url": trim_slice(s("url").trim(), 500) })),
        "content_featured" => Ok(json!({ "href": trim_slice(s("href").trim(), 240) })),
        "moderator_role" => Ok(json!({
            "email": canonical_email(p.get("email"), "email")?,
            "active": p.get("active").and_then(|v| v.as_bool()) == Some(true),
        })),
        "moderator_panel" => Ok(
            json!({ "links": sanitize_moderator_panel_links(p.get("links").unwrap_or(&json!([]))) }),
        ),
        _ => Err(AdminActionError::new(400, "unknown moderator action")),
    }
}

/// `loadModeratorRequests()` / `saveModeratorRequests()`.
pub fn load_moderator_requests(store: &DataStore, data_dir: &Path) -> Vec<Value> {
    store
        .read_document(&data_dir.join("moderator_requests.json"), json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default()
}

pub fn save_moderator_requests(store: &DataStore, data_dir: &Path, requests: &[Value]) {
    let capped: Vec<Value> = requests.iter().take(1000).cloned().collect();
    let _ = store.write_document(&data_dir.join("moderator_requests.json"), &json!(capped));
}

/// `publicModeratorRequest(req)`.
pub fn public_moderator_request(req: &Value) -> Value {
    let action = req.get("action").and_then(|v| v.as_str()).unwrap_or("");
    json!({
        "id": req.get("id").and_then(|v| v.as_str()).unwrap_or(""),
        "requestedBy": req.get("requestedBy").and_then(|v| v.as_str()).unwrap_or(""),
        "requestedByEmail": req.get("requestedByEmail").and_then(|v| v.as_str()).unwrap_or(""),
        "action": action,
        "label": req.get("label").and_then(|v| v.as_str())
            .unwrap_or(moderator_action_label(action).unwrap_or("Moderator action")),
        "payload": req.get("payload").cloned().unwrap_or(json!({})),
        "status": req.get("status").and_then(|v| v.as_str()).unwrap_or("pending"),
        "requestedAt": req.get("requestedAt").and_then(|v| v.as_i64()).unwrap_or(0),
        "resolvedAt": req.get("resolvedAt").and_then(|v| v.as_i64()).unwrap_or(0),
        "resolvedBy": req.get("resolvedBy").and_then(|v| v.as_str()).unwrap_or(""),
        "note": req.get("note").and_then(|v| v.as_str()).unwrap_or(""),
        "result": req.get("result").cloned().unwrap_or(Value::Null),
        "error": req.get("error").and_then(|v| v.as_str()).unwrap_or(""),
    })
}

/// `createModeratorActionRequest(sid, body)` — validates the action + payload,
/// unshifts a pending entry, logs it.
pub fn create_moderator_action_request(
    store: &DataStore,
    id_secret: &[u8],
    data_dir: &Path,
    sid: &str,
    body: &Value,
) -> Result<Value, AdminActionError> {
    let requested_by_email = crate::auth::email_from_sid(store, id_secret, sid)
        .unwrap_or_else(|| "moderator".to_string());
    let action = moderator_action_from_body(body);
    if action.is_empty() {
        return Err(AdminActionError::new(400, "unknown moderator action"));
    }
    let empty = json!({});
    let payload_in = body.get("payload").unwrap_or(&empty);
    let payload = clean_moderator_action_payload(&action, payload_in)?;
    let label = body
        .get("label")
        .and_then(|v| v.as_str())
        .map(|l| trim_slice(l.trim(), 100))
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| {
            moderator_action_label(&action)
                .unwrap_or("Moderator action")
                .to_string()
        });
    let entry = json!({
        "id": crate::crypto::random_bytes_hex(10),
        "requestedBy": public_session_id(sid),
        "requestedByEmail": requested_by_email,
        "action": action,
        "label": label,
        "payload": payload,
        "status": "pending",
        "requestedAt": now_millis(),
        "resolvedAt": 0,
        "resolvedBy": "",
        "note": "",
        "result": null,
        "error": "",
    });
    let mut requests = load_moderator_requests(store, data_dir);
    requests.insert(0, entry.clone());
    save_moderator_requests(store, data_dir, &requests);
    log_admin_action(
        store,
        data_dir,
        &requested_by_email,
        "moderator_request",
        json!({
            "id": entry.get("id").cloned().unwrap_or_default(),
            "action": action,
            "label": label,
        }),
    );
    Ok(public_moderator_request(&entry))
}

// ── Premium granting ─────────────────────────────────────────────────────────

/// `can_grant_premium_email(email)` — checks whether an email is an authorized owner, co-owner, or admin.
pub fn can_grant_premium_email(store: &DataStore, email: &str) -> bool {
    if email.is_empty() || !crate::auth::is_admin_email(store, email) {
        return false;
    }
    let norm = normalize_email(email);
    crate::auth::is_owner_email(store, &norm)
        || crate::auth::is_co_owner_email(store, &norm)
        || crate::auth::admin_member_emails(store)
            .iter()
            .any(|a| normalize_email(a) == norm)
}

/// `canGrantPremiumId(sid)`.
pub fn can_grant_premium_id(store: &DataStore, id_secret: &[u8], sid: &str) -> bool {
    if sid.is_empty() || !crate::auth::is_admin_id(store, id_secret, sid, false) {
        return false;
    }
    let email = crate::auth::email_from_sid(store, id_secret, sid).unwrap_or_default();
    can_grant_premium_email(store, &email)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_store(tag: &str) -> (std::path::PathBuf, std::path::PathBuf, DataStore) {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-admin-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        let store = DataStore::open(&base, &base.join("data")).unwrap();
        // Helpers take the DATA dir; tests return both for convenience.
        (base.clone(), base.join("data"), store)
    }

    #[test]
    fn admin_action_search_matches_actor_details_action_and_time() {
        let (base, data, store) = temp_store("search");
        log_admin_action(
            &store,
            &data,
            "mod@x.test",
            "ban_account",
            json!({ "target": "kid@x.test" }),
        );
        log_admin_action(
            &store,
            &data,
            "boss@x.test",
            "gift_coins",
            json!({ "targetEmail": "kid@x.test", "amount": 5 }),
        );
        log_admin_action(
            &store,
            &data,
            "boss@x.test",
            "add_tester",
            json!({ "target": "100%_odd" }),
        );
        let run = |person: &str, action: &str| {
            search_admin_actions(
                &store,
                &ActionQuery {
                    person,
                    action,
                    ..Default::default()
                },
                10,
            )
        };
        assert_eq!(run("", "").1, 3);
        // The actor and the affected account are both found by "person".
        assert_eq!(run("kid@x.test", "").1, 2);
        assert_eq!(run("boss", "").1, 2);
        assert_eq!(run("kid", "gift").0[0]["action"], json!("gift_coins"));
        // LIKE wildcards in the search text are literal.
        assert_eq!(run("100%_", "").1, 1);
        assert_eq!(run("%", "").1, 1);
        let future = ActionQuery {
            from: Some(now_millis() + 86_400_000),
            ..Default::default()
        };
        assert_eq!(search_admin_actions(&store, &future, 10).1, 0);
        assert_eq!(
            admin_action_names(&store),
            ["add_tester", "ban_account", "gift_coins"]
        );
        assert_eq!(
            search_admin_actions(&store, &ActionQuery::default(), 1)
                .0
                .len(),
            1
        );
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn admin_actions_are_kept_in_a_table_and_migrated_from_json() {
        let (base, data, store) = temp_store("log");
        store
            .write_document(
                &data.join("admin_actions.json"),
                &json!([
                    { "id": "old1", "ts": 10, "actor": "a@x", "action": "matrix_ban_user",
                      "details": { "target": "t", "ip": "198.51.100.3", "userAgent": "UA" } },
                    { "id": "old2", "ts": 5, "actor": "a@x", "action": "ban_account", "details": {} }
                ]),
            )
            .unwrap();
        assert_eq!(migrate_admin_actions_from_json(&store), 2);
        assert_eq!(migrate_admin_actions_from_json(&store), 0);
        assert!(store
            .read_document(&data.join("admin_actions.json"), Value::Null)
            .is_null());
        assert_eq!(
            admin_action_by_id(&store, "old1").unwrap()["details"],
            json!({ "target": "t" })
        );

        // No cap: every action is kept.
        for i in 0..1005 {
            log_admin_action(
                &store,
                &data,
                "admin@mitch.pro",
                "test_action",
                json!({ "i": i }),
            );
        }
        let recent = admin_actions_recent(&store, 5000);
        assert_eq!(recent.len(), 1007);
        assert_eq!(recent[0]["actor"], "admin@mitch.pro");
        let matrix = admin_actions_with_prefix(&store, "matrix_", 10);
        assert_eq!(matrix.len(), 1);
        assert_eq!(matrix[0]["id"], "old1");
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn mask_email_matches_js() {
        // maskEmail is an identity function in the JS (server.js:1352).
        assert_eq!(
            mask_email("mitchfogler@student.rjuhsd.us"),
            "mitchfogler@student.rjuhsd.us"
        );
        assert_eq!(mask_email("ab@c.com"), "ab@c.com");
        assert_eq!(mask_email(""), "anonymous");
    }

    #[test]
    fn clean_payload_validates() {
        let ok = clean_moderator_action_payload(
            "grant_premium",
            &json!({ "targetEmail": "user@mitch.pro", "reason": "because" }),
        )
        .unwrap();
        assert_eq!(
            ok.get("targetEmail").and_then(|v| v.as_str()),
            Some("user@mitch.pro")
        );
        let bad = clean_moderator_action_payload(
            "gift_coins",
            &json!({ "targetEmail": "not-an-email", "amount": 5 }),
        );
        assert_eq!(bad.unwrap_err().status, 400);
        let unknown = clean_moderator_action_payload("nonsense", &json!({}));
        assert_eq!(unknown.unwrap_err().message, "unknown moderator action");
    }

    #[test]
    fn moderator_panel_links_sanitize() {
        let links = json!([
            { "label": "Wiki", "href": "/wiki/" },
            { "label": "", "href": "/x" },
            { "label": "Evil", "href": "//evil.com" },
            { "label": "Back", "href": "\\evil" },
            { "label": "Abs", "href": "https://x.com" },
        ]);
        let out = sanitize_moderator_panel_links(&links);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].get("label").and_then(|v| v.as_str()), Some("Wiki"));
    }

    #[test]
    fn advanced_data_shapes() {
        let (base, data, store) = temp_store("advanced");
        log_admin_action(
            &store,
            &data,
            "admin@mitch.pro",
            "view_admin_tools",
            json!({}),
        );
        crate::coins::add_admin_notification(
            &store,
            &data,
            "user@mitch.pro",
            "Hi",
            "m",
            "admin@mitch.pro",
            "b1",
            "",
        );
        crate::coins::add_coins(&store, &data, "user@mitch.pro", 10.0, 1.0, "daily login");
        let advanced = build_advanced_admin_data(&store, &data, 0);
        assert_eq!(
            advanced
                .get("stats")
                .and_then(|s| s.get("adminActions"))
                .and_then(|v| v.as_i64()),
            Some(1)
        );
        assert_eq!(
            advanced
                .get("sentNotifications")
                .and_then(|v| v.as_array())
                .map(|a| a.len()),
            Some(1)
        );
        assert_eq!(
            advanced
                .get("stats")
                .and_then(|s| s.get("coinLogs"))
                .and_then(|v| v.as_i64()),
            Some(1)
        );
        assert_eq!(
            advanced
                .get("coinLogs")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|r| r.get("reason"))
                .and_then(|v| v.as_str()),
            Some("daily login")
        );
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn sensitive_admin_actions_are_mutations_only() {
        assert!(is_sensitive_admin_action("POST", "/api/admin/ban-account"));
        assert!(is_sensitive_admin_action("POST", "/api/admin/vms/delete"));
        assert!(!is_sensitive_admin_action("GET", "/api/admin/bans"));
        assert!(!is_sensitive_admin_action(
            "POST",
            "/api/admin/special-schedule"
        ));
    }

    #[test]
    fn unlock_mac_is_bound_to_session_hash_and_expiry() {
        let key = b"secret";
        let base = admin_unlock_mac(key, "sid1", "hash-a", 1000);
        assert_eq!(base, admin_unlock_mac(key, "sid1", "hash-a", 1000));
        assert_ne!(base, admin_unlock_mac(key, "sid2", "hash-a", 1000));
        assert_ne!(base, admin_unlock_mac(key, "sid1", "hash-b", 1000));
        assert_ne!(base, admin_unlock_mac(key, "sid1", "hash-a", 1001));
    }
}
