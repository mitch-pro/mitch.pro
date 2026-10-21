//! Casino rounds settle once in SQLite with their balance change. No shop effects,
//! earn multipliers, loss refunds, or lifetime-earned credit apply to gambling.
use crate::{auth::normalize_email, data::DataStore};
use rand::TryRngCore;
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};

#[derive(Debug, PartialEq)]
pub enum CasinoError {
    Invalid,
    Insufficient,
    Conflict,
    Unavailable,
}

fn outcomes(game: &str, choice: &str) -> Option<u32> {
    match (game, choice) {
        ("flip", "heads" | "tails") => Some(2),
        ("dice", "under" | "over") => Some(36),
        ("roulette", "red" | "black") => Some(37),
        _ => None,
    }
}

/// Every choice has losing outcomes. Enumerating the finite sample spaces proves
/// expected returns of 97.5%, 95.833…%, and 97.297…%; payouts include the stake.
fn result(game: &str, choice: &str, roll: u32, stake: u32) -> (String, f64) {
    let (label, multiplier) = match game {
        "flip" => {
            let side = if roll == 0 { "heads" } else { "tails" };
            (side.to_string(), if side == choice { 1.95 } else { 0.0 })
        }
        "dice" => {
            let (a, b) = (roll / 6 + 1, roll % 6 + 1);
            let win = (choice == "under" && a + b < 7) || (choice == "over" && a + b > 7);
            (
                format!("{a} + {b} = {}", a + b),
                if win { 2.3 } else { 0.0 },
            )
        }
        _ => {
            let red = [
                1, 3, 5, 7, 9, 12, 14, 16, 18, 19, 21, 23, 25, 27, 30, 32, 34, 36,
            ]
            .contains(&roll);
            let color = if roll == 0 {
                "green"
            } else if red {
                "red"
            } else {
                "black"
            };
            (
                format!("{roll} {color}"),
                if color == choice { 2.0 } else { 0.0 },
            )
        }
    };
    (
        label,
        (f64::from(stake) * multiplier * 10000.0).round() / 10000.0,
    )
}

/// Rejection sampling avoids modulo bias; OS entropy failure aborts the transaction.
fn random_outcome(count: u32) -> Result<u32, CasinoError> {
    let ceiling = u32::MAX - u32::MAX % count;
    loop {
        let n = rand::rngs::OsRng
            .try_next_u32()
            .map_err(|_| CasinoError::Unavailable)?;
        if n < ceiling {
            return Ok(n % count);
        }
    }
}

/// A client-generated ID identifies one intended round, including retries after a
/// lost HTTP response. Reusing it with a different bet fails rather than rerolling.
pub fn play(
    store: &DataStore,
    email: &str,
    id: &str,
    game: &str,
    choice: &str,
    stake: u32,
) -> Result<Value, CasinoError> {
    settle(store, email, id, game, choice, stake, random_outcome)
}

fn settle(
    store: &DataStore,
    email: &str,
    id: &str,
    game: &str,
    choice: &str,
    stake: u32,
    roll: impl FnOnce(u32) -> Result<u32, CasinoError>,
) -> Result<Value, CasinoError> {
    let email = normalize_email(email);
    let count = outcomes(game, choice).ok_or(CasinoError::Invalid)?;
    if email.is_empty()
        || !(1..=10000).contains(&stake)
        || id.len() != 36
        || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
    {
        return Err(CasinoError::Invalid);
    }
    let storage = |_| CasinoError::Unavailable;
    let mut conn = store.conn();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    let previous: Option<String> = tx
        .query_row(
            "SELECT result FROM casino_rounds WHERE email=?1 AND id=?2",
            params![email, id],
            |r| r.get(0),
        )
        .optional()
        .map_err(storage)?;
    if let Some(previous) = previous {
        let value: Value = serde_json::from_str(&previous).map_err(|_| CasinoError::Unavailable)?;
        if value["game"] != game || value["choice"] != choice || value["stake"] != stake {
            return Err(CasinoError::Conflict);
        }
        return Ok(value);
    }
    let balance: f64 = tx
        .query_row("SELECT balance FROM coins WHERE email=?1", [&email], |r| {
            r.get(0)
        })
        .optional()
        .map_err(storage)?
        .unwrap_or(0.0);
    if !balance.is_finite() || balance < f64::from(stake) {
        return Err(CasinoError::Insufficient);
    }
    let outcome = roll(count)?;
    if outcome >= count {
        return Err(CasinoError::Unavailable);
    }
    let (label, payout) = result(game, choice, outcome, stake);
    let after = ((balance - f64::from(stake) + payout) * 10000.0).round() / 10000.0;
    if !after.is_finite() || after < 0.0 {
        return Err(CasinoError::Unavailable);
    }
    let now = chrono::Utc::now().timestamp_millis();
    let value = json!({"id":id,"game":game,"choice":choice,"stake":stake,"outcome":label,"payout":payout,"net":payout-f64::from(stake),"balance":after,"ts":now});
    let encoded = serde_json::to_string(&value).map_err(|_| CasinoError::Unavailable)?;
    tx.execute(
        "UPDATE coins SET balance=?1, updated_at=?2 WHERE email=?3",
        params![after, now, email],
    )
    .map_err(storage)?;
    tx.execute(
        "INSERT INTO casino_rounds(email,id,result,ts) VALUES(?1,?2,?3,?4)",
        params![email, id, encoded, now],
    )
    .map_err(storage)?;
    tx.commit().map_err(storage)?;
    drop(conn);
    crate::coins::log_coin_change(
        store.data_dir(),
        &email,
        balance,
        after,
        &format!("casino {game} round {id}"),
    );
    Ok(value)
}

/// Only the caller's own rounds are returned; the ledger is the source of truth
/// when a response is lost or a client reconnects after settling a bet.
pub fn history(store: &DataStore, email: &str) -> Result<Vec<Value>, CasinoError> {
    let conn = store.conn();
    let mut query = conn
        .prepare(
            "SELECT result FROM casino_rounds WHERE email=?1 ORDER BY ts DESC, rowid DESC LIMIT 25",
        )
        .map_err(|_| CasinoError::Unavailable)?;
    let rows = query
        .query_map([normalize_email(email)], |r| r.get::<_, String>(0))
        .map_err(|_| CasinoError::Unavailable)?;
    rows.map(|r| {
        r.map_err(|_| CasinoError::Unavailable)
            .and_then(|s| serde_json::from_str(&s).map_err(|_| CasinoError::Unavailable))
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (std::path::PathBuf, DataStore) {
        let path =
            std::env::temp_dir().join(format!("casino-{}", crate::crypto::random_bytes_hex(12)));
        std::fs::create_dir_all(path.join("data")).unwrap();
        let store = DataStore::open(&path, &path.join("data")).unwrap();
        crate::coins::set_coins(
            &store,
            &path.join("data"),
            "player@example.test",
            100.0,
            "fixture",
        );
        (path, store)
    }
    #[test]
    fn every_game_has_losing_outcomes_and_negative_expected_return() {
        for (game, choices) in [
            ("flip", ["heads", "tails"]),
            ("dice", ["under", "over"]),
            ("roulette", ["red", "black"]),
        ] {
            let count = outcomes(game, choices[0]).unwrap();
            for choice in choices {
                let payouts: Vec<_> = (0..count).map(|n| result(game, choice, n, 100).1).collect();
                assert!(payouts.contains(&0.0));
                assert!(payouts.iter().sum::<f64>() / f64::from(count) < 100.0);
            }
            let combined: Vec<_> = (0..count)
                .map(|n| {
                    choices
                        .iter()
                        .map(|c| result(game, c, n, 100).1)
                        .sum::<f64>()
                })
                .collect();
            assert!(combined.iter().any(|payout| *payout < 200.0));
            assert!(combined.iter().sum::<f64>() / f64::from(count) < 200.0);
        }
    }
    #[test]
    fn retries_replay_the_original_result_and_never_debit_twice() {
        let (path, store) = fixture();
        let id = "00000000-0000-4000-8000-000000000001";
        let first = settle(
            &store,
            "player@example.test",
            id,
            "flip",
            "heads",
            10,
            |_| Ok(0),
        )
        .unwrap();
        let second = settle(
            &store,
            "player@example.test",
            id,
            "flip",
            "heads",
            10,
            |_| panic!("retry must not draw"),
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(
            crate::coins::get_coins(&store, &path.join("data"), "player@example.test"),
            109.5
        );
        assert_eq!(
            crate::coins::get_lifetime_earned(&store, "player@example.test"),
            0.0
        );
        assert_eq!(
            play(&store, "player@example.test", id, "flip", "tails", 10),
            Err(CasinoError::Conflict)
        );
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn failed_draw_and_invalid_bets_leave_no_debit_or_round() {
        let (path, store) = fixture();
        let id = "00000000-0000-4000-8000-000000000002";
        assert_eq!(
            settle(
                &store,
                "player@example.test",
                id,
                "dice",
                "under",
                10,
                |_| Err(CasinoError::Unavailable)
            ),
            Err(CasinoError::Unavailable)
        );
        assert_eq!(
            play(&store, "player@example.test", id, "flip", "heads", 101),
            Err(CasinoError::Insufficient)
        );
        assert_eq!(
            play(&store, "player@example.test", id, "flip", "heads", 0),
            Err(CasinoError::Invalid)
        );
        assert_eq!(
            crate::coins::get_coins(&store, &path.join("data"), "player@example.test"),
            100.0
        );
        assert!(history(&store, "player@example.test").unwrap().is_empty());
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn simultaneous_duplicate_requests_settle_only_once() {
        let (path, store) = fixture();
        let store = std::sync::Arc::new(store);
        let workers: Vec<_> = (0..12)
            .map(|_| {
                let store = store.clone();
                std::thread::spawn(move || {
                    settle(
                        &store,
                        "player@example.test",
                        "00000000-0000-4000-8000-000000000003",
                        "flip",
                        "heads",
                        100,
                        |_| Ok(1),
                    )
                    .unwrap()
                })
            })
            .collect();
        for worker in workers {
            assert_eq!(worker.join().unwrap()["balance"], 0.0);
        }
        assert_eq!(history(&store, "player@example.test").unwrap().len(), 1);
        assert!(history(&store, "other@example.test").unwrap().is_empty());
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }
}
