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
//! - addCoins: positive amounts are multiplied by the global coin multiplier;
//!   negative amounts
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
/// `multiplier` is `globalCoinMultiplier` at call time (positive amounts only).
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
    let adjusted = if amount > 0.0 {
        amount * multiplier
    } else {
        amount
    };

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

    log_coin_change(data_dir, &norm, before, after, reason);
}

/// What stopped a bet from settling.
#[derive(Debug, PartialEq, Eq)]
pub enum BetError {
    /// `bet` or `payout` was non-finite, zero, or negative.
    InvalidAmount,
    /// The account's balance (checked inside the same locked transaction
    /// as the debit, not beforehand) was less than `bet`.
    InsufficientFunds,
}

/// Atomically resolves one casino bet: debits `bet` and credits `payout`
/// (pass `0.0` for a loss) against the same row, inside one `BEGIN
/// IMMEDIATE` transaction — there is no window between "check the
/// balance" and "write the new balance" where a second concurrent bet for
/// the same email could read the same stale balance and also pass, which
/// is how a race turns two legitimate-looking bets into a free one. This
/// is the only function that should ever move money for a casino game;
/// every game's route handler computes its outcome from a server-side RNG
/// draw *before* calling this (never from client-supplied input) and then
/// calls this once per round.
///
/// Returns `Err(InsufficientFunds)` — leaving the balance untouched — if
/// the account can't cover `bet`. Never allows a balance to go negative.
pub fn settle_bet(
    store: &DataStore,
    data_dir: &Path,
    email: &str,
    bet: f64,
    payout: f64,
    reason: &str,
) -> Result<(f64, f64), BetError> {
    if email.is_empty() || !bet.is_finite() || bet <= 0.0 || !payout.is_finite() || payout < 0.0 {
        return Err(BetError::InvalidAmount);
    }
    let norm = normalize_email(email);
    let net = js_round4(payout - bet);

    let (before, after) = {
        let conn = store.conn();
        if conn.execute_batch("BEGIN IMMEDIATE").is_err() {
            return Err(BetError::InvalidAmount);
        }
        let before = conn
            .query_row(
                "SELECT balance FROM coins WHERE email = ?1",
                rusqlite::params![norm],
                |row| row.get::<_, f64>(0),
            )
            .unwrap_or(0.0);
        if before < bet {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(BetError::InsufficientFunds);
        }
        let lifetime_before = conn
            .query_row(
                "SELECT lifetime_earned FROM coins WHERE email = ?1",
                rusqlite::params![norm],
                |row| row.get::<_, f64>(0),
            )
            .unwrap_or(0.0);
        let after = js_round4(before + net);
        let lifetime_after = if net > 0.0 {
            lifetime_before + net
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

    log_coin_change(data_dir, &norm, before, after, reason);
    Ok((before, after))
}

/// Atomically deducts a bet upfront for multi-step casino games (Blackjack, Poker).
/// Leaves balance untouched and returns `Err(InsufficientFunds)` if before < bet.
pub fn escrow_bet(
    store: &DataStore,
    data_dir: &Path,
    email: &str,
    bet: f64,
    reason: &str,
) -> Result<f64, BetError> {
    if email.is_empty() || !bet.is_finite() || bet <= 0.0 {
        return Err(BetError::InvalidAmount);
    }
    let norm = normalize_email(email);

    let (before, after) = {
        let conn = store.conn();
        if conn.execute_batch("BEGIN IMMEDIATE").is_err() {
            return Err(BetError::InvalidAmount);
        }
        let before = conn
            .query_row(
                "SELECT balance FROM coins WHERE email = ?1",
                rusqlite::params![norm],
                |row| row.get::<_, f64>(0),
            )
            .unwrap_or(0.0);
        if before < bet {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(BetError::InsufficientFunds);
        }
        let after = js_round4(before - bet);
        let _ = conn.execute(
            "UPDATE coins SET balance = ?1, updated_at = ?2 WHERE email = ?3",
            rusqlite::params![after, now_millis(), norm],
        );
        let _ = conn.execute_batch("COMMIT");
        (before, after)
    };

    log_coin_change(data_dir, &norm, before, after, reason);
    Ok(after)
}

/// Settles an already-escrowed bet. `payout` is gross payout:
/// 0.0 on loss (bet was already deducted, balance unchanged),
/// bet on push (refund),
/// > bet on win.
pub fn settle_escrowed_bet(
    store: &DataStore,
    data_dir: &Path,
    email: &str,
    bet: f64,
    payout: f64,
    reason: &str,
) -> Result<f64, BetError> {
    if email.is_empty() || !bet.is_finite() || bet <= 0.0 || !payout.is_finite() || payout < 0.0 {
        return Err(BetError::InvalidAmount);
    }
    let norm = normalize_email(email);
    let net = js_round4(payout - bet);

    let (before, after) = {
        let conn = store.conn();
        if conn.execute_batch("BEGIN IMMEDIATE").is_err() {
            return Err(BetError::InvalidAmount);
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
        let after = js_round4(before + payout);
        let lifetime_after = if net > 0.0 {
            lifetime_before + net
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

    log_coin_change(data_dir, &norm, before, after, reason);
    Ok(after)
}

/// Sets a balance to an exact value — an admin correction, not an earn/spend
/// event: unlike `add_coins`, this never applies the coin multiplier and
/// never touches `lifetime_earned`. Returns the prior balance.
pub fn set_coins(
    store: &DataStore,
    data_dir: &Path,
    email: &str,
    new_balance: f64,
    reason: &str,
) -> f64 {
    if email.is_empty() {
        return 0.0;
    }
    let norm = normalize_email(email);
    let new_balance = js_round4(new_balance);

    let before = {
        let conn = store.conn();
        if conn.execute_batch("BEGIN IMMEDIATE").is_err() {
            return 0.0;
        }
        let before = conn
            .query_row(
                "SELECT balance FROM coins WHERE email = ?1",
                rusqlite::params![norm],
                |row| row.get::<_, f64>(0),
            )
            .unwrap_or(0.0);
        let _ = conn.execute(
            "INSERT INTO coins (email, balance, lifetime_earned, updated_at) VALUES (?1, ?2, 0, ?3)
             ON CONFLICT(email) DO UPDATE SET balance = ?2, updated_at = ?3",
            rusqlite::params![norm, new_balance, now_millis()],
        );
        let _ = conn.execute_batch("COMMIT");
        before
    };

    log_coin_change(data_dir, &norm, before, new_balance, reason);
    before
}

/// Sets every balance strictly above `threshold` to `new_balance`, once — a
/// point-in-time economy correction (e.g. clawing back an exploit's
/// winnings), not an ongoing cap: nothing stops an affected balance from
/// climbing back above `threshold` afterward. Returns the affected emails
/// with their prior balances.
pub fn cap_coins_above(
    store: &DataStore,
    data_dir: &Path,
    threshold: f64,
    new_balance: f64,
    reason: &str,
) -> Vec<(String, f64)> {
    let new_balance = js_round4(new_balance);
    let conn = store.conn();
    if conn.execute_batch("BEGIN IMMEDIATE").is_err() {
        return Vec::new();
    }
    let affected: Vec<(String, f64)> = {
        let mut stmt = match conn
            .prepare("SELECT email, balance FROM coins WHERE balance > ?1 ORDER BY balance DESC")
        {
            Ok(s) => s,
            Err(_) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Vec::new();
            }
        };
        let rows = stmt.query_map(rusqlite::params![threshold], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
        });
        match rows {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    };
    let now = now_millis();
    for (email, _) in &affected {
        let _ = conn.execute(
            "UPDATE coins SET balance = ?2, updated_at = ?3 WHERE email = ?1",
            rusqlite::params![email, new_balance, now],
        );
    }
    let _ = conn.execute_batch("COMMIT");
    drop(conn);

    for (email, before) in &affected {
        log_coin_change(data_dir, email, *before, new_balance, reason);
    }
    affected
}

/// Resets every coin balance in the `coins` table to zero and returns the
/// total number of coin rows affected.
pub fn reset_all_coins(store: &DataStore, data_dir: &Path, reason: &str) -> usize {
    let conn = store.conn();
    if conn.execute_batch("BEGIN IMMEDIATE").is_err() {
        return 0;
    }
    let rows: Vec<(String, f64)> = {
        let mut stmt = match conn.prepare("SELECT email, balance FROM coins") {
            Ok(s) => s,
            Err(_) => {
                let _ = conn.execute_batch("ROLLBACK");
                return 0;
            }
        };
        let mapped = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
        });
        match mapped {
            Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    };
    let count = rows.len();
    let _ = conn.execute(
        "UPDATE coins SET balance = 0, updated_at = ?1",
        rusqlite::params![now_millis()],
    );
    let _ = conn.execute_batch("COMMIT");
    drop(conn);
    for (email, before) in &rows {
        if *before != 0.0 {
            log_coin_change(data_dir, email, *before, 0.0, reason);
        }
    }
    count
}

/// Shared `logs/coins.log` TSV append, factored out of `add_coins`/
/// `set_coins`/`cap_coins_above` so every balance mutation leaves the same
/// audit trail.
pub(crate) fn log_coin_change(
    data_dir: &Path,
    norm_email: &str,
    before: f64,
    after: f64,
    reason: &str,
) {
    let ts = js_iso_date();
    let delta = after - before;
    let sign = if delta >= 0.0 { "+" } else { "" };
    let line = format!(
        "{ts}\t{norm_email}\t{sign}{:.4}\t{:.4} -> {:.4}\t{}\n",
        delta,
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

/// Reads back the `logs/coins.log` TSV trail `log_coin_change` appends to,
/// for the admin "Coin Log" view — every balance mutation (earn, spend,
/// admin burn/set/cap/reset) in one place. Returns up to `limit` entries,
/// newest first; a malformed or missing file just yields no rows rather
/// than an error, since this is a best-effort audit view, not a source of
/// truth (the `coins` table balance is that).
pub fn read_coin_log(data_dir: &Path, limit: usize) -> Vec<Value> {
    let logs_dir = data_dir.parent().unwrap_or(data_dir).join("logs");
    let Ok(content) = std::fs::read_to_string(logs_dir.join("coins.log")) else {
        return Vec::new();
    };
    let mut entries: Vec<Value> = content
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(5, '\t');
            let ts = parts.next()?;
            let email = parts.next()?;
            let delta: f64 = parts.next()?.parse().ok()?;
            let (before, after) = parts.next()?.split_once(" -> ")?;
            let before: f64 = before.trim().parse().ok()?;
            let after: f64 = after.trim().parse().ok()?;
            let reason = parts.next().unwrap_or("unspecified");
            Some(json!({
                "ts": ts,
                "email": email,
                "delta": delta,
                "before": before,
                "after": after,
                "reason": reason,
            }))
        })
        .collect();
    entries.reverse();
    entries.truncate(limit);
    entries
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
    let _ = conn.execute(
        "DELETE FROM coins WHERE email = ?1",
        rusqlite::params![new_norm],
    );
    let _ = conn.execute(
        "UPDATE coins SET email = ?1 WHERE email = ?2",
        rusqlite::params![new_norm, old_norm],
    );
}

/// Inserts one notice row for `norm_email` and prunes that user back down
/// to the newest 50 (the old blob's unshift-then-truncate-49 cap). `data`
/// is the notice's own fields (minus id/ts/read, set here) — including
/// `kind`/`batchId` when the caller wants them echoed back, since `data`
/// is what reads reconstruct verbatim.
fn insert_coin_gift_notice(
    store: &DataStore,
    norm_email: &str,
    kind: &str,
    batch_id: &str,
    mut data: Value,
) -> Value {
    let id = crate::crypto::random_bytes_hex(12);
    let ts = now_millis();
    if let Some(obj) = data.as_object_mut() {
        obj.insert("id".into(), json!(id));
        obj.insert("ts".into(), json!(ts));
        obj.insert("read".into(), json!(false));
    }
    let data_str = serde_json::to_string(&data).unwrap_or_else(|_| "{}".to_string());
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO coin_gift_notices (id, email, kind, batch_id, ts, read, data, created_at) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?5)",
        rusqlite::params![id, norm_email, kind, batch_id, ts, data_str],
    );
    let _ = conn.execute(
        "DELETE FROM coin_gift_notices WHERE email = ?1 AND id NOT IN (
            SELECT id FROM coin_gift_notices WHERE email = ?1 ORDER BY ts DESC, rowid DESC LIMIT 50
        )",
        rusqlite::params![norm_email],
    );
    data
}

/// Every notice for one user, newest first (was a coin_gifts.json[norm]
/// array read).
pub fn coin_gift_notices_for(store: &DataStore, norm_email: &str) -> Vec<Value> {
    let conn = store.conn();
    let mut stmt = match conn.prepare(
        "SELECT data, read FROM coin_gift_notices WHERE email = ?1 ORDER BY ts DESC, rowid DESC",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let result = match stmt.query_map(rusqlite::params![norm_email], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0))
    }) {
        Ok(rows) => rows
            .filter_map(|r| r.ok())
            .filter_map(|(data, read)| with_live_read(&data, read))
            .collect(),
        Err(_) => Vec::new(),
    };
    result
}

/// Parses `data` and overlays the real `read` column — `data` carries
/// whatever `read` was true at insert time, but `mark_coin_gift_notices_read`
/// only updates the column, so reads must overlay it to see mark-as-read
/// changes.
fn with_live_read(data: &str, read: bool) -> Option<Value> {
    let mut v: Value = serde_json::from_str(data).ok()?;
    if let Some(obj) = v.as_object_mut() {
        obj.insert("read".into(), json!(read));
    }
    Some(v)
}

/// Marks notices read for one user — specific `ids`, or every notice when
/// `ids` is empty (was the JS's "empty ids marks all" contract).
pub fn mark_coin_gift_notices_read(
    store: &DataStore,
    norm_email: &str,
    ids: &std::collections::HashSet<String>,
) {
    let conn = store.conn();
    if ids.is_empty() {
        let _ = conn.execute(
            "UPDATE coin_gift_notices SET read = 1 WHERE email = ?1",
            rusqlite::params![norm_email],
        );
    } else {
        for id in ids {
            let _ = conn.execute(
                "UPDATE coin_gift_notices SET read = 1 WHERE email = ?1 AND id = ?2",
                rusqlite::params![norm_email, id],
            );
        }
    }
}

/// Every `admin_notice` row across every user, as (email, notice) pairs —
/// was a full coin_gifts.json scan over every user's array.
pub fn admin_notice_rows(store: &DataStore) -> Vec<(String, Value)> {
    let conn = store.conn();
    let mut stmt = match conn.prepare(
        "SELECT email, data, read FROM coin_gift_notices WHERE kind = 'admin_notice' ORDER BY ts DESC, rowid DESC",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let result = match stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)? != 0,
        ))
    }) {
        Ok(rows) => rows
            .filter_map(|r| r.ok())
            .filter_map(|(email, data, read)| with_live_read(&data, read).map(|v| (email, v)))
            .collect(),
        Err(_) => Vec::new(),
    };
    result
}

/// Deletes every `admin_notice` row matching `id` or `batch_id` (either
/// may be empty to skip that condition) across every user. Returns how
/// many rows were removed — was a full-site scan rewriting every user's
/// blob to drop matching entries.
pub fn unsend_coin_gift_notices(store: &DataStore, id: &str, batch_id: &str) -> i64 {
    let conn = store.conn();
    let mut removed = 0i64;
    if !id.is_empty() {
        removed += conn
            .execute(
                "DELETE FROM coin_gift_notices WHERE kind = 'admin_notice' AND id = ?1",
                rusqlite::params![id],
            )
            .unwrap_or(0) as i64;
    }
    if !batch_id.is_empty() {
        removed += conn
            .execute(
                "DELETE FROM coin_gift_notices WHERE kind = 'admin_notice' AND batch_id = ?1",
                rusqlite::params![batch_id],
            )
            .unwrap_or(0) as i64;
    }
    removed
}

/// Moves every one of a user's notices onto their new normalized email.
pub fn rename_coin_gift_notices_email(store: &DataStore, old_norm: &str, new_norm: &str) {
    let conn = store.conn();
    let _ = conn.execute(
        "UPDATE coin_gift_notices SET email = ?2 WHERE email = ?1",
        rusqlite::params![old_norm, new_norm],
    );
}

/// `addCoinGiftNotice(targetEmail, amount, adminEmail, reason)` — 3083-3100.
pub fn add_coin_gift_notice(
    store: &DataStore,
    _data_dir: &Path,
    target_email: &str,
    amount: f64,
    admin_email: &str,
    reason: &str,
) -> Option<Value> {
    let norm = normalize_email(target_email);
    if norm.is_empty() {
        return None;
    }
    let data = json!({
        "amount": amount,
        "from": if admin_email.is_empty() { "admin" } else { admin_email },
        "reason": if reason.is_empty() { "admin gift" } else { reason },
    });
    Some(insert_coin_gift_notice(store, &norm, "", "", data))
}

/// `addAdminNotification(targetEmail, title, message, adminEmail, batchId, url)`
/// — server.js:3102-3123. `kind: 'admin_notice'` rows in coin_gifts.json.
#[allow(clippy::too_many_arguments)] // mirrors the JS signature
pub fn add_admin_notification(
    store: &DataStore,
    _data_dir: &Path,
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
    let clean_path = if url.starts_with('/') {
        url.to_string()
    } else if url.is_empty() {
        "/".to_string()
    } else {
        format!("/{url}")
    };
    let data = json!({
        "kind": "admin_notice",
        "title": if title.is_empty() { "Admin notification" } else { title },
        "message": message,
        "from": if admin_email.is_empty() { "admin" } else { admin_email },
        "source": "mitchdog.com",
        "url": clean_path,
        "batchId": batch_id,
    });
    Some(insert_coin_gift_notice(
        store,
        &norm,
        "admin_notice",
        batch_id,
        data,
    ))
}

/// `addVmAdminNotification(targetEmail, title, message, adminEmail, url)`
/// — server.js:3309-3330. `kind: 'vm_admin_access'` rows in coin_gifts.json.
pub fn add_vm_admin_notification(
    store: &DataStore,
    _data_dir: &Path,
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
    let data = json!({
        "kind": "vm_admin_access",
        "title": if title.is_empty() { "Computer Access Alert" } else { title },
        "message": message,
        "from": if admin_email.is_empty() { "admin" } else { admin_email },
        "source": "mitchdog.com",
        "url": if url.is_empty() { "/vms/" } else { url },
        "batchId": "",
    });
    Some(insert_coin_gift_notice(
        store,
        &norm,
        "vm_admin_access",
        "",
        data,
    ))
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
    fn retired_personal_boost_cannot_multiply_new_earnings() {
        let (base, data, store) = temp_store("retired-boost");
        crate::user_stats::user_stats_set(
            &store,
            "member@example.com",
            &json!({"personal_happy_hour_until": 9_999_999_999_999i64}),
        );
        add_coins(&store, &data, "member@example.com", 10.0, 1.0, "test");
        assert_eq!(get_coins(&store, &data, "member@example.com"), 10.0);
        std::fs::remove_dir_all(base).ok();
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
    fn settle_bet_rejects_a_bet_the_balance_cannot_cover() {
        let (base, data, store) = temp_store("bet-insufficient");
        add_coins(&store, &data, "poor@mitch.pro", 10.0, 1.0, "seed");
        let err =
            settle_bet(&store, &data, "poor@mitch.pro", 10.01, 0.0, "casino:test").unwrap_err();
        assert_eq!(err, BetError::InsufficientFunds);
        // Balance must be untouched by the rejected bet.
        assert_eq!(get_coins(&store, &data, "poor@mitch.pro"), 10.0);
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn settle_bet_rejects_non_finite_or_non_positive_amounts() {
        let (base, data, store) = temp_store("bet-invalid");
        add_coins(&store, &data, "p@mitch.pro", 10.0, 1.0, "seed");
        assert_eq!(
            settle_bet(&store, &data, "p@mitch.pro", 0.0, 0.0, "x").unwrap_err(),
            BetError::InvalidAmount
        );
        assert_eq!(
            settle_bet(&store, &data, "p@mitch.pro", -5.0, 0.0, "x").unwrap_err(),
            BetError::InvalidAmount
        );
        assert_eq!(
            settle_bet(&store, &data, "p@mitch.pro", f64::NAN, 0.0, "x").unwrap_err(),
            BetError::InvalidAmount
        );
        assert_eq!(
            settle_bet(&store, &data, "p@mitch.pro", 1.0, f64::INFINITY, "x").unwrap_err(),
            BetError::InvalidAmount
        );
        assert_eq!(
            settle_bet(&store, &data, "p@mitch.pro", 1.0, -1.0, "x").unwrap_err(),
            BetError::InvalidAmount
        );
        assert_eq!(get_coins(&store, &data, "p@mitch.pro"), 10.0);
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn settle_bet_debits_on_loss_and_credits_net_on_win() {
        let (base, data, store) = temp_store("bet-settle");
        add_coins(&store, &data, "player@mitch.pro", 100.0, 1.0, "seed");
        // Loss: payout 0, balance drops by the full bet.
        settle_bet(
            &store,
            &data,
            "player@mitch.pro",
            10.0,
            0.0,
            "casino:coinflip:loss",
        )
        .unwrap();
        assert_eq!(get_coins(&store, &data, "player@mitch.pro"), 90.0);
        // Win: payout 19.2 on a 10 bet nets +9.2.
        settle_bet(
            &store,
            &data,
            "player@mitch.pro",
            10.0,
            19.2,
            "casino:coinflip:win",
        )
        .unwrap();
        assert_eq!(get_coins(&store, &data, "player@mitch.pro"), 99.2);
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn settle_bet_never_goes_negative_under_concurrent_bets() {
        // 20 threads each bet the player's entire 10-coin balance on a
        // guaranteed loss at the same instant. Exactly one must succeed;
        // the other 19 must see InsufficientFunds and touch nothing — if
        // the balance check and the debit weren't in the same locked
        // transaction, several could read "10" before any of them wrote
        // back, and the balance would go negative.
        let (base, data, store) = temp_store("bet-race");
        add_coins(&store, &data, "racer@mitch.pro", 10.0, 1.0, "seed");
        let store = std::sync::Arc::new(store);
        let data = std::sync::Arc::new(data);
        let mut handles = Vec::new();
        for _ in 0..20 {
            let store = store.clone();
            let data = data.clone();
            handles.push(std::thread::spawn(move || {
                settle_bet(
                    &store,
                    &data,
                    "racer@mitch.pro",
                    10.0,
                    0.0,
                    "casino:test:loss",
                )
            }));
        }
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let ok_count = results.iter().filter(|r| r.is_ok()).count();
        assert_eq!(ok_count, 1);
        assert_eq!(get_coins(&store, &data, "racer@mitch.pro"), 0.0);
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
        let norm = normalize_email("user@mitch.pro");
        let mine = coin_gift_notices_for(&store, &norm);
        assert_eq!(mine.len(), 1);
        // Coin gift notices cap at 50, newest first.
        for i in 0..55 {
            add_coin_gift_notice(&store, &data, "user@mitch.pro", i as f64, "admin", "");
        }
        let mine = coin_gift_notices_for(&store, &norm);
        assert_eq!(mine.len(), 50);
        assert_eq!(mine[0].get("amount").and_then(|v| v.as_f64()), Some(54.0));
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn mark_read_specific_ids_and_all() {
        let (base, data, store) = temp_store("mark-read");
        let norm = normalize_email("user@mitch.pro");
        let a = add_coin_gift_notice(&store, &data, "user@mitch.pro", 1.0, "admin", "").unwrap();
        let b = add_coin_gift_notice(&store, &data, "user@mitch.pro", 2.0, "admin", "").unwrap();
        let a_id = a.get("id").and_then(|v| v.as_str()).unwrap().to_string();
        mark_coin_gift_notices_read(&store, &norm, &[a_id.clone()].into_iter().collect());
        let mine = coin_gift_notices_for(&store, &norm);
        let a_row = mine
            .iter()
            .find(|n| n.get("id").and_then(|v| v.as_str()) == Some(a_id.as_str()))
            .unwrap();
        let b_id = b.get("id").and_then(|v| v.as_str()).unwrap();
        let b_row = mine
            .iter()
            .find(|n| n.get("id").and_then(|v| v.as_str()) == Some(b_id))
            .unwrap();
        assert_eq!(a_row.get("read").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(b_row.get("read").and_then(|v| v.as_bool()), Some(false));
        // Empty ids marks every notice read.
        mark_coin_gift_notices_read(&store, &norm, &std::collections::HashSet::new());
        let mine = coin_gift_notices_for(&store, &norm);
        assert!(mine
            .iter()
            .all(|n| n.get("read").and_then(|v| v.as_bool()) == Some(true)));
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn unsend_removes_by_id_or_batch_across_users() {
        let (base, data, store) = temp_store("unsend");
        let n1 = add_admin_notification(
            &store,
            &data,
            "a@mitch.pro",
            "t",
            "m",
            "admin",
            "batchA",
            "",
        )
        .unwrap();
        add_admin_notification(
            &store,
            &data,
            "b@mitch.pro",
            "t",
            "m",
            "admin",
            "batchA",
            "",
        )
        .unwrap();
        add_admin_notification(
            &store,
            &data,
            "c@mitch.pro",
            "t",
            "m",
            "admin",
            "batchB",
            "",
        )
        .unwrap();
        // A coin gift (not an admin_notice) must never be touched by unsend.
        add_coin_gift_notice(&store, &data, "a@mitch.pro", 5.0, "admin", "");

        let removed = unsend_coin_gift_notices(&store, "", "batchA");
        assert_eq!(removed, 2);
        assert!(
            coin_gift_notices_for(&store, &normalize_email("a@mitch.pro"))
                .iter()
                .any(|n| n.get("amount").is_some())
        );
        assert!(coin_gift_notices_for(&store, &normalize_email("b@mitch.pro")).is_empty());
        assert_eq!(
            coin_gift_notices_for(&store, &normalize_email("c@mitch.pro")).len(),
            1
        );

        let n1_id = n1.get("id").and_then(|v| v.as_str()).unwrap();
        let removed = unsend_coin_gift_notices(&store, n1_id, "");
        assert_eq!(removed, 0, "already removed by the batch delete above");
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn rename_moves_every_notice() {
        let (base, data, store) = temp_store("notice-rename");
        add_coin_gift_notice(&store, &data, "old@gmail.com", 1.0, "admin", "");
        add_admin_notification(&store, &data, "old@gmail.com", "t", "m", "admin", "b1", "")
            .unwrap();
        rename_coin_gift_notices_email(&store, "old@gmail.com", "new@gmail.com");
        assert!(coin_gift_notices_for(&store, "old@gmail.com").is_empty());
        assert_eq!(coin_gift_notices_for(&store, "new@gmail.com").len(), 2);
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn set_coins_overwrites_balance_without_multiplier_or_lifetime() {
        let (base, data, store) = temp_store("set-coins");
        // Earn 10 with a 2x multiplier, tracked in lifetime_earned.
        add_coins(&store, &data, "p@gmail.com", 10.0, 2.0, "earn");
        assert_eq!(get_coins(&store, &data, "p@gmail.com"), 20.0);
        assert_eq!(get_lifetime_earned(&store, "p@gmail.com"), 20.0);

        // set_coins overwrites the balance exactly, ignoring the multiplier,
        // and leaves lifetime_earned untouched since it's a correction, not
        // an earn event.
        let previous = set_coins(&store, &data, "p@gmail.com", 10000.0, "admin set");
        assert_eq!(previous, 20.0);
        assert_eq!(get_coins(&store, &data, "p@gmail.com"), 10000.0);
        assert_eq!(get_lifetime_earned(&store, "p@gmail.com"), 20.0);

        // Works for a brand-new account too (no prior row).
        let previous = set_coins(&store, &data, "fresh@gmail.com", 500.0, "admin set");
        assert_eq!(previous, 0.0);
        assert_eq!(get_coins(&store, &data, "fresh@gmail.com"), 500.0);
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn cap_coins_above_only_touches_accounts_over_threshold() {
        let (base, data, store) = temp_store("cap-coins");
        set_coins(&store, &data, "whale@gmail.com", 250000.0, "seed");
        set_coins(&store, &data, "rich@gmail.com", 100001.0, "seed");
        set_coins(&store, &data, "exact@gmail.com", 100000.0, "seed");
        set_coins(&store, &data, "normal@gmail.com", 500.0, "seed");

        let affected = cap_coins_above(&store, &data, 100000.0, 10000.0, "exploit clawback");
        let affected_emails: std::collections::HashSet<_> =
            affected.iter().map(|(e, _)| e.clone()).collect();
        assert_eq!(affected.len(), 2);
        assert!(affected_emails.contains("whale@gmail.com"));
        assert!(affected_emails.contains("rich@gmail.com"));

        assert_eq!(get_coins(&store, &data, "whale@gmail.com"), 10000.0);
        assert_eq!(get_coins(&store, &data, "rich@gmail.com"), 10000.0);
        // Exactly-at-threshold and below are untouched.
        assert_eq!(get_coins(&store, &data, "exact@gmail.com"), 100000.0);
        assert_eq!(get_coins(&store, &data, "normal@gmail.com"), 500.0);
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn read_coin_log_parses_entries_newest_first_and_caps_at_limit() {
        let (base, data, store) = temp_store("read-log");
        add_coins(&store, &data, "earner@gmail.com", 10.0, 1.0, "daily login");
        add_coins(
            &store,
            &data,
            "earner@gmail.com",
            -4.0,
            1.0,
            "shop purchase",
        );
        set_coins(&store, &data, "whale@gmail.com", 500.0, "admin set");

        let log = read_coin_log(&data, 10);
        assert_eq!(log.len(), 3);
        // Newest entry (the set_coins admin correction) comes first.
        assert_eq!(log[0]["email"], json!("whale@gmail.com"));
        assert_eq!(log[0]["reason"], json!("admin set"));
        assert_eq!(log[0]["before"], json!(0.0));
        assert_eq!(log[0]["after"], json!(500.0));
        assert_eq!(log[0]["delta"], json!(500.0));
        // Oldest entry is last.
        assert_eq!(log[2]["email"], json!("earner@gmail.com"));
        assert_eq!(log[2]["reason"], json!("daily login"));
        assert_eq!(log[2]["delta"], json!(10.0));
        // The negative-amount entry's delta parses correctly (no leading
        // '+' for negative deltas, unlike positive ones).
        assert_eq!(log[1]["delta"], json!(-4.0));

        // limit caps to the most recent entries, not just truncates the
        // file in whatever order it was read.
        let capped = read_coin_log(&data, 1);
        assert_eq!(capped.len(), 1);
        assert_eq!(capped[0]["email"], json!("whale@gmail.com"));

        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn read_coin_log_is_empty_when_no_log_file_exists() {
        let (base, data, _store) = temp_store("read-log-missing");
        assert_eq!(read_coin_log(&data, 10), Vec::<Value>::new());
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn test_escrow_bet_and_settle_escrowed_bet() {
        let (base, data, store) = temp_store("escrow-bet");
        let email = "bettor@mitch.pro";
        set_coins(&store, &data, email, 100.0, "init");

        // Escrow 40 coins: balance drops to 60
        let after_escrow = escrow_bet(&store, &data, email, 40.0, "casino:blackjack:bet").unwrap();
        assert_eq!(after_escrow, 60.0);
        assert_eq!(get_coins(&store, &data, email), 60.0);

        // Trying to escrow 70 coins fails with InsufficientFunds
        let err = escrow_bet(&store, &data, email, 70.0, "casino:blackjack:bet");
        assert_eq!(err, Err(BetError::InsufficientFunds));
        assert_eq!(get_coins(&store, &data, email), 60.0);

        // Settle losing round (payout 0.0): balance remains 60.0 (bet was already deducted)
        let after_loss = settle_escrowed_bet(
            &store,
            &data,
            email,
            40.0,
            0.0,
            "casino:blackjack:dealer_wins",
        )
        .unwrap();
        assert_eq!(after_loss, 60.0);
        assert_eq!(get_coins(&store, &data, email), 60.0);

        // Escrow 50 coins: balance drops to 10
        let after_escrow2 = escrow_bet(&store, &data, email, 50.0, "casino:blackjack:bet").unwrap();
        assert_eq!(after_escrow2, 10.0);

        // Settle winning round (payout 100.0 on 50.0 bet): balance becomes 10 + 100 = 110.0
        let after_win = settle_escrowed_bet(
            &store,
            &data,
            email,
            50.0,
            100.0,
            "casino:blackjack:player_wins",
        )
        .unwrap();
        assert_eq!(after_win, 110.0);
        assert_eq!(get_coins(&store, &data, email), 110.0);

        std::fs::remove_dir_all(base).ok();
    }
}
