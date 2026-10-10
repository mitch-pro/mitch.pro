//! The coins economy data layer — port of server.js's loadCoins/saveCoins/
//! getCoins/addCoins (2976-3005), addCoinGiftNotice (3083-3100), and
//! addAdminNotification (3102-3123).
//!
//! Contract:
//! - Balances live in the real `coins` table (email PK, balance,
//!   lifetime_earned), not a JSON blob — every earn/spend used to read the
//!   *entire site's* coins.json, mutate one entry, and write the whole
//!   blob back, which both serializes every concurrent transaction
//!   site-wide through one row and is vulnerable to a lost update if two
//!   writes raced (both read the same "before", second write clobbers the
//!   first). `coins.json` is migrated once into this table on startup
//!   (`DataStore::backfill_coins_table_from_json`) and no longer written.
//! - addCoins: positive amounts are multiplied by the global coin multiplier
//!   (raised to at least 2.0 during a personal happy hour), negative amounts
//!   pass through unmultiplied. Balances round to 4 decimals (JS toFixed).
//!   The read-modify-write of one row still happens in Rust (to keep the
//!   exact rounding/multiplier logic), but now inside a BEGIN IMMEDIATE
//!   transaction scoped to that one row instead of a whole-document write,
//!   the same pattern `reserve_virtual_machine` already uses for atomic
//!   single-row updates.
//! - Positive amounts also bump `lifetime_earned`, now a column on the
//!   same row instead of a separate user_stats.json write.
//! - Every change appends a TSV line to logs/coins.log.
//! - coin_gifts.json maps normalized email → array of notices (capped 50,
//!   newest first via unshift). Unrelated to the balance itself — still a
//!   JSON blob, not migrated here.

use crate::auth::normalize_email;
use crate::data::DataStore;
use serde_json::{json, Value};
use std::path::Path;

/// `loadCoins()` — every known balance as `{email: balance}`, matching the
/// old coins.json shape so existing callers (the leaderboard, the admin
/// economy audit) need no changes beyond this function's internals.
pub fn load_coins(store: &DataStore, _data_dir: &Path) -> Value {
    let conn = store.conn();
    let mut stmt = match conn.prepare("SELECT email, balance FROM coins") {
        Ok(s) => s,
        Err(_) => return json!({}),
    };
    let rows = stmt.query_map([], |row| {
        let email: String = row.get(0)?;
        let balance: f64 = row.get(1)?;
        Ok((email, balance))
    });
    let Ok(rows) = rows else { return json!({}) };
    let mut map = serde_json::Map::new();
    for row in rows.filter_map(|r| r.ok()) {
        map.insert(row.0, json!(row.1));
    }
    Value::Object(map)
}

/// `getCoins(email)` — 0 for empty email or unknown user.
pub fn get_coins(store: &DataStore, _data_dir: &Path, email: &str) -> f64 {
    if email.is_empty() {
        return 0.0;
    }
    let norm = normalize_email(email);
    let conn = store.conn();
    conn.query_row(
        "SELECT balance FROM coins WHERE email = ?1",
        rusqlite::params![norm],
        |row| row.get::<_, f64>(0),
    )
    .unwrap_or(0.0)
}

/// `lifetimeEarned(email)` — companion to `get_coins`, 0 for unknown user.
pub fn get_lifetime_earned(store: &DataStore, email: &str) -> f64 {
    if email.is_empty() {
        return 0.0;
    }
    let norm = normalize_email(email);
    let conn = store.conn();
    conn.query_row(
        "SELECT lifetime_earned FROM coins WHERE email = ?1",
        rusqlite::params![norm],
        |row| row.get::<_, f64>(0),
    )
    .unwrap_or(0.0)
}

/// `globalCoinMultiplier` default (server.js:1159).
pub const DEFAULT_COIN_MULTIPLIER: f64 = 1.0;

/// `addCoins(email, amount, reason)` — server.js:2979-3005.
/// `multiplier` is `globalCoinMultiplier` at call time; `personal_happy_hour`
/// is read from user_stats.json (positive amounts only, minimum 2.0x) —
/// that lookup stays a blob read (user_stats.json isn't migrated here),
/// only the balance/lifetime_earned persistence is now a real row.
pub fn add_coins(
    store: &DataStore,
    data_dir: &Path,
    email: &str,
    amount: f64,
    multiplier: f64,
    reason: &str,
) {
    if email.is_empty() {
        return;
    }
    let norm = normalize_email(email);
    let stats = store.read_document(&data_dir.join("user_stats.json"), json!({}));

    let personal_hh = stats
        .get(&norm)
        .and_then(|s| s.get("personal_happy_hour_until"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0)
        > now_millis() as f64;
    let mult = if personal_hh {
        multiplier.max(2.0)
    } else {
        multiplier
    };
    let adjusted = if amount > 0.0 { amount * mult } else { amount };

    // One row, one transaction: BEGIN IMMEDIATE takes the write lock
    // up front (same pattern reserve_virtual_machine uses), so a
    // concurrent add_coins for the same email can't read the same
    // "before" this one is about to overwrite — the lost-update race
    // the old whole-blob read/write had.
    let (before, after) = {
        let conn = store.conn();
        if conn.execute_batch("BEGIN IMMEDIATE").is_err() {
            return;
        }
        let before = conn
            .query_row(
                "SELECT balance FROM coins WHERE email = ?1",
                rusqlite::params![norm],
                |row| row.get::<_, f64>(0),
            )
            .unwrap_or(0.0);
        let lifetime_before = conn
            .query_row(
                "SELECT lifetime_earned FROM coins WHERE email = ?1",
                rusqlite::params![norm],
                |row| row.get::<_, f64>(0),
            )
            .unwrap_or(0.0);
        let after = js_round4(before + adjusted);
        let lifetime_after = if amount > 0.0 {
            lifetime_before + adjusted
        } else {
            lifetime_before
        };
        let _ = conn.execute(
            "INSERT INTO coins (email, balance, lifetime_earned, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(email) DO UPDATE SET balance = ?2, lifetime_earned = ?3, updated_at = ?4",
            rusqlite::params![norm, after, lifetime_after, now_millis()],
        );
        let _ = conn.execute_batch("COMMIT");
        (before, after)
    };

    // Append to coin log (JS parity: silent on failure).
    let ts = js_iso_date();
    let sign = if adjusted >= 0.0 { "+" } else { "" };
    let line = format!(
        "{ts}\t{norm}\t{sign}{:.4}\t{:.4} -> {:.4}\t{}\n",
        adjusted,
        before,
        after,
        if reason.is_empty() {
            "unspecified"
        } else {
            reason
        }
    );
    let logs_dir = data_dir.parent().unwrap_or(data_dir).join("logs");
    let _ = std::fs::create_dir_all(&logs_dir);
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(logs_dir.join("coins.log"))
        .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
}

/// Moves a balance row to a new email key — used when a user changes their
/// account email (server.js's `renameEmailReferences`, which used to treat
/// coins.json as just another entry in its generic key-rename list; a real
/// table needs its own UPDATE instead of a blob key move).
pub fn rename_coins_email(store: &DataStore, old_norm: &str, new_norm: &str) {
    if old_norm.is_empty() || new_norm.is_empty() || old_norm == new_norm {
        return;
    }
    let conn = store.conn();
    // DELETE any existing row at the destination first — ON CONFLICT would
    // otherwise fail the rename outright if new_norm somehow already has a
    // (presumably stale/zero) row.
    let _ = conn.execute("DELETE FROM coins WHERE email = ?1", rusqlite::params![new_norm]);
    let _ = conn.execute(
        "UPDATE coins SET email = ?1 WHERE email = ?2",
        rusqlite::params![new_norm, old_norm],
    );
}

/// `addCoinGiftNotice(targetEmail, amount, adminEmail, reason)` — 3083-3100.
pub fn add_coin_gift_notice(
    store: &DataStore,
    data_dir: &Path,
    target_email: &str,
    amount: f64,
    admin_email: &str,
    reason: &str,
) -> Option<Value> {
    let norm = normalize_email(target_email);
    if norm.is_empty() {
        return None;
    }
    let file = data_dir.join("coin_gifts.json");
    let mut gifts = store.read_document(&file, json!({}));
    let map = gifts.as_object_mut()?;
    let notices = map
        .entry(norm.clone())
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .cloned()
        .unwrap_or_default();
    let notice = json!({
        "id": crate::crypto::random_bytes_hex(12),
        "amount": amount,
        "from": if admin_email.is_empty() { "admin" } else { admin_email },
        "reason": if reason.is_empty() { "admin gift" } else { reason },
        "ts": now_millis(),
        "read": false,
    });
    let mut next = Vec::with_capacity(notices.len() + 1);
    next.push(notice.clone());
    next.extend(notices.into_iter().take(49));
    map.insert(norm, json!(next));
    let _ = store.write_document(&file, &gifts);
    Some(notice)
}

/// `addAdminNotification(targetEmail, title, message, adminEmail, batchId, url)`
/// — server.js:3102-3123. `kind: 'admin_notice'` rows in coin_gifts.json.
#[allow(clippy::too_many_arguments)] // mirrors the JS signature
pub fn add_admin_notification(
    store: &DataStore,
    data_dir: &Path,
    target_email: &str,
    title: &str,
    message: &str,
    admin_email: &str,
    batch_id: &str,
    url: &str,
) -> Option<Value> {
    let norm = normalize_email(target_email);
    if norm.is_empty() {
        return None;
    }
    let file = data_dir.join("coin_gifts.json");
    let mut gifts = store.read_document(&file, json!({}));
    let map = gifts.as_object_mut()?;
    let notices = map
        .entry(norm.clone())
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .cloned()
        .unwrap_or_default();
    let clean_path = if url.starts_with('/') {
        url.to_string()
    } else if url.is_empty() {
        "/".to_string()
    } else {
        format!("/{url}")
    };
    let notice = json!({
        "id": crate::crypto::random_bytes_hex(12),
        "kind": "admin_notice",
        "title": if title.is_empty() { "Admin notification" } else { title },
        "message": message,
        "from": if admin_email.is_empty() { "admin" } else { admin_email },
        "source": "mitchdog.com",
        "url": clean_path,
        "batchId": batch_id,
        "ts": now_millis(),
        "read": false,
    });
    let mut next = Vec::with_capacity(notices.len() + 1);
    next.push(notice.clone());
    next.extend(notices.into_iter().take(49));
    map.insert(norm, json!(next));
    let _ = store.write_document(&file, &gifts);
    Some(notice)
}

/// `addVmAdminNotification(targetEmail, title, message, adminEmail, url)`
/// — server.js:3309-3330. `kind: 'vm_admin_access'` rows in coin_gifts.json.
pub fn add_vm_admin_notification(
    store: &DataStore,
    data_dir: &Path,
    target_email: &str,
    title: &str,
    message: &str,
    admin_email: &str,
    url: &str,
) -> Option<Value> {
    let norm = normalize_email(target_email);
    if norm.is_empty() {
        return None;
    }
    let file = data_dir.join("coin_gifts.json");
    let mut gifts = store.read_document(&file, json!({}));
    let map = gifts.as_object_mut()?;
    let notices = map
        .entry(norm.clone())
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .cloned()
        .unwrap_or_default();
    let notice = json!({
        "id": crate::crypto::random_bytes_hex(12),
        "kind": "vm_admin_access",
        "title": if title.is_empty() { "Computer Access Alert" } else { title },
        "message": message,
        "from": if admin_email.is_empty() { "admin" } else { admin_email },
        "source": "mitchdog.com",
        "url": if url.is_empty() { "/vms/" } else { url },
        "batchId": "",
        "ts": now_millis(),
        "read": false,
    });
    let mut next = Vec::with_capacity(notices.len() + 1);
    next.push(notice.clone());
    next.extend(notices.into_iter().take(49));
    map.insert(norm, json!(next));
    let _ = store.write_document(&file, &gifts);
    Some(notice)
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `new Date().toISOString()` — `YYYY-MM-DDTHH:MM:SS.sssZ`, UTC, no time crate.
pub fn js_iso_date() -> String {
    js_iso_date_from(now_millis())
}

/// `new Date(ms).toISOString()` — the parameterized form used by endpoints
/// that stamp a request-time `now` into day keys.
pub fn js_iso_date_from(millis: i64) -> String {
    let days = millis.div_euclid(86_400_000);
    let secs = millis.rem_euclid(86_400_000) / 1000;
    let ms = millis.rem_euclid(1000);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// JS `(x).toFixed(4)` as a number — round-half-away-from-zero to 4 places.
fn js_round4(x: f64) -> f64 {
    let scaled = x * 10000.0;
    let rounded = if scaled >= 0.0 {
        (scaled + 0.5).floor()
    } else {
        (scaled - 0.5).ceil()
    };
    rounded / 10000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_store(tag: &str) -> (std::path::PathBuf, std::path::PathBuf, DataStore) {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-coins-test-{tag}-{}",
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
    fn add_coins_rounds_to_four_decimals() {
        let (base, data, store) = temp_store("round");
        add_coins(&store, &data, "A.B+x@mitch.pro", 10.0, 1.0, "test");
        let norm = normalize_email("a.b+x@mitch.pro");
        let coins = load_coins(&store, &data);
        assert_eq!(coins.get(&norm).and_then(|v| v.as_f64()), Some(10.0));
        // Multiplier applies to positive amounts.
        add_coins(&store, &data, "ab@student.rjuhsd.us", 3.0, 2.0, "");
        let coins = load_coins(&store, &data);
        assert_eq!(coins.get(&norm).and_then(|v| v.as_f64()), Some(16.0));
        // Negative amounts skip the multiplier.
        add_coins(&store, &data, "ab@student.rjuhsd.us", -5.0, 2.0, "burn");
        let coins = load_coins(&store, &data);
        assert_eq!(coins.get(&norm).and_then(|v| v.as_f64()), Some(11.0));
        assert_eq!(get_coins(&store, &data, "ab@student.rjuhsd.us"), 11.0);
        // lifetime_earned tracked on the same coins row for positive
        // amounts only (multiplier-adjusted: 10 + 3×2.0 — the -5 burn
        // doesn't count).
        assert_eq!(get_lifetime_earned(&store, "ab@student.rjuhsd.us"), 16.0);
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn add_coins_is_race_free_under_concurrent_writers() {
        // The whole point of moving off the whole-blob read/modify/write:
        // two "concurrent" adds to the same email must both land, not have
        // the second clobber the first's update with a stale "before".
        let (base, data, store) = temp_store("race");
        let store = std::sync::Arc::new(store);
        let data = std::sync::Arc::new(data);
        let mut handles = Vec::new();
        for _ in 0..20 {
            let store = store.clone();
            let data = data.clone();
            handles.push(std::thread::spawn(move || {
                add_coins(&store, &data, "racer@mitch.pro", 1.0, 1.0, "race");
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(get_coins(&store, &data, "racer@mitch.pro"), 20.0);
        std::fs::remove_dir_all(&*base).ok();
    }

    #[test]
    fn rename_coins_email_moves_the_row() {
        // Plain gmail.com addresses pass through normalize_email unchanged
        // (no dot/plus, non-mitch domain), unlike mitch.pro ones (which
        // remap to student.rjuhsd.us) — keeps this test's literals equal
        // to their own normalized form, matching what the real caller
        // (rename_email_references) always passes: already-normalized keys.
        let (base, data, store) = temp_store("rename");
        add_coins(&store, &data, "old@gmail.com", 50.0, 1.0, "test");
        rename_coins_email(&store, "old@gmail.com", "new@gmail.com");
        assert_eq!(get_coins(&store, &data, "old@gmail.com"), 0.0);
        assert_eq!(get_coins(&store, &data, "new@gmail.com"), 50.0);
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn admin_notices_land_in_coin_gifts() {
        let (base, data, store) = temp_store("notice");
        let n = add_admin_notification(
            &store,
            &data,
            "user@mitch.pro",
            "Hi",
            "msg",
            "admin@mitch.pro",
            "batch1",
            "",
        )
        .unwrap();
        assert_eq!(n.get("kind").and_then(|v| v.as_str()), Some("admin_notice"));
        assert_eq!(
            n.get("source").and_then(|v| v.as_str()),
            Some("mitchdog.com")
        );
        let gifts = store.read_document(&data.join("coin_gifts.json"), json!({}));
        let mine = gifts
            .get(normalize_email("user@mitch.pro").as_str())
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(mine.len(), 1);
        // Coin gift notices cap at 50, newest first.
        for i in 0..55 {
            add_coin_gift_notice(&store, &data, "user@mitch.pro", i as f64, "admin", "");
        }
        let gifts = store.read_document(&base.join("data/coin_gifts.json"), json!({}));
        let mine = gifts
            .get(normalize_email("user@mitch.pro").as_str())
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(mine.len(), 50);
        assert_eq!(mine[0].get("amount").and_then(|v| v.as_f64()), Some(54.0));
        std::fs::remove_dir_all(base).ok();
    }
}
