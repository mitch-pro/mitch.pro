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

pub struct StreakStatus {
    pub can_claim: bool,
    /// The streak that's actually current right now - 0 if a non-frozen
    /// gap just broke it, same as `streak` otherwise. NOT the raw stored
    /// streak; see daily_login_state's "streak" field comment.
    pub effective_streak: i64,
    pub next_streak: i64,
    pub streak_frozen: bool,
}

/// Pulled out of daily_login_state so the "was the streak just broken"
/// logic is unit-testable without an HTTP request/DataStore - this exact
/// computation was the source of a real bug (the HTTP handler used to
/// report the stale pre-reset `streak` instead of this `effective_streak`).
pub fn compute_streak_status(
    streak: i64,
    freezes: i64,
    last_claim_date: &str,
    today_str: &str,
    yesterday_str: &str,
) -> StreakStatus {
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

    StreakStatus {
        can_claim,
        effective_streak,
        next_streak,
        streak_frozen,
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
    if let Some((code, msg)) = state.rate_limit_check(&client_ip, "anon", "/api/daily-login/state")
    {
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
    let data = mitch_lib::daily_logins::daily_login_get(&state.store, &norm).unwrap_or(json!({}));

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

    let StreakStatus {
        can_claim,
        effective_streak,
        next_streak,
        streak_frozen,
    } = compute_streak_status(
        streak,
        freezes,
        &last_claim_date,
        &today_str,
        &yesterday_str,
    );
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
            // effective_streak, not the raw stored streak: when a streak
            // was just broken (missed a day, no freeze) effective_streak
            // is reset to 0 above but streak still holds the stale
            // pre-reset count. The old UI never showed this (it hides the
            // number whenever a claim is available), but the always-visible
            // streak badge does - a broken streak would otherwise keep
            // proudly displaying the old count. Identical to streak
            // whenever can_claim is false (effective_streak is never
            // reassigned in that branch), so this changes nothing for the
            // already-claimed-today case.
            "streak": effective_streak,
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
    if let Some((code, msg)) = state.rate_limit_check(&client_ip, "anon", "/api/daily-login/claim")
    {
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

    let today_str = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let yesterday_str = (chrono::Utc::now() - chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();

    // The tester hub's "bypass daily claim cooldown" (sandbox testers only).
    let is_bypassing =
        mitch_lib::tester::settings(&state.store, &norm).is_some_and(|t| t.bypass_cooldowns);

    let (streak, freezes, freeze_consumed) = match mitch_lib::daily_logins::daily_login_try_claim(
        &state.store,
        &norm,
        &today_str,
        &yesterday_str,
        is_bypassing,
    ) {
        Ok(res) => res,
        Err(err) => return json_resp(400, json!({ "error": err })),
    };

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

    #[test]
    fn streak_broken_by_a_missed_day_reports_zero_not_the_stale_count() {
        // Claimed day-before-yesterday (2 days ago from "today"), not
        // yesterday, no freeze - the streak should show as broken (0), not
        // the old pre-break count (7).
        let status = compute_streak_status(7, 0, "2026-01-01", "2026-01-04", "2026-01-03");
        assert!(status.can_claim);
        assert_eq!(status.effective_streak, 0);
        assert_eq!(status.next_streak, 1);
        assert!(!status.streak_frozen);
    }

    #[test]
    fn streak_broken_but_frozen_keeps_the_count_and_flags_frozen() {
        let status = compute_streak_status(7, 1, "2026-01-01", "2026-01-04", "2026-01-03");
        assert!(status.can_claim);
        assert_eq!(status.effective_streak, 7);
        assert_eq!(status.next_streak, 8);
        assert!(status.streak_frozen);
    }

    #[test]
    fn streak_continues_normally_when_claimed_yesterday() {
        let status = compute_streak_status(7, 0, "2026-01-03", "2026-01-04", "2026-01-03");
        assert!(status.can_claim);
        assert_eq!(status.effective_streak, 7);
        assert_eq!(status.next_streak, 8);
        assert!(!status.streak_frozen);
    }

    #[test]
    fn already_claimed_today_reports_the_current_streak_unmodified() {
        let status = compute_streak_status(7, 0, "2026-01-04", "2026-01-04", "2026-01-03");
        assert!(!status.can_claim);
        assert_eq!(status.effective_streak, 7);
        assert_eq!(status.next_streak, 7);
        assert!(!status.streak_frozen);
    }

    #[test]
    fn never_claimed_before_starts_fresh() {
        let status = compute_streak_status(0, 0, "", "2026-01-04", "2026-01-03");
        assert!(status.can_claim);
        assert_eq!(status.effective_streak, 0);
        assert_eq!(status.next_streak, 1);
        assert!(!status.streak_frozen);
    }
}
