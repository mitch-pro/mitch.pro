//! Beta tester sandbox: per-tester overrides for coins, premium, cosmetics and
//! the daily-claim cooldown, backing `/tester/` (`/api/tester/*`).
//!
//! # Contract
//!
//! - A row in `tester_settings` is what makes an account a *sandbox tester*.
//!   Rows are created when an admin adds the account to the tester list or the
//!   tester first opens the hub, and deleted when they're removed from the list
//!   (`routes/admin/moderation.rs::set_tester`). The hot paths (`get_coins`,
//!   `is_premium_email`, ...) only do one indexed lookup on this table.
//! - Admins are testers for *access* (`auth::is_tester_email`) but get no
//!   sandbox unless they're explicitly on the tester list, so an admin's real
//!   balance is never replaced by a sandbox one.
//! - **Coins never touch the real economy.** A sandbox tester's balance lives
//!   here, not in `coins`: unlimited mode reports [`UNLIMITED_BALANCE`] and
//!   spends never drain it; custom mode holds an exact number that bets, spends
//!   and earnings adjust. Nothing a tester does mints real coins, shows in
//!   `coins.log`, or touches `lifetime_earned`. Routes that would move value to
//!   *another* account (marketplace, coin gifts) refuse sandbox testers via
//!   [`is_sandboxed`], because otherwise sandbox coins could pay a real seller.
//! - Testers are excluded from every leaderboard (`routes/misc.rs`).

use crate::auth::normalize_email;
use crate::data::DataStore;
use rusqlite::params;

/// What an "unlimited" balance reads as (the old server's constant).
pub const UNLIMITED_BALANCE: f64 = 999_999_999.0;

/// Largest custom balance a tester can set; keeps the number displayable.
pub const MAX_CUSTOM_COINS: f64 = 999_999_999.0;

#[derive(Debug, Clone, PartialEq)]
pub struct TesterSettings {
    pub premium: bool,
    pub unlimited_coins: bool,
    pub custom_coins: f64,
    pub unlock_all_cosmetics: bool,
    pub bypass_cooldowns: bool,
}

impl Default for TesterSettings {
    fn default() -> Self {
        Self {
            premium: true,
            unlimited_coins: true,
            custom_coins: 0.0,
            unlock_all_cosmetics: false,
            bypass_cooldowns: false,
        }
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn round4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

/// Whether `email` is on the explicit tester list (`data/testers.json`).
/// Unlike `auth::is_tester_email` this does not count admins.
pub fn is_listed(store: &DataStore, email: &str) -> bool {
    if email.is_empty() {
        return false;
    }
    let norm = normalize_email(email);
    let raw = store.read_document(
        &store.base_dir.join("data/testers.json"),
        serde_json::json!([]),
    );
    if let Some(arr) = raw.as_array() {
        return arr
            .iter()
            .any(|v| v.as_str().is_some_and(|s| normalize_email(s) == norm));
    }
    raw.as_object()
        .is_some_and(|obj| obj.keys().any(|k| normalize_email(k) == norm))
}

fn read_row(store: &DataStore, norm: &str) -> Option<TesterSettings> {
    let conn = store.conn();
    conn.query_row(
        "SELECT premium, unlimited_coins, custom_coins, unlock_all_cosmetics, bypass_cooldowns
         FROM tester_settings WHERE email = ?1",
        params![norm],
        |r| {
            Ok(TesterSettings {
                premium: r.get::<_, i64>(0)? != 0,
                unlimited_coins: r.get::<_, i64>(1)? != 0,
                custom_coins: r.get(2)?,
                unlock_all_cosmetics: r.get::<_, i64>(3)? != 0,
                bypass_cooldowns: r.get::<_, i64>(4)? != 0,
            })
        },
    )
    .ok()
}

/// The sandbox settings for `email`, or `None` if it isn't a sandbox tester.
pub fn settings(store: &DataStore, email: &str) -> Option<TesterSettings> {
    if email.is_empty() {
        return None;
    }
    read_row(store, &normalize_email(email))
}

/// True when `email` is a sandbox tester (has a settings row).
pub fn is_sandboxed(store: &DataStore, email: &str) -> bool {
    settings(store, email).is_some()
}

/// Creates the default settings row if the account has none.
pub fn ensure(store: &DataStore, email: &str) -> TesterSettings {
    let norm = normalize_email(email);
    if let Some(existing) = read_row(store, &norm) {
        return existing;
    }
    let d = TesterSettings::default();
    save(store, email, &d);
    d
}

/// Upserts the whole settings row.
pub fn save(store: &DataStore, email: &str, s: &TesterSettings) {
    let norm = normalize_email(email);
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO tester_settings
           (email, premium, unlimited_coins, custom_coins, unlock_all_cosmetics, bypass_cooldowns, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(email) DO UPDATE SET premium = ?2, unlimited_coins = ?3, custom_coins = ?4,
           unlock_all_cosmetics = ?5, bypass_cooldowns = ?6, updated_at = ?7",
        params![
            norm,
            s.premium as i64,
            s.unlimited_coins as i64,
            s.custom_coins.clamp(0.0, MAX_CUSTOM_COINS),
            s.unlock_all_cosmetics as i64,
            s.bypass_cooldowns as i64,
            now_millis()
        ],
    );
}

/// Removes the sandbox (tester removed from the list): the account goes back to
/// its real balance and real premium status.
pub fn remove(store: &DataStore, email: &str) {
    let conn = store.conn();
    let _ = conn.execute(
        "DELETE FROM tester_settings WHERE email = ?1",
        params![normalize_email(email)],
    );
}

/// The balance a sandbox tester sees: [`UNLIMITED_BALANCE`] or their custom one.
pub fn balance(store: &DataStore, email: &str) -> Option<f64> {
    settings(store, email).map(|s| {
        if s.unlimited_coins {
            UNLIMITED_BALANCE
        } else {
            s.custom_coins
        }
    })
}

/// Applies a balance change for a sandbox tester. `None` means "not a
/// sandbox tester, use the real economy". `require` is the minimum balance the
/// operation needs (a bet or a purchase); if a custom balance can't cover it
/// the result is `Some(Err(()))` and nothing changes. Unlimited balances are
/// never reduced. Returns `(before, after)`.
#[allow(clippy::result_unit_err)]
pub fn apply_delta(
    store: &DataStore,
    email: &str,
    delta: f64,
    require: f64,
) -> Option<Result<(f64, f64), ()>> {
    let norm = normalize_email(email);
    let conn = store.conn();
    if conn.execute_batch("BEGIN IMMEDIATE").is_err() {
        return None;
    }
    let row = conn.query_row(
        "SELECT unlimited_coins, custom_coins FROM tester_settings WHERE email = ?1",
        params![norm],
        |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, f64>(1)?)),
    );
    let Ok((unlimited, custom)) = row else {
        let _ = conn.execute_batch("ROLLBACK");
        return None;
    };
    if unlimited {
        let _ = conn.execute_batch("ROLLBACK");
        return Some(Ok((UNLIMITED_BALANCE, UNLIMITED_BALANCE)));
    }
    if custom < require {
        let _ = conn.execute_batch("ROLLBACK");
        return Some(Err(()));
    }
    let after = round4((custom + delta).clamp(0.0, MAX_CUSTOM_COINS));
    let _ = conn.execute(
        "UPDATE tester_settings SET custom_coins = ?1, updated_at = ?2 WHERE email = ?3",
        params![after, now_millis(), norm],
    );
    let _ = conn.execute_batch("COMMIT");
    Some(Ok((custom, after)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> DataStore {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let base = std::env::temp_dir().join(format!(
            "tester-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn non_testers_have_no_sandbox() {
        let s = store();
        assert!(settings(&s, "a@student.rjuhsd.us").is_none());
        assert!(balance(&s, "a@student.rjuhsd.us").is_none());
        assert!(apply_delta(&s, "a@student.rjuhsd.us", -5.0, 5.0).is_none());
    }

    #[test]
    fn unlimited_never_drains_and_custom_tracks_exactly() {
        let s = store();
        let e = "t@student.rjuhsd.us";
        ensure(&s, e);
        assert_eq!(balance(&s, e), Some(UNLIMITED_BALANCE));
        assert_eq!(
            apply_delta(&s, e, -5000.0, 5000.0),
            Some(Ok((UNLIMITED_BALANCE, UNLIMITED_BALANCE)))
        );
        let mut cfg = settings(&s, e).unwrap();
        cfg.unlimited_coins = false;
        cfg.custom_coins = 50.0;
        save(&s, e, &cfg);
        assert_eq!(balance(&s, e), Some(50.0));
        assert_eq!(apply_delta(&s, e, -20.0, 20.0), Some(Ok((50.0, 30.0))));
        assert_eq!(apply_delta(&s, e, 15.0, 0.0), Some(Ok((30.0, 45.0))));
        // Can't overspend, balance unchanged.
        assert_eq!(apply_delta(&s, e, -100.0, 100.0), Some(Err(())));
        assert_eq!(balance(&s, e), Some(45.0));
        // Never goes negative.
        assert_eq!(apply_delta(&s, e, -45.0, 0.0), Some(Ok((45.0, 0.0))));
    }

    #[test]
    fn custom_balance_is_capped_and_remove_restores_real_account() {
        let s = store();
        let e = "t@student.rjuhsd.us";
        let cfg = TesterSettings {
            unlimited_coins: false,
            custom_coins: 5e12,
            ..Default::default()
        };
        save(&s, e, &cfg);
        assert_eq!(balance(&s, e), Some(MAX_CUSTOM_COINS));
        remove(&s, e);
        assert!(!is_sandboxed(&s, e));
    }

    #[test]
    fn sandbox_flows_through_economy_without_touching_real_coins() {
        let s = store();
        let dir = std::env::temp_dir();
        let e = "t@student.rjuhsd.us";
        // A real balance exists before the account becomes a tester.
        crate::coins::add_coins(&s, &dir, e, 100.0, 1.0, "seed");
        assert_eq!(crate::coins::get_coins(&s, &dir, e), 100.0);
        assert!(!crate::auth::is_premium_email(&s, e));

        ensure(&s, e);
        // Unlimited: reads as the sandbox constant, bets and spends never fail or drain.
        assert_eq!(crate::coins::get_coins(&s, &dir, e), UNLIMITED_BALANCE);
        assert!(crate::coins::spend_coins(&s, &dir, e, 1e12, "x").is_ok());
        assert_eq!(
            crate::coins::settle_bet(&s, &dir, e, 50.0, 0.0, "x"),
            Ok((UNLIMITED_BALANCE, UNLIMITED_BALANCE))
        );
        assert!(crate::auth::is_premium_email(&s, e));

        // Custom: exact accounting through every coin entry point.
        let cfg = TesterSettings {
            unlimited_coins: false,
            custom_coins: 100.0,
            ..Default::default()
        };
        save(&s, e, &cfg);
        assert_eq!(crate::coins::escrow_bet(&s, &dir, e, 40.0, "x"), Ok(60.0));
        assert_eq!(
            crate::coins::settle_escrowed_bet(&s, &dir, e, 40.0, 80.0, "x"),
            Ok(140.0)
        );
        assert!(crate::coins::spend_coins(&s, &dir, e, 500.0, "x").is_err());
        crate::coins::add_coins(&s, &dir, e, -10.0, 1.0, "x");
        assert_eq!(crate::coins::get_coins(&s, &dir, e), 130.0);

        // Removing the sandbox restores the untouched real balance and premium state.
        remove(&s, e);
        assert_eq!(crate::coins::get_coins(&s, &dir, e), 100.0);
        assert!(!crate::auth::is_premium_email(&s, e));
    }
}
