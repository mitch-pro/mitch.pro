//! Per-user state for companion special abilities: Sandevistan strain
//! (with blood and cyberpsychosis past its limits) and chrome upgrades.
//! Tables: `companion_ability_strain`, `companion_upgrades`.
//!
//! Strain model (all per (email, character)):
//! - Every Sandevistan use adds `BASE_STRAIN_PER_USE` × the upgrade
//!   multiplier × (1 + current strain / 100), capped at `MAX_STRAIN`, so
//!   uses close together snowball. Strain drains by `DECAY_PER_MINUTE`
//!   while he rests. Uses are at least `COOLDOWN_MILLIS` apart.
//! - Using it during cyberpsychosis extends the psychosis by
//!   `PSYCHOSIS_EXTENSION_MILLIS` (at most `MAX_PSYCHOSIS_AHEAD_MILLIS` out).
//! - At `BLEEDING_STRAIN` or more he coughs up blood after each use.
//! - Hitting `MAX_STRAIN` starts cyberpsychosis for `PSYCHOSIS_MILLIS`.
//!   When it wears off, strain drops to `POST_PSYCHOSIS_STRAIN` and keeps
//!   draining from there.
//!   Decay and the end of psychosis are computed lazily on read, so there
//!   is no background job.
//!
//! Economy: upgrades are a pure coin sink (nobody is paid), bought once
//! per (email, character), never refunded, and only change how the
//! ability looks and how fast strain builds. Nothing here grants coins.
//! A purchase inserts the row first (`INSERT OR IGNORE`) and only then
//! charges, removing the row again if the charge fails, so two concurrent
//! buys can't both charge, and a failed charge never leaves the upgrade.

use crate::auth::normalize_email;
use crate::data::DataStore;
use crate::school::now_millis;
use serde::Serialize;
use std::path::Path;

pub const MAX_STRAIN: f64 = 100.0;
pub const BASE_STRAIN_PER_USE: f64 = 16.0;
pub const DECAY_PER_MINUTE: f64 = 2.0;
pub const BLEEDING_STRAIN: f64 = 60.0;
pub const POST_PSYCHOSIS_STRAIN: f64 = 50.0;
pub const PSYCHOSIS_MILLIS: i64 = 10 * 60 * 1000;
/// Minimum time between uses, enforced here (the widget and game only
/// mirror it), so the API can't be spammed.
pub const COOLDOWN_MILLIS: i64 = 12_000;
/// Each use during cyberpsychosis drags it out this much longer, up to
/// `MAX_PSYCHOSIS_AHEAD_MILLIS` from now.
pub const PSYCHOSIS_EXTENSION_MILLIS: i64 = 3 * 60 * 1000;
pub const MAX_PSYCHOSIS_AHEAD_MILLIS: i64 = 30 * 60 * 1000;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Upgrade {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// The special ability this upgrade belongs to; only characters with
    /// that ability can take it.
    pub ability: &'static str,
    pub price_coins: f64,
    /// Another upgrade that must already be owned, or "".
    pub requires: &'static str,
}

/// The one catalog of chrome upgrades. The /companion/ page and the
/// widget both read it through `/api/companion/ability`; don't copy it.
pub const UPGRADES: &[Upgrade] = &[
    Upgrade {
        id: "immunosuppressants",
        name: "Immunosuppressants",
        description: "Halves the strain each Sandevistan use puts on him.",
        ability: "sandevistan",
        price_coins: 400.0,
        requires: "",
    },
    Upgrade {
        id: "chrome_arms",
        name: "Chrome arms",
        description: "Reaches farther and moves faster when he helps in Catch the Turkey.",
        ability: "sandevistan",
        price_coins: 600.0,
        requires: "",
    },
    Upgrade {
        id: "cyberskeleton",
        name: "Cyberskeleton",
        description: "Sandevistan lasts twice as long, but every use strains him 50% more.",
        ability: "sandevistan",
        price_coins: 1500.0,
        requires: "chrome_arms",
    },
];

pub fn upgrade(id: &str) -> Option<&'static Upgrade> {
    UPGRADES.iter().find(|u| u.id == id)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AbilityState {
    pub strain: f64,
    pub bleeding: bool,
    /// Millis timestamp; 0 when not in cyberpsychosis.
    pub psychosis_until: i64,
    pub upgrades: Vec<String>,
}

impl AbilityState {
    pub fn in_psychosis(&self, now: i64) -> bool {
        self.psychosis_until > now
    }
}

/// Strain multiplier from owned upgrades (immunosuppressants halve it,
/// the cyberskeleton adds half again).
pub fn strain_multiplier(upgrades: &[String]) -> f64 {
    let mut m = 1.0;
    if upgrades.iter().any(|u| u == "immunosuppressants") {
        m *= 0.5;
    }
    if upgrades.iter().any(|u| u == "cyberskeleton") {
        m *= 1.5;
    }
    m
}

/// Applies decay and the end of psychosis to a stored row, as of `now`.
/// Returns (strain, psychosis_until).
fn settle(strain: f64, updated_at: i64, psychosis_until: i64, now: i64) -> (f64, i64) {
    if psychosis_until > now {
        return (MAX_STRAIN, psychosis_until);
    }
    let (base, since) = if psychosis_until > 0 {
        (
            POST_PSYCHOSIS_STRAIN.min(strain),
            psychosis_until.max(updated_at),
        )
    } else {
        (strain, updated_at)
    };
    let minutes = ((now - since).max(0) as f64) / 60_000.0;
    ((base - minutes * DECAY_PER_MINUTE).max(0.0), 0)
}

fn read_row(store: &DataStore, email: &str, character_id: &str) -> (f64, i64, i64) {
    let conn = store.conn();
    conn.query_row(
        "SELECT strain, updated_at, psychosis_until FROM companion_ability_strain WHERE email = ?1 AND character_id = ?2",
        rusqlite::params![email, character_id],
        |r| Ok((r.get::<_, f64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)),
    )
    .unwrap_or((0.0, 0, 0))
}

pub fn upgrades_owned(store: &DataStore, email: &str, character_id: &str) -> Vec<String> {
    let norm = normalize_email(email);
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(
        "SELECT upgrade FROM companion_upgrades WHERE email = ?1 AND character_id = ?2 ORDER BY purchased_at",
    ) else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![norm, character_id], |r| {
        r.get::<_, String>(0)
    })
    .map(|rows| rows.filter_map(|r| r.ok()).collect())
    .unwrap_or_default()
}

pub fn ability_state(store: &DataStore, email: &str, character_id: &str) -> AbilityState {
    let norm = normalize_email(email);
    let now = now_millis();
    let (strain, updated_at, until) = read_row(store, &norm, character_id);
    let (strain, psychosis_until) = settle(strain, updated_at, until, now);
    AbilityState {
        strain,
        bleeding: strain >= BLEEDING_STRAIN,
        psychosis_until,
        upgrades: upgrades_owned(store, &norm, character_id),
    }
}

/// Records one Sandevistan use and returns the state right after it, or
/// `Err(ms)` with the time left when it's still recharging.
pub fn record_use(store: &DataStore, email: &str, character_id: &str) -> Result<AbilityState, i64> {
    record_use_at(store, email, character_id, now_millis())
}

fn record_use_at(
    store: &DataStore,
    email: &str,
    character_id: &str,
    now: i64,
) -> Result<AbilityState, i64> {
    let norm = normalize_email(email);
    let upgrades = upgrades_owned(store, &norm, character_id);
    let (strain, updated_at, until) = read_row(store, &norm, character_id);
    if updated_at > 0 && now - updated_at < COOLDOWN_MILLIS {
        return Err(COOLDOWN_MILLIS - (now - updated_at));
    }
    let (strain, mut psychosis_until) = settle(strain, updated_at, until, now);
    let strain = if psychosis_until > now {
        psychosis_until =
            (psychosis_until + PSYCHOSIS_EXTENSION_MILLIS).min(now + MAX_PSYCHOSIS_AHEAD_MILLIS);
        MAX_STRAIN
    } else {
        let cost = BASE_STRAIN_PER_USE * strain_multiplier(&upgrades) * (1.0 + strain / MAX_STRAIN);
        (strain + cost).min(MAX_STRAIN)
    };
    if strain >= MAX_STRAIN && psychosis_until <= now {
        psychosis_until = now + PSYCHOSIS_MILLIS;
    }
    {
        let conn = store.conn();
        let _ = conn.execute(
            "INSERT INTO companion_ability_strain (email, character_id, strain, updated_at, psychosis_until) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(email, character_id) DO UPDATE SET strain = ?3, updated_at = ?4, psychosis_until = ?5",
            rusqlite::params![norm, character_id, strain, now, psychosis_until],
        );
    }
    Ok(AbilityState {
        strain,
        bleeding: strain >= BLEEDING_STRAIN,
        psychosis_until,
        upgrades,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub enum BuyError {
    UnknownUpgrade,
    WrongCharacter,
    AlreadyOwned,
    MissingRequirement(&'static str),
    InsufficientFunds,
}

/// Buys `upgrade_id` for (email, character). `character_ability` is the
/// character's `special_ability`; the upgrade must belong to it.
pub fn buy_upgrade(
    store: &DataStore,
    data_dir: &Path,
    email: &str,
    character_id: &str,
    character_ability: &str,
    upgrade_id: &str,
) -> Result<AbilityState, BuyError> {
    let norm = normalize_email(email);
    let Some(up) = upgrade(upgrade_id) else {
        return Err(BuyError::UnknownUpgrade);
    };
    if up.ability != character_ability {
        return Err(BuyError::WrongCharacter);
    }
    let owned = upgrades_owned(store, &norm, character_id);
    if owned.iter().any(|u| u == up.id) {
        return Err(BuyError::AlreadyOwned);
    }
    if !up.requires.is_empty() && !owned.iter().any(|u| u == up.requires) {
        return Err(BuyError::MissingRequirement(up.requires));
    }
    let inserted = {
        let conn = store.conn();
        conn.execute(
            "INSERT OR IGNORE INTO companion_upgrades (email, character_id, upgrade, purchased_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![norm, character_id, up.id, now_millis()],
        )
        .unwrap_or(0)
    };
    if inserted == 0 {
        return Err(BuyError::AlreadyOwned);
    }
    if crate::coins::spend_coins(store, data_dir, &norm, up.price_coins, "companion_upgrade")
        .is_err()
    {
        let conn = store.conn();
        let _ = conn.execute(
            "DELETE FROM companion_upgrades WHERE email = ?1 AND character_id = ?2 AND upgrade = ?3",
            rusqlite::params![norm, character_id, up.id],
        );
        return Err(BuyError::InsufficientFunds);
    }
    Ok(ability_state(store, &norm, character_id))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn strain_decays_and_psychosis_wears_off() {
        // 40 strain, 5 minutes later: 30.
        assert_eq!(settle(40.0, 0, 0, 5 * 60_000), (30.0, 0));
        // Still in psychosis: pinned at max.
        assert_eq!(settle(100.0, 0, 1_000, 500), (MAX_STRAIN, 1_000));
        // Psychosis ended at t=1000; 10 minutes after that, 50 - 20 = 30.
        assert_eq!(settle(100.0, 0, 1_000, 1_000 + 10 * 60_000), (30.0, 0));
        assert_eq!(settle(5.0, 0, 0, 60 * 60_000), (0.0, 0));
    }

    #[test]
    fn upgrades_change_strain() {
        assert_eq!(strain_multiplier(&[]), 1.0);
        assert_eq!(strain_multiplier(&["immunosuppressants".into()]), 0.5);
        assert_eq!(
            strain_multiplier(&["immunosuppressants".into(), "cyberskeleton".into()]),
            0.75
        );
    }

    fn temp_store() -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-chrome-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn buying_upgrades_charges_once_and_checks_requirements() {
        let store = temp_store();
        let dir = store.base_dir.join("data");
        let email = "buyer@example.com";
        let david = crate::companion::DAVID_MARTINEZ_ID;
        let buy = |id: &str| buy_upgrade(&store, &dir, email, david, "sandevistan", id);

        // No coins: refused, and nothing left installed.
        assert_eq!(
            buy("immunosuppressants").unwrap_err(),
            BuyError::InsufficientFunds
        );
        assert!(upgrades_owned(&store, email, david).is_empty());

        crate::coins::add_coins(&store, &dir, email, 1000.0, 1.0, "test");
        let start = crate::coins::get_coins(&store, &dir, email);
        assert!(buy("immunosuppressants").is_ok());
        assert_eq!(crate::coins::get_coins(&store, &dir, email), start - 400.0);
        assert_eq!(
            buy("immunosuppressants").unwrap_err(),
            BuyError::AlreadyOwned
        );
        assert_eq!(crate::coins::get_coins(&store, &dir, email), start - 400.0);
        assert_eq!(
            buy("cyberskeleton").unwrap_err(),
            BuyError::MissingRequirement("chrome_arms")
        );
        assert_eq!(buy("monowire").unwrap_err(), BuyError::UnknownUpgrade);
        assert_eq!(
            buy_upgrade(&store, &dir, email, david, "", "chrome_arms").unwrap_err(),
            BuyError::WrongCharacter
        );
    }

    #[test]
    fn overuse_snowballs_into_cyberpsychosis_and_spam_is_blocked() {
        let store = temp_store();
        let david = crate::companion::DAVID_MARTINEZ_ID;
        let who = "user@example.com";
        let mut now = now_millis();
        let mut last = record_use_at(&store, who, david, now).unwrap();
        assert_eq!(last.strain, BASE_STRAIN_PER_USE);
        // Straight away again: still recharging.
        assert!(record_use_at(&store, who, david, now + 1000).is_err());
        let mut uses = 1;
        while last.psychosis_until == 0 {
            now += COOLDOWN_MILLIS;
            last = record_use_at(&store, who, david, now).unwrap();
            uses += 1;
            if last.strain >= BLEEDING_STRAIN {
                assert!(last.bleeding);
            }
            assert!(uses < 20);
        }
        // Escalating cost: 16, 34.6, 56, 81, 100 (minus a little decay).
        assert_eq!(uses, 5);
        assert!(last.in_psychosis(now));
        let until = last.psychosis_until;
        // Using it again during psychosis drags it out.
        now += COOLDOWN_MILLIS;
        let again = record_use_at(&store, who, david, now).unwrap();
        assert_eq!(again.psychosis_until, until + PSYCHOSIS_EXTENSION_MILLIS);
    }
}
