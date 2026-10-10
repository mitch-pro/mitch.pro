//! `/api/me/inventory` + `/api/me/cosmetics/equip` (server.js:11347-11355,
//! 12281-12320) + `/api/shop/buy` (server.js:13276-13403).

use super::{cookies_of, data_file, json_response, parse_body_strict};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::auth;
use serde_json::{json, Value};
use std::sync::Arc;

pub(crate) async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body: &Value,
    body_bytes: &[u8],
) -> Option<Response> {
    if path == "/api/shop/items" && *method == Method::GET {
        return Some(shop_items(state, headers));
    }
    if path == "/api/shop/buy" && *method == Method::POST {
        return Some(shop_buy(state, headers, body, body_bytes).await);
    }
    if path == "/api/me/inventory" && *method == Method::GET {
        return Some(me_inventory(state, headers));
    }
    if path == "/api/me/cosmetics/equip" && *method == Method::POST {
        return Some(me_cosmetics_equip(state, headers, body, body_bytes));
    }
    None
}

/// `GET /api/shop/items` — server.js:12018-12027.
fn shop_items(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let cookies = cookies_of(state, headers);
    let sid = super::me_uid(&cookies);
    let email = if auth::valid_id(&sid, &state.id_secret) {
        auth::email_from_sid(&state.store, &state.id_secret, &sid).unwrap_or_default()
    } else {
        String::new()
    };
    let catalog = mitch_lib::shop::load_shop_catalog(&state.store, state.data_dir());
    let items = mitch_lib::shop::shop_items_for(&state.store, state.data_dir(), &catalog, &email);
    json_response(
        200,
        json!({
            "items": items,
            "premiumDiscountPct": 0,
            "premiumDiscountNote": "Premium discounts vary by item."
        }),
    )
}

/// `POST /api/shop/buy` — server.js:13276-13403. Was never ported to Rust
/// at all (the frontend's `/api/shop/buy` call just 404'd), so every
/// purchase failed regardless of balance.
async fn shop_buy(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body: &Value,
    body_bytes: &[u8],
) -> Response {
    if parse_body_strict(body_bytes).is_none() {
        return json_response(400, json!({ "error": "bad json" }));
    }
    let cookies = cookies_of(state, headers);
    let sid = cookies.auth_sid();
    if !auth::valid_id(&sid, &state.id_secret) {
        return json_response(401, json!({ "error": "unauthorized" }));
    }
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &sid) else {
        return json_response(401, json!({ "error": "email not found" }));
    };

    let ip = crate::handler::get_real_ip(headers, None);
    let token = body
        .get("recaptcha_token")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !crate::routes::push::verify_recaptcha(state, token, &ip, &sid).await {
        return json_response(
            400,
            json!({ "error": "reCAPTCHA failed. Please try again." }),
        );
    }

    let norm = auth::normalize_email(&email);
    let item_id = body
        .get("itemId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // activeShopItemById: the live catalog only, no fallback to retired
    // defaults — an item that's been removed from the catalog shouldn't
    // still be buyable just because it's hardcoded in the JS-era defaults.
    let catalog = mitch_lib::shop::load_shop_catalog(&state.store, state.data_dir());
    let Some(item) = mitch_lib::shop::shop_item_by_id(&catalog, &item_id).cloned() else {
        return json_response(400, json!({ "error": "invalid shop item" }));
    };
    if item.get("type").and_then(|v| v.as_str()) == Some("premium") {
        return json_response(400, json!({ "error": "invalid shop item" }));
    }
    let disabled = item.get("disabled").and_then(|v| v.as_bool()).unwrap_or(false);
    if disabled && !auth::is_admin_email(&state.store, &email) {
        return json_response(
            400,
            json!({ "error": "This item is disabled / limited-edition and can only be bought player-to-player in the Marketplace!" }),
        );
    }
    let admin_only = item
        .get("adminOnly")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if admin_only && !auth::is_admin_email(&state.store, &email) {
        return json_response(403, json!({ "error": "Admins and owners only." }));
    }
    let premium_only = item
        .get("premiumOnly")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if premium_only
        && !auth::is_premium_email(&state.store, &email)
        && !auth::is_admin_email(&state.store, &email)
    {
        return json_response(403, json!({ "error": "Premium required for this item." }));
    }

    if mitch_lib::shop::owns_shop_item(&state.store, state.data_dir(), &catalog, &email, &item) {
        return json_response(400, json!({ "error": "You already own this item." }));
    }

    let cost = mitch_lib::shop::shop_cost_for(&state.store, state.data_dir(), &item, &email);
    let balance = mitch_lib::coins::get_coins(&state.store, state.data_dir(), &email);
    if balance < cost as f64 {
        return json_response(
            400,
            json!({ "error": format!("Insufficient coins. Need {cost}, have {balance:.2}.") }),
        );
    }

    let cost_type = item
        .get("costType")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if let Some((bucket, _active)) = mitch_lib::shop::shop_type_config(&cost_type) {
        let cosmetics_file = data_file(state, "cosmetics.json");
        let mut cosm_all = state.store.read_document(&cosmetics_file, json!({}));
        let entry = cosm_all.get(norm.as_str()).cloned().unwrap_or(json!({}));
        let mut owned = mitch_lib::shop::sanitize_cosmetics_for_email(&state.store, &email, &entry);
        if mitch_lib::shop::cosmetics_bucket_has(&owned, bucket, &item_id) {
            return json_response(400, json!({ "error": "You already own this item." }));
        }
        if let Some(arr) = owned.get_mut(bucket).and_then(|v| v.as_array_mut()) {
            arr.push(json!(item_id));
        }
        if let Some(obj) = cosm_all.as_object_mut() {
            obj.insert(norm.clone(), owned);
        }
        let _ = state.store.write_document(&cosmetics_file, &cosm_all);
    } else if cost_type == "ai_personality" {
        let unlocked_file = data_file(state, "unlocked_ai.json");
        let mut unlocked = state.store.read_document(&unlocked_file, json!({}));
        let mut mine: Vec<String> = unlocked
            .get(norm.as_str())
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if mine.iter().any(|m| m == &item_id) {
            return json_response(400, json!({ "error": "You already unlocked this personality." }));
        }
        mine.push(item_id.clone());
        if let Some(obj) = unlocked.as_object_mut() {
            obj.insert(norm.clone(), json!(mine));
        }
        let _ = state.store.write_document(&unlocked_file, &unlocked);
    } else if cost_type == "vip_casino_pass" {
        let minutes = item
            .get("durationMinutes")
            .and_then(|v| v.as_f64())
            .unwrap_or(1440.0);
        bump_stat_until(state, &norm, "vip_casino_until", minutes);
    } else if cost_type == "streak_freeze" {
        let daily_file = data_file(state, "daily_logins.json");
        let mut daily = state.store.read_document(&daily_file, json!({}));
        let mut entry = daily.get(norm.as_str()).cloned().unwrap_or(json!({
            "lastClaimDate": "", "streak": 0, "streakFreezes": 0
        }));
        let freezes = entry.get("streakFreezes").and_then(|v| v.as_i64()).unwrap_or(0);
        if freezes >= 3 {
            return json_response(
                400,
                json!({ "error": "You can only hold a maximum of 3 Streak Freezes." }),
            );
        }
        if let Some(obj) = entry.as_object_mut() {
            obj.insert("streakFreezes".to_string(), json!(freezes + 1));
        }
        if let Some(obj) = daily.as_object_mut() {
            obj.insert(norm.clone(), entry);
        }
        let _ = state.store.write_document(&daily_file, &daily);
    } else if cost_type == "happy_hour_ticket" {
        let minutes = item
            .get("durationMinutes")
            .and_then(|v| v.as_f64())
            .unwrap_or(30.0);
        bump_stat_until(state, &norm, "personal_happy_hour_until", minutes);
    } else if cost_type == "double_down_ticket" {
        let minutes = item
            .get("durationMinutes")
            .and_then(|v| v.as_f64())
            .unwrap_or(30.0);
        bump_stat_until(state, &norm, "double_down_until", minutes);
    } else if cost_type == "bad_beat_insurance" {
        let minutes = item
            .get("durationMinutes")
            .and_then(|v| v.as_f64())
            .unwrap_or(30.0);
        bump_stat_until(state, &norm, "bad_beat_insurance_until", minutes);
    } else if cost_type == "happy_hour_extension" {
        let user_stats_file = data_file(state, "user_stats.json");
        let mut stats = state.store.read_document(&user_stats_file, json!({}));
        let mut entry = stats.get(norm.as_str()).cloned().unwrap_or(json!({}));
        let current = entry
            .get("personal_happy_hour_until")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let now = mitch_lib::school::now_millis() as f64;
        if current <= now {
            return json_response(
                400,
                json!({ "error": "You must have an active Personal Happy Hour to extend it." }),
            );
        }
        if let Some(obj) = entry.as_object_mut() {
            obj.insert(
                "personal_happy_hour_until".to_string(),
                json!(current + 15.0 * 60.0 * 1000.0),
            );
        }
        if let Some(obj) = stats.as_object_mut() {
            obj.insert(norm.clone(), entry);
        }
        let _ = state.store.write_document(&user_stats_file, &stats);
    } else if cost_type == "slots_free_spin" {
        let user_stats_file = data_file(state, "user_stats.json");
        let mut stats = state.store.read_document(&user_stats_file, json!({}));
        let mut entry = stats.get(norm.as_str()).cloned().unwrap_or(json!({}));
        let current = entry
            .get("slots_free_spins")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let add = item.get("spinCount").and_then(|v| v.as_i64()).unwrap_or(5);
        if let Some(obj) = entry.as_object_mut() {
            obj.insert("slots_free_spins".to_string(), json!(current + add));
        }
        if let Some(obj) = stats.as_object_mut() {
            obj.insert(norm.clone(), entry);
        }
        let _ = state.store.write_document(&user_stats_file, &stats);
    } else if cost_type == "loaded_dice" {
        bump_stat_until(state, &norm, "loaded_dice_until", 30.0);
    } else if cost_type == "casino_glitch_chip" {
        bump_stat_until(state, &norm, "casino_glitch_until", 20.0);
    } else if cost_type == "infinite_luck_charm" {
        let user_stats_file = data_file(state, "user_stats.json");
        let mut stats = state.store.read_document(&user_stats_file, json!({}));
        let mut entry = stats.get(norm.as_str()).cloned().unwrap_or(json!({}));
        let now = mitch_lib::school::now_millis() as f64;
        let current = entry
            .get("infinite_luck_until")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let until = now.max(current) + 30.0 * 60.0 * 1000.0;
        if let Some(obj) = entry.as_object_mut() {
            obj.insert("infinite_luck_until".to_string(), json!(until));
            for key in [
                "loaded_dice_until",
                "casino_glitch_until",
                "bad_beat_insurance_until",
                "vip_casino_until",
            ] {
                let existing = obj.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0);
                obj.insert(key.to_string(), json!(existing.max(until)));
            }
        }
        if let Some(obj) = stats.as_object_mut() {
            obj.insert(norm.clone(), entry);
        }
        let _ = state.store.write_document(&user_stats_file, &stats);
    }

    if cost > 0 {
        mitch_lib::coins::add_coins(
            &state.store,
            state.data_dir(),
            &email,
            -(cost as f64),
            1.0,
            "shop_purchase",
        );
    }
    json_response(
        200,
        json!({ "ok": true, "message": "Purchase successful!", "cost": cost }),
    )
}

/// `stats[norm][key] = max(now, current) + minutes * 60_000` — the shared
/// shape behind vip_casino_pass/happy_hour_ticket/double_down_ticket/
/// bad_beat_insurance/loaded_dice/casino_glitch_chip's "extend from now or
/// from whenever the existing grant runs out" duration stacking.
fn bump_stat_until(state: &Arc<AppState>, norm: &str, key: &str, duration_minutes: f64) {
    let file = data_file(state, "user_stats.json");
    let mut stats = state.store.read_document(&file, json!({}));
    let mut entry = stats.get(norm).cloned().unwrap_or(json!({}));
    let now = mitch_lib::school::now_millis() as f64;
    let current = entry.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0);
    let until = now.max(current) + duration_minutes * 60.0 * 1000.0;
    if let Some(obj) = entry.as_object_mut() {
        obj.insert(key.to_string(), json!(until));
    }
    if let Some(obj) = stats.as_object_mut() {
        obj.insert(norm.to_string(), entry);
    }
    let _ = state.store.write_document(&file, &stats);
}

/// `GET /api/me/inventory` — server.js:11347-11356. Note the different 401
/// wording (`not logged in`) versus equip's `unauthorized`.
fn me_inventory(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let cookies = cookies_of(state, headers);
    let sid = cookies.auth_sid();
    if !auth::valid_id(&sid, &state.id_secret) {
        return json_response(401, json!({ "error": "not logged in" }));
    }
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &sid) else {
        return json_response(401, json!({ "error": "email not found" }));
    };
    let catalog = mitch_lib::shop::load_shop_catalog(&state.store, state.data_dir());
    let inventory =
        mitch_lib::shop::build_inventory(&state.store, state.data_dir(), &catalog, &email);
    json_response(200, inventory)
}

/// `POST /api/me/cosmetics/equip` — server.js:12281-12324.
fn me_cosmetics_equip(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body: &Value,
    body_bytes: &[u8],
) -> Response {
    if parse_body_strict(body_bytes).is_none() {
        return json_response(400, json!({ "error": "bad json" }));
    }
    let cookies = cookies_of(state, headers);
    let sid = cookies.auth_sid();
    if !auth::valid_id(&sid, &state.id_secret) {
        return json_response(401, json!({ "error": "unauthorized" }));
    }
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &sid) else {
        return json_response(401, json!({ "error": "email not found" }));
    };
    let norm = auth::normalize_email(&email);

    let item_id = body.get("itemId").cloned().unwrap_or(Value::Null);
    let equip_type = body
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let cosmetics_file = data_file(state, "cosmetics.json");
    let mut cosm = state.store.read_document(&cosmetics_file, json!({}));
    let entry = cosm.get(norm.as_str()).cloned().unwrap_or(json!({}));
    let mut user_cosm = mitch_lib::shop::sanitize_cosmetics_for_email(&state.store, &email, &entry);

    // `String(itemId || '')` — null/undefined/0 collapse to ''.
    let next_item_id = match &item_id {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(false) => String::new(),
        Value::Number(n) if n.as_f64() == Some(0.0) => String::new(),
        other => other.to_string(),
    };
    let catalog = mitch_lib::shop::load_shop_catalog(&state.store, state.data_dir());

    if let Some((bucket, active)) = mitch_lib::shop::shop_type_config(&equip_type) {
        let item = if next_item_id.is_empty() {
            None
        } else {
            mitch_lib::shop::shop_item_by_id(&catalog, &next_item_id)
        };
        if !next_item_id.is_empty() {
            let cost_type_matches = item
                .and_then(|i| i.get("costType"))
                .and_then(|v| v.as_str())
                .map(|ct| ct == equip_type.as_str())
                .unwrap_or(false);
            if !cost_type_matches {
                return json_response(400, json!({ "error": "invalid item" }));
            }
            let admin_only = item
                .and_then(|i| i.get("adminOnly"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if admin_only && !auth::is_admin_email(&state.store, &email) {
                return json_response(403, json!({ "error": "Admins and owners only." }));
            }
            if !auth::is_admin_email(&state.store, &email)
                && !mitch_lib::shop::cosmetics_bucket_has(&user_cosm, bucket, &next_item_id)
            {
                return json_response(403, json!({ "error": "You do not own this item" }));
            }
        }
        if let Some(obj) = user_cosm.as_object_mut() {
            obj.insert(active.to_string(), json!(next_item_id));
        }
    } else if equip_type == "ai_personality" {
        let item = if next_item_id.is_empty() {
            None
        } else {
            mitch_lib::shop::shop_item_by_id(&catalog, &next_item_id)
        };
        let unlocked = state
            .store
            .read_document(&data_file(state, "unlocked_ai.json"), json!({}));
        let mine: Vec<String> = unlocked
            .get(norm.as_str())
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if !next_item_id.is_empty() {
            let cost_type_matches = item
                .and_then(|i| i.get("costType"))
                .and_then(|v| v.as_str())
                .map(|ct| ct == "ai_personality")
                .unwrap_or(false);
            if !cost_type_matches {
                return json_response(400, json!({ "error": "invalid item" }));
            }
            if !auth::is_admin_email(&state.store, &email)
                && !mine.iter().any(|m| m == &next_item_id)
            {
                return json_response(403, json!({ "error": "You do not own this personality" }));
            }
        }
        if let Some(obj) = user_cosm.as_object_mut() {
            obj.insert("activeAi".to_string(), json!(next_item_id));
        }
    } else {
        return json_response(400, json!({ "error": "invalid type" }));
    }

    if let Some(obj) = cosm.as_object_mut() {
        obj.insert(norm.clone(), user_cosm);
    }
    if state.store.write_document(&cosmetics_file, &cosm).is_err() {
        return json_response(400, json!({ "error": "save failed" }));
    }
    json_response(200, json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::HeaderValue;

    fn test_state() -> (Arc<AppState>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "mitch-server-shop-buy-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap_or_default();
        let cfg = crate::hosts::SiteConfig::load();
        let cfg = crate::hosts::SiteConfig {
            data_dir: dir.join("data"),
            ..cfg
        };
        let store = Arc::new(
            mitch_lib::data::DataStore::open(&dir, &dir.join("data"))
                .unwrap_or_else(|e| panic!("store: {e}")),
        );
        (Arc::new(AppState::new(cfg, Arc::clone(&store))), dir)
    }

    fn auth_headers(state: &AppState, email: &str) -> HeaderMap {
        let sess = auth::create_auth_session(
            &state.store,
            &state.id_secret,
            &auth::normalize_email(email),
            email,
            "",
            "",
            false,
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_str(&format!(
                "mitch_session={}; studentId={}",
                sess.token, sess.sid
            ))
            .unwrap(),
        );
        headers
    }

    async fn body_json(resp: Response) -> Value {
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn shop_buy_purchases_a_cosmetic_and_deducts_coins() {
        std::env::set_var("NODE_ENV", "test");
        let (state, _dir) = test_state();
        let email = "buyer@student.rjuhsd.us";
        mitch_lib::coins::add_coins(&state.store, state.data_dir(), email, 2000.0, 1.0, "test_init");
        let headers = auth_headers(&state, email);

        let resp = shop_buy(
            &state,
            &headers,
            &json!({ "itemId": "chess_badge" }),
            br#"{"itemId":"chess_badge"}"#,
        )
        .await;
        assert_eq!(resp.status(), 200);
        let data = body_json(resp).await;
        assert_eq!(data["ok"], json!(true));
        assert_eq!(data["cost"], json!(1775));

        let balance = mitch_lib::coins::get_coins(&state.store, state.data_dir(), email);
        assert_eq!(balance, 225.0);

        let norm = auth::normalize_email(email);
        let cosm = state
            .store
            .read_document(&data_file(&state, "cosmetics.json"), json!({}));
        let badges = cosm[&norm]["badges"].as_array().cloned().unwrap_or_default();
        assert!(badges.contains(&json!("chess_badge")));

        // Buying it again is rejected, not double-charged.
        let resp2 = shop_buy(
            &state,
            &headers,
            &json!({ "itemId": "chess_badge" }),
            br#"{"itemId":"chess_badge"}"#,
        )
        .await;
        assert_eq!(resp2.status(), 400);
        let balance2 = mitch_lib::coins::get_coins(&state.store, state.data_dir(), email);
        assert_eq!(balance2, 225.0);
    }

    #[tokio::test]
    async fn shop_buy_rejects_insufficient_balance_without_charging() {
        std::env::set_var("NODE_ENV", "test");
        let (state, _dir) = test_state();
        let email = "poor@student.rjuhsd.us";
        mitch_lib::coins::add_coins(&state.store, state.data_dir(), email, 10.0, 1.0, "test_init");
        let headers = auth_headers(&state, email);

        let resp = shop_buy(
            &state,
            &headers,
            &json!({ "itemId": "chess_badge" }),
            br#"{"itemId":"chess_badge"}"#,
        )
        .await;
        assert_eq!(resp.status(), 400);
        let data = body_json(resp).await;
        assert!(data["error"].as_str().unwrap().contains("Insufficient coins"));
        let balance = mitch_lib::coins::get_coins(&state.store, state.data_dir(), email);
        assert_eq!(balance, 10.0, "a failed purchase must never charge coins");
    }
}
