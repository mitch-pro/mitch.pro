//! Daily login rewards and streak tracking (server.js:19279-19340, 21081-21140).
//!
//! - `GET /api/daily-login/state`
//! - `POST /api/daily-login/claim`

use crate::errors::json_resp;
use crate::routes::me::{cookies_of, me_uid};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::{auth, coins, shop};
use serde_json::json;
use std::sync::Arc;

pub struct DailyReward {
    pub coins: i64,
    pub grant_premium: bool,
}

pub fn get_daily_reward(streak: i64) -> DailyReward {
    if streak == 60 {
        DailyReward {
            coins: 5000,
            grant_premium: true,
        }
    } else if streak > 60 {
        DailyReward {
            coins: 1000 + (streak - 60) * 100,
            grant_premium: false,
        }
    } else if streak >= 31 {
        DailyReward {
            coins: 400 + (streak - 1) * 50,
            grant_premium: false,
        }
    } else if streak >= 15 {
        DailyReward {
            coins: 200 + (streak - 1) * 30,
            grant_premium: false,
        }
    } else if streak >= 8 {
        DailyReward {
            coins: 100 + (streak - 1) * 20,
            grant_premium: false,
        }
    } else {
        DailyReward {
            coins: 50 + (streak.max(1) - 1) * 15,
            grant_premium: false,
        }
    }
}

pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    _body_bytes: &[u8],
) -> Option<Response> {
    if path == "/api/daily-login/state" && *method == Method::GET {
        return Some(daily_login_state(state, headers));
    }
    if path == "/api/daily-login/claim" && *method == Method::POST {
        return Some(daily_login_claim(state, headers));
    }
    None
}

fn daily_login_state(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let client_ip = crate::handler::get_real_ip(headers, None);
    if let Some((code, msg)) = state.rate_limit_check(&client_ip, "anon", "/api/daily-login/state") {
        return json_resp(code, json!({ "error": msg }));
    }

    let cookies = cookies_of(state, headers);
    let uid = me_uid(&cookies);
    if !auth::valid_id(&uid, &state.id_secret) {
        return json_resp(401, json!({ "error": "Not authenticated" }));
    }
    if let Some(ban) = auth::banned_info_for_sid(&state.store, &state.id_secret, &uid) {
        let reason = ban
            .get("reason")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("This account is banned from the website.");
        return json_resp(
            403,
            json!({ "error": "account banned", "banned": true, "reason": reason }),
        );
    }
    let email = match auth::email_from_sid(&state.store, &state.id_secret, &uid) {
        Some(e) if !e.is_empty() => e,
        _ => return json_resp(401, json!({ "error": "Email not found" })),
    };

    let norm = auth::normalize_email(&email);
    let daily_file = state.data_dir().join("daily_logins.json");
    let daily_logins = state.store.read_document(&daily_file, json!({}));
    let data = daily_logins
        .get(norm.as_str())
        .cloned()
        .unwrap_or(json!({}));

    let last_claim_date = data
        .get("lastClaimDate")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let streak = data.get("streak").and_then(|v| v.as_i64()).unwrap_or(0);
    let freezes = data
        .get("streakFreezes")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let today_str = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let yesterday_str = (chrono::Utc::now() - chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();

    let mut can_claim = false;
    let mut effective_streak = streak;
    let mut streak_frozen = false;

    if last_claim_date != today_str {
        can_claim = true;
        if last_claim_date != yesterday_str && !last_claim_date.is_empty() {
            if freezes > 0 {
                streak_frozen = true;
            } else {
                effective_streak = 0;
            }
        }
    }

    let next_streak = if can_claim {
        effective_streak + 1
    } else {
        effective_streak
    };
    let reward_info = get_daily_reward(next_streak);

    let message = if can_claim {
        if streak_frozen {
            format!(
                "❄️ Streak Freeze active! Your {effective_streak}-day streak is protected. Claim Day {next_streak} Login Bonus!"
            )
        } else {
            format!("Claim your Day {next_streak} Login Bonus!")
        }
    } else {
        format!(
            "You claimed today's reward! Come back tomorrow. Current streak: {streak} day{}.",
            if streak == 1 { "" } else { "s" }
        )
    };

    json_resp(
        200,
        json!({
            "canClaim": can_claim,
            "streak": streak,
            "nextStreak": next_streak,
            "nextReward": reward_info.coins,
            "grantPremium": reward_info.grant_premium,
            "lastClaimDate": last_claim_date,
            "streakFreezes": freezes,
            "streakFrozen": streak_frozen,
            "message": message,
        }),
    )
}

fn daily_login_claim(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let client_ip = crate::handler::get_real_ip(headers, None);
    if let Some((code, msg)) = state.rate_limit_check(&client_ip, "anon", "/api/daily-login/claim") {
        return json_resp(code, json!({ "error": msg }));
    }

    let cookies = cookies_of(state, headers);
    let uid = me_uid(&cookies);
    if !auth::valid_id(&uid, &state.id_secret) {
        return json_resp(401, json!({ "error": "Not authenticated" }));
    }
    if let Some(ban) = auth::banned_info_for_sid(&state.store, &state.id_secret, &uid) {
        let reason = ban
            .get("reason")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("This account is banned from the website.");
        return json_resp(
            403,
            json!({ "error": "account banned", "banned": true, "reason": reason }),
        );
    }
    let email = match auth::email_from_sid(&state.store, &state.id_secret, &uid) {
        Some(e) if !e.is_empty() => e,
        _ => return json_resp(401, json!({ "error": "Email not found" })),
    };

    let norm = auth::normalize_email(&email);
    let daily_file = state.data_dir().join("daily_logins.json");
    let mut daily_logins = state.store.read_document(&daily_file, json!({}));
    let mut data = daily_logins
        .get(norm.as_str())
        .cloned()
        .unwrap_or(json!({}));

    let last_claim_date = data
        .get("lastClaimDate")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mut streak = data.get("streak").and_then(|v| v.as_i64()).unwrap_or(0);
    let mut freezes = data
        .get("streakFreezes")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let today_str = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let yesterday_str = (chrono::Utc::now() - chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();

    let is_tester = auth::is_tester_email(&state.store, &email);
    let overrides = state
        .store
        .read_document(&state.data_dir().join("tester_overrides.json"), json!({}));
    let bypass_cooldowns = overrides
        .get(norm.as_str())
        .and_then(|v| v.get("bypassCooldowns"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let is_bypassing = is_tester && bypass_cooldowns;

    if last_claim_date == today_str && !is_bypassing {
        return json_resp(400, json!({ "error": "Already claimed today" }));
    }

    let mut freeze_consumed = false;
    if last_claim_date == yesterday_str {
        streak += 1;
    } else if last_claim_date.is_empty() {
        streak = 1;
    } else {
        if freezes > 0 {
            freezes -= 1;
            streak += 1;
            freeze_consumed = true;
        } else {
            streak = 1;
        }
    }

    if let Some(obj) = data.as_object_mut() {
        obj.insert("lastClaimDate".into(), json!(today_str));
        obj.insert("streak".into(), json!(streak));
        obj.insert("streakFreezes".into(), json!(freezes));
    } else {
        data = json!({
            "lastClaimDate": today_str,
            "streak": streak,
            "streakFreezes": freezes,
        });
    }

    if let Some(map) = daily_logins.as_object_mut() {
        map.insert(norm.clone(), data);
    } else {
        daily_logins = json!({ norm.clone(): data });
    }
    let _ = state.store.write_document(&daily_file, &daily_logins);

    let reward = get_daily_reward(streak);

    let catalog = shop::load_shop_catalog(&state.store, state.data_dir());
    let daily_bonus_item = catalog
        .iter()
        .find(|item| item.get("id").and_then(|v| v.as_str()) == Some("daily_bonus_plus"));
    let has_daily_bonus_plus = daily_bonus_item
        .map(|item| shop::owns_shop_item(&state.store, state.data_dir(), &catalog, &email, item))
        .unwrap_or(false);

    let reward_coins = reward.coins + if has_daily_bonus_plus { 50 } else { 0 };
    let reason = format!(
        "daily-login: streak Day {streak}{}",
        if has_daily_bonus_plus {
            " + daily_bonus_plus"
        } else {
            ""
        }
    );
    coins::add_coins(
        &state.store,
        state.data_dir(),
        &email,
        reward_coins as f64,
        1.0,
        &reason,
    );

    if reward.grant_premium {
        crate::routes::admin::economy::grant_premium_application(
            state,
            &email,
            "system",
            "Earned via 60-day daily login streak",
        );
    }

    let message = if reward.grant_premium {
        format!(
            "Congratulations! You've logged in for 60 consecutive days and earned Premium Status + {reward_coins} MitchCoins!"
        )
    } else if freeze_consumed {
        format!(
            "❄️ Streak Freeze consumed! Your {streak}-day streak was protected! Claimed Day {streak} bonus: +{reward_coins} MitchCoins!"
        )
    } else {
        format!("Claimed Day {streak} bonus: +{reward_coins} MitchCoins!")
    };

    json_resp(
        200,
        json!({
            "success": true,
            "streak": streak,
            "rewardCoins": reward_coins,
            "grantedPremium": reward.grant_premium,
            "grantPremium": reward.grant_premium,
            "freezeConsumed": freeze_consumed,
            "streakFreezes": freezes,
            "message": message,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_reward_brackets() {
        assert_eq!(get_daily_reward(1).coins, 50);
        assert!(!get_daily_reward(1).grant_premium);

        assert_eq!(get_daily_reward(7).coins, 50 + 6 * 15);
        assert_eq!(get_daily_reward(8).coins, 100 + 7 * 20);
        assert_eq!(get_daily_reward(14).coins, 100 + 13 * 20);
        assert_eq!(get_daily_reward(15).coins, 200 + 14 * 30);
        assert_eq!(get_daily_reward(30).coins, 200 + 29 * 30);
        assert_eq!(get_daily_reward(31).coins, 400 + 30 * 50);
        assert_eq!(get_daily_reward(59).coins, 400 + 58 * 50);

        let r60 = get_daily_reward(60);
        assert_eq!(r60.coins, 5000);
        assert!(r60.grant_premium);

        let r61 = get_daily_reward(61);
        assert_eq!(r61.coins, 1100);
        assert!(!r61.grant_premium);
    }
}
