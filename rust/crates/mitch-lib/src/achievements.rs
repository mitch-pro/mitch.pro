//! Achievement stat tracking (server.js:3049-3078, 3382-3442) —
//! `ACHIEVEMENT_DEFINITIONS`, `awardAchievement`, `updateStat`,
//! `addPaintingCoin`.
//!
//! `user_stats.json` / `achievements.json` are `loadJson`/`saveJson` files →
//! the SQLite-backed document layer, matching every other consumer.

use crate::auth::normalize_email;
use crate::coins::add_coins;
use crate::data::DataStore;
use crate::school::{now_millis, utc_iso_day};
use serde_json::{json, Map, Value};
use std::path::Path;

/// One definition entry (server.js:3049-3078).
pub struct AchievementDef {
    pub name: &'static str,
    pub desc: &'static str,
    pub bonus: f64,
    pub goal: f64,
    pub stat: &'static str,
}

/// `ACHIEVEMENT_DEFINITIONS` — source order preserved (Object.entries order).
pub const ACHIEVEMENT_DEFINITIONS: &[(&str, AchievementDef)] = &[
    (
        "painter_1",
        AchievementDef {
            name: "Painter I",
            desc: "Place 100 pixels",
            bonus: 100.0,
            goal: 100.0,
            stat: "pixels",
        },
    ),
    (
        "painter_2",
        AchievementDef {
            name: "Painter II",
            desc: "Place 1000 pixels",
            bonus: 500.0,
            goal: 1000.0,
            stat: "pixels",
        },
    ),
    (
        "chess_win_1",
        AchievementDef {
            name: "First Win",
            desc: "Win your first chess game",
            bonus: 100.0,
            goal: 1.0,
            stat: "chess_wins",
        },
    ),
    (
        "chess_win_2",
        AchievementDef {
            name: "Grandmaster",
            desc: "Win 10 chess games",
            bonus: 500.0,
            goal: 10.0,
            stat: "chess_wins",
        },
    ),
    (
        "puzzle_master",
        AchievementDef {
            name: "Puzzle Master",
            desc: "Solve 50 puzzles",
            bonus: 250.0,
            goal: 50.0,
            stat: "puzzles_solved",
        },
    ),
    (
        "casino_novice",
        AchievementDef {
            name: "High Roller I",
            desc: "Play 10 casino games",
            bonus: 100.0,
            goal: 10.0,
            stat: "casino_bets",
        },
    ),
    (
        "casino_regular",
        AchievementDef {
            name: "High Roller II",
            desc: "Play 100 casino games",
            bonus: 500.0,
            goal: 100.0,
            stat: "casino_bets",
        },
    ),
    (
        "casino_winner",
        AchievementDef {
            name: "Lucky Streak",
            desc: "Win 50 casino games",
            bonus: 250.0,
            goal: 50.0,
            stat: "casino_wins",
        },
    ),
    (
        "clicker_1",
        AchievementDef {
            name: "Clicker Novice",
            desc: "Reach 1,000,000 clicker points",
            bonus: 100.0,
            goal: 1_000_000.0,
            stat: "clicker_points",
        },
    ),
    (
        "clicker_2",
        AchievementDef {
            name: "Clicker Master",
            desc: "Reach 100,000,000 clicker points",
            bonus: 500.0,
            goal: 100_000_000.0,
            stat: "clicker_points",
        },
    ),
    (
        "typing_1",
        AchievementDef {
            name: "Typist Novice",
            desc: "Play 10 typing races",
            bonus: 100.0,
            goal: 10.0,
            stat: "typing_races",
        },
    ),
    (
        "typing_2",
        AchievementDef {
            name: "Typist Master",
            desc: "Play 100 typing races",
            bonus: 500.0,
            goal: 100.0,
            stat: "typing_races",
        },
    ),
    (
        "logic_1",
        AchievementDef {
            name: "Logic Novice",
            desc: "Solve 10 logic puzzles",
            bonus: 100.0,
            goal: 10.0,
            stat: "logic_puzzles",
        },
    ),
    (
        "logic_2",
        AchievementDef {
            name: "Logic Master",
            desc: "Solve 100 logic puzzles",
            bonus: 500.0,
            goal: 100.0,
            stat: "logic_puzzles",
        },
    ),
    (
        "richard_1",
        AchievementDef {
            name: "Rich Friends",
            desc: "Earn 1,000 coins from Richard",
            bonus: 100.0,
            goal: 1000.0,
            stat: "richard_coins",
        },
    ),
    (
        "richard_2",
        AchievementDef {
            name: "Royal Riches",
            desc: "Earn 10,000 coins from Richard",
            bonus: 500.0,
            goal: 10000.0,
            stat: "richard_coins",
        },
    ),
    (
        "piano_1",
        AchievementDef {
            name: "Pianist Novice",
            desc: "Play 10 piano games",
            bonus: 100.0,
            goal: 10.0,
            stat: "piano_games",
        },
    ),
    (
        "piano_2",
        AchievementDef {
            name: "Pianist Master",
            desc: "Play 100 piano games",
            bonus: 500.0,
            goal: 100.0,
            stat: "piano_games",
        },
    ),
    (
        "battleship_1",
        AchievementDef {
            name: "Commodore",
            desc: "Win 1 battleship game",
            bonus: 100.0,
            goal: 1.0,
            stat: "battleship_wins",
        },
    ),
    (
        "battleship_2",
        AchievementDef {
            name: "Fleet Admiral",
            desc: "Win 10 battleship games",
            bonus: 500.0,
            goal: 10.0,
            stat: "battleship_wins",
        },
    ),
    (
        "jeopardy_1",
        AchievementDef {
            name: "Smart Contestant",
            desc: "Win 1 Jeopardy game",
            bonus: 100.0,
            goal: 1.0,
            stat: "jeopardy_wins",
        },
    ),
    (
        "jeopardy_2",
        AchievementDef {
            name: "Jeopardy Legend",
            desc: "Win 10 Jeopardy games",
            bonus: 500.0,
            goal: 10.0,
            stat: "jeopardy_wins",
        },
    ),
];

fn user_stats_file(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("user_stats.json")
}
fn achievements_file(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("achievements.json")
}

/// `updateStat(email, key, inc = 1)` (server.js:3406-3420) — increments the
/// stat (`(stats[norm][key] || 0) + inc`), then walks EVERY definition whose
/// `stat` matches; awardAchievement itself dedupes. Returns the new total,
/// or 0 for an empty email.
pub fn update_stat(
    store: &DataStore,
    data_dir: &Path,
    email: &str,
    key: &str,
    inc: f64,
    coin_multiplier: f64,
) -> f64 {
    if email.is_empty() {
        return 0.0;
    }
    let norm = normalize_email(email);
    let mut stats: Map<String, Value> = store
        .read_document(&user_stats_file(data_dir), json!({}))
        .as_object()
        .cloned()
        .unwrap_or_default();
    let entry = stats.entry(norm.clone()).or_insert_with(|| json!({}));
    if !entry.is_object() {
        // JS `stats[norm][key] = ...` on a primitive would throw; callers only
        // ever see object entries, so replace a corrupt one the crash-path way.
        *entry = json!({});
    }
    let current = entry.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0);
    let total = current + inc;
    if let Some(obj) = entry.as_object_mut() {
        obj.insert(key.to_string(), json!(total));
    }
    let _ = store.write_document(&user_stats_file(data_dir), &Value::Object(stats));

    for (id, def) in ACHIEVEMENT_DEFINITIONS {
        if def.stat == key && total >= def.goal {
            award_achievement(store, data_dir, &norm, id, def.bonus, coin_multiplier);
        }
    }
    total
}

/// `awardAchievement(email, achId, bonus = 0)` (server.js:3383-3394) —
/// idempotent per email; a positive bonus pays through addCoins (norm passed,
/// so the personal-happy-hour path sees the normalized email either way).
pub fn award_achievement(
    store: &DataStore,
    data_dir: &Path,
    email: &str,
    ach_id: &str,
    bonus: f64,
    coin_multiplier: f64,
) -> bool {
    if email.is_empty() {
        return false;
    }
    let norm = normalize_email(email);
    let mut achs: Map<String, Value> = store
        .read_document(&achievements_file(data_dir), json!({}))
        .as_object()
        .cloned()
        .unwrap_or_default();
    let list = achs.entry(norm.clone()).or_insert_with(|| json!([]));
    let already = list
        .as_array()
        .map(|a| a.iter().any(|v| v.as_str() == Some(ach_id)))
        .unwrap_or(false);
    if already {
        return false;
    }
    if let Some(arr) = list.as_array_mut() {
        arr.push(json!(ach_id));
    }
    let _ = store.write_document(&achievements_file(data_dir), &Value::Object(achs));
    if bonus > 0.0 {
        add_coins(store, data_dir, &norm, bonus, coin_multiplier, "");
    }
    true
}

/// `addPaintingCoin(email)` (server.js:3424-3442) — UTC-day reset ladder,
/// 1000-pixels/day cap (200 coins), then `updateStat(email, 'pixels', 1)`
/// unconditionally (the pixels stat grows even on capped days).
pub fn add_painting_coin(store: &DataStore, data_dir: &Path, email: &str, coin_multiplier: f64) {
    if email.is_empty() {
        return;
    }
    let norm = normalize_email(email);
    let today = utc_iso_day(now_millis());
    let mut stats: Map<String, Value> = store
        .read_document(&user_stats_file(data_dir), json!({}))
        .as_object()
        .cloned()
        .unwrap_or_default();
    let entry = stats.entry(norm.clone()).or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }
    if entry.get("last_paint_day").and_then(|v| v.as_str()) != Some(today.as_str()) {
        if let Some(obj) = entry.as_object_mut() {
            obj.insert("last_paint_day".into(), json!(today));
            obj.insert("day_paint_count".into(), json!(0));
        }
        let _ = store.write_document(&user_stats_file(data_dir), &Value::Object(stats.clone()));
    }
    // The JS re-loads here (`const s = loadUserStats()`); the day-reset save
    // already landed, so a fresh read is the equivalent.
    let day_count = stats
        .get(&norm)
        .and_then(|s| s.get("day_paint_count"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    if day_count < 1000.0 {
        if let Some(obj) = stats.get_mut(&norm).and_then(|s| s.as_object_mut()) {
            let count = obj
                .get("day_paint_count")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            obj.insert("day_paint_count".into(), json!(count + 1.0));
        }
        let _ = store.write_document(&user_stats_file(data_dir), &Value::Object(stats));
        add_coins(store, data_dir, email, 0.2, coin_multiplier, "");
    }
    update_stat(store, data_dir, email, "pixels", 1.0, coin_multiplier);
}

#[cfg(test)]
mod tests {
    #![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
    use super::*;

    fn fresh_store(name: &str) -> (DataStore, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "ach-test-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap();
        let store = DataStore::open(&dir, &dir.join("data")).unwrap();
        (store, dir.join("data"))
    }

    #[test]
    fn stat_increment_and_award_once() {
        let (store, data_dir) = fresh_store("stats");
        let mult = 1.0;
        // 99 paints → no award; the 100th crosses painter_1 exactly once.
        assert_eq!(
            update_stat(&store, &data_dir, "a@x.com", "pixels", 99.0, mult),
            99.0
        );
        assert_eq!(
            update_stat(&store, &data_dir, "a@x.com", "pixels", 1.0, mult),
            100.0
        );
        let achs = store.read_document(&achievements_file(&data_dir), json!({}));
        assert_eq!(
            achs.get("a@x.com").and_then(|v| v.as_array()).map(Vec::len),
            Some(1)
        );
        // 900 more crosses painter_2; painter_1 is not re-awarded.
        assert_eq!(
            update_stat(&store, &data_dir, "a@x.com", "pixels", 900.0, mult),
            1000.0
        );
        let achs = store.read_document(&achievements_file(&data_dir), json!({}));
        assert_eq!(
            achs.get("a@x.com").and_then(|v| v.as_array()).map(Vec::len),
            Some(2)
        );
        // Empty email is a 0 no-op.
        assert_eq!(update_stat(&store, &data_dir, "", "pixels", 1.0, mult), 0.0);
    }

    #[test]
    fn painting_coin_day_cap() {
        let (store, data_dir) = fresh_store("paint-coin");
        let mult = 1.0;
        add_painting_coin(&store, &data_dir, "p@x.com", mult);
        let stats = store.read_document(&user_stats_file(&data_dir), json!({}));
        let entry = stats.get("p@x.com").unwrap();
        assert_eq!(
            entry.get("day_paint_count").and_then(|v| v.as_f64()),
            Some(1.0)
        );
        assert_eq!(entry.get("pixels").and_then(|v| v.as_f64()), Some(1.0));
        // Coin balance reflects the 0.2 paint coin (no multiplier).
        assert_eq!(crate::coins::get_coins(&store, &data_dir, "p@x.com"), 0.2);
        // Empty email is a no-op (no coins row created).
        add_painting_coin(&store, &data_dir, "", mult);
        let coins = crate::coins::load_coins(&store, &data_dir);
        assert_eq!(coins.as_object().map(Map::len), Some(1));
    }
}
