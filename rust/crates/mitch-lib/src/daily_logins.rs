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
