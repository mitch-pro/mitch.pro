//! Daily login streak tracking — real `daily_logins` table (one row per
//! email, real columns since the JSON blob was always exactly
//! `{lastClaimDate, streak, streakFreezes}`) instead of the old
//! daily_logins.json blob.

use crate::data::DataStore;
use serde_json::{json, Value};

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `{lastClaimDate, streak, streakFreezes}` for one user, or `None` if
/// they've never claimed (callers that previously did
/// `read_document(...).get(norm).unwrap_or(json!({}))` should do
/// `daily_login_get(...).unwrap_or(json!({}))`).
pub fn daily_login_get(store: &DataStore, norm_email: &str) -> Option<Value> {
    let conn = store.conn();
    conn.query_row(
        "SELECT last_claim_date, streak, streak_freezes FROM daily_logins WHERE email = ?1",
        rusqlite::params![norm_email],
        |r| {
            Ok(json!({
                "lastClaimDate": r.get::<_, String>(0)?,
                "streak": r.get::<_, i64>(1)?,
                "streakFreezes": r.get::<_, i64>(2)?,
            }))
        },
    )
    .ok()
}

/// Upserts the whole `{lastClaimDate, streak, streakFreezes}` record.
pub fn daily_login_set(store: &DataStore, norm_email: &str, data: &Value) {
    let last_claim_date = data
        .get("lastClaimDate")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let streak = data.get("streak").and_then(|v| v.as_i64()).unwrap_or(0);
    let streak_freezes = data
        .get("streakFreezes")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO daily_logins (email, last_claim_date, streak, streak_freezes, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(email) DO UPDATE SET last_claim_date = ?2, streak = ?3, streak_freezes = ?4, updated_at = ?5",
        rusqlite::params![norm_email, last_claim_date, streak, streak_freezes, now_millis()],
    );
}

/// Atomically checks and claims today's daily login streak in a single BEGIN IMMEDIATE transaction.
/// Returns Ok((new_streak, new_freezes, freeze_consumed)) on success, or Err("Already claimed today") if already claimed.
pub fn daily_login_try_claim(
    store: &DataStore,
    norm_email: &str,
    today_str: &str,
    yesterday_str: &str,
    is_bypassing: bool,
) -> Result<(i64, i64, bool), &'static str> {
    let conn = store.conn();
    if conn.execute_batch("BEGIN IMMEDIATE").is_err() {
        return Err("Database busy");
    }
    let existing: Option<(String, i64, i64)> = conn
        .query_row(
            "SELECT last_claim_date, streak, streak_freezes FROM daily_logins WHERE email = ?1",
            rusqlite::params![norm_email],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            },
        )
        .ok();

    let (last_claim_date, mut streak, mut freezes) = existing.unwrap_or_default();
    if last_claim_date == today_str && !is_bypassing {
        let _ = conn.execute_batch("ROLLBACK");
        return Err("Already claimed today");
    }

    let mut freeze_consumed = false;
    if last_claim_date == yesterday_str {
        streak += 1;
    } else if last_claim_date.is_empty() {
        streak = 1;
    } else if freezes > 0 {
        freezes -= 1;
        streak += 1;
        freeze_consumed = true;
    } else {
        streak = 1;
    }

    let now = now_millis();
    let res = conn.execute(
        "INSERT INTO daily_logins (email, last_claim_date, streak, streak_freezes, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(email) DO UPDATE SET last_claim_date = ?2, streak = ?3, streak_freezes = ?4, updated_at = ?5",
        rusqlite::params![norm_email, today_str, streak, freezes, now],
    );
    if res.is_err() {
        let _ = conn.execute_batch("ROLLBACK");
        return Err("Database error");
    }
    let _ = conn.execute_batch("COMMIT");
    Ok((streak, freezes, freeze_consumed))
}

pub fn rename_daily_login_email(store: &DataStore, old_norm: &str, new_norm: &str) {
    let conn = store.conn();
    let _ = conn.execute(
        "UPDATE daily_logins SET email = ?2 WHERE email = ?1",
        rusqlite::params![old_norm, new_norm],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-daily-logins-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        DataStore::open(&base, &base.join("data")).unwrap()
    }

    #[test]
    fn get_set_round_trip_and_missing_is_none() {
        let store = temp_store("round-trip");
        assert_eq!(daily_login_get(&store, "a@gmail.com"), None);
        daily_login_set(
            &store,
            "a@gmail.com",
            &json!({"lastClaimDate": "2026-01-01", "streak": 3, "streakFreezes": 1}),
        );
        assert_eq!(
            daily_login_get(&store, "a@gmail.com"),
            Some(json!({"lastClaimDate": "2026-01-01", "streak": 3, "streakFreezes": 1}))
        );
    }

    #[test]
    fn rename_moves_the_row() {
        let store = temp_store("rename");
        daily_login_set(
            &store,
            "old@gmail.com",
            &json!({"lastClaimDate": "2026-01-01", "streak": 5, "streakFreezes": 0}),
        );
        rename_daily_login_email(&store, "old@gmail.com", "new@gmail.com");
        assert_eq!(daily_login_get(&store, "old@gmail.com"), None);
        assert_eq!(
            daily_login_get(&store, "new@gmail.com"),
            Some(json!({"lastClaimDate": "2026-01-01", "streak": 5, "streakFreezes": 0}))
        );
    }
}
