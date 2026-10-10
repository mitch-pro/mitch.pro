//! Per-user mini-game session state — a real `mini_game_sessions` table
//! (one row per email+game) instead of typing_sessions.json/
//! piano_sessions.json/piccolo_sessions.json/logic_sessions.json, each of
//! which used to be loaded whole into an in-memory `Mutex<HashMap>` at
//! boot and rewritten *in full* on every single payout.
//!
//! The game-specific fields (`dailyCount`, `lastTs`, `currentWordle`, …)
//! stay a JSON blob in the `data` column — this migration is about the
//! storage layer (one indexed row per user instead of a map rewritten
//! whole every time, and in-memory state that a rename could silently
//! desync from), not about reshaping what each mini-game tracks.

use crate::data::DataStore;
use serde_json::Value;

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The stored session data for one user's one mini-game, or `None` if
/// they've never played it.
pub fn mini_session_get(store: &DataStore, norm_email: &str, game: &str) -> Option<Value> {
    let conn = store.conn();
    let raw: Option<String> = conn
        .query_row(
            "SELECT data FROM mini_game_sessions WHERE email = ?1 AND game = ?2",
            rusqlite::params![norm_email, game],
            |r| r.get(0),
        )
        .ok();
    raw.and_then(|s| serde_json::from_str(&s).ok())
}

/// Upserts the whole session data blob for one user's one mini-game.
pub fn mini_session_set(store: &DataStore, norm_email: &str, game: &str, data: &Value) {
    let data_str = serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string());
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO mini_game_sessions (email, game, data, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(email, game) DO UPDATE SET data = ?3, updated_at = ?4",
        rusqlite::params![norm_email, game, data_str, now_millis()],
    );
}

/// Moves every mini-game session row from one email to another (the old
/// generic rename sweep never covered these four blobs at all — a gap
/// this closes rather than carries forward).
pub fn rename_mini_sessions_email(store: &DataStore, old_norm: &str, new_norm: &str) {
    let conn = store.conn();
    let _ = conn.execute(
        "UPDATE mini_game_sessions SET email = ?2 WHERE email = ?1",
        rusqlite::params![old_norm, new_norm],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_store(tag: &str) -> DataStore {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-minigames-test-{tag}-{}",
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
        assert_eq!(mini_session_get(&store, "a@gmail.com", "typing"), None);
        mini_session_set(&store, "a@gmail.com", "typing", &json!({"dailyCount": 3, "lastTs": 100}));
        assert_eq!(
            mini_session_get(&store, "a@gmail.com", "typing"),
            Some(json!({"dailyCount": 3, "lastTs": 100}))
        );
        // A different game for the same user is a separate row.
        assert_eq!(mini_session_get(&store, "a@gmail.com", "piano"), None);
    }

    #[test]
    fn set_overwrites_the_whole_blob() {
        let store = temp_store("overwrite");
        mini_session_set(&store, "a@gmail.com", "logic", &json!({"puzzlesDone": 1}));
        mini_session_set(&store, "a@gmail.com", "logic", &json!({"puzzlesDone": 2, "currentWordle": "APPLE"}));
        assert_eq!(
            mini_session_get(&store, "a@gmail.com", "logic"),
            Some(json!({"puzzlesDone": 2, "currentWordle": "APPLE"}))
        );
    }

    #[test]
    fn rename_moves_rows_across_every_game() {
        let store = temp_store("rename");
        mini_session_set(&store, "old@gmail.com", "typing", &json!({"dailyCount": 1}));
        mini_session_set(&store, "old@gmail.com", "piano", &json!({"dailyCount": 2}));
        rename_mini_sessions_email(&store, "old@gmail.com", "new@gmail.com");
        assert_eq!(mini_session_get(&store, "old@gmail.com", "typing"), None);
        assert_eq!(
            mini_session_get(&store, "new@gmail.com", "typing"),
            Some(json!({"dailyCount": 1}))
        );
        assert_eq!(
            mini_session_get(&store, "new@gmail.com", "piano"),
            Some(json!({"dailyCount": 2}))
        );
    }
}
