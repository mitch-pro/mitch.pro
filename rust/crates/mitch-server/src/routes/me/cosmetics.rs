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
    if path == "/api/coins/buy-premium" && *method == Method::POST {
        return Some(buy_premium(state, headers, body, body_bytes).await);
    }
    if path == "/api/coins/gift-premium" && *method == Method::POST {
        return Some(gift_premium(state, headers, body, body_bytes).await);
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
    let items = mitch_lib::shop::shop_items_for(&state.store, &catalog, &email);
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
    let disabled = item
        .get("disabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
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

    let cost = mitch_lib::shop::shop_cost_for(&state.store, &item, &email);
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
        let entry = mitch_lib::shop::cosmetics_get(&state.store, &norm).unwrap_or(json!({}));
        let mut owned = mitch_lib::shop::sanitize_cosmetics_for_email(&state.store, &email, &entry);
        if mitch_lib::shop::cosmetics_bucket_has(&owned, bucket, &item_id) {
            return json_response(400, json!({ "error": "You already own this item." }));
        }
        if cost > 0
            && mitch_lib::coins::spend_coins(
                &state.store,
                state.data_dir(),
                &email,
                cost as f64,
                "shop_purchase",
            )
            .is_err()
        {
            return json_response(
                400,
                json!({ "error": format!("Insufficient coins. Need {cost}.") }),
            );
        }
        if let Some(arr) = owned.get_mut(bucket).and_then(|v| v.as_array_mut()) {
            arr.push(json!(item_id));
        }
        mitch_lib::shop::cosmetics_set(&state.store, &norm, &owned);
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
            return json_response(
                400,
                json!({ "error": "You already unlocked this personality." }),
            );
        }
        if cost > 0
            && mitch_lib::coins::spend_coins(
                &state.store,
                state.data_dir(),
                &email,
                cost as f64,
                "shop_purchase",
            )
            .is_err()
        {
            return json_response(
                400,
                json!({ "error": format!("Insufficient coins. Need {cost}.") }),
            );
        }
        mine.push(item_id.clone());
        if let Some(obj) = unlocked.as_object_mut() {
            obj.insert(norm.clone(), json!(mine));
        }
        let _ = state.store.write_document(&unlocked_file, &unlocked);
    } else if cost_type == "streak_freeze" {
        let mut entry =
            mitch_lib::daily_logins::daily_login_get(&state.store, &norm).unwrap_or(json!({
                "lastClaimDate": "", "streak": 0, "streakFreezes": 0
            }));
        let freezes = entry
            .get("streakFreezes")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        if freezes >= 3 {
            return json_response(
                400,
                json!({ "error": "You can only hold a maximum of 3 Streak Freezes." }),
            );
        }
        if cost > 0
            && mitch_lib::coins::spend_coins(
                &state.store,
                state.data_dir(),
                &email,
                cost as f64,
                "shop_purchase",
            )
            .is_err()
        {
            return json_response(
                400,
                json!({ "error": format!("Insufficient coins. Need {cost}.") }),
            );
        }
        if let Some(obj) = entry.as_object_mut() {
            obj.insert("streakFreezes".to_string(), json!(freezes + 1));
        }
        mitch_lib::daily_logins::daily_login_set(&state.store, &norm, &entry);
    } else if cost > 0
        && mitch_lib::coins::spend_coins(
            &state.store,
            state.data_dir(),
            &email,
            cost as f64,
            "shop_purchase",
        )
        .is_err()
    {
        return json_response(
            400,
            json!({ "error": format!("Insufficient coins. Need {cost}.") }),
        );
    }
    json_response(
        200,
        json!({ "ok": true, "message": "Purchase successful!", "cost": cost }),
    )
}

/// `POST /api/coins/buy-premium` — the frontend's dedicated purchase path
/// for the Premium shop item itself (the generic `/api/shop/buy` above
/// explicitly rejects `type: "premium"` items and the frontend branches to
/// this endpoint instead for exactly that item - see shop/index.html's
/// `buyPremium()`). This endpoint didn't exist in Rust at all, so every
/// purchase 404'd regardless of balance.
async fn buy_premium(
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

    if auth::is_premium_email(&state.store, &email) {
        return json_response(400, json!({ "error": "You already have Premium." }));
    }

    let catalog = mitch_lib::shop::load_shop_catalog(&state.store, state.data_dir());
    let Some(item) = mitch_lib::shop::shop_item_by_id(&catalog, "premium").cloned() else {
        return json_response(
            400,
            json!({ "error": "Premium is not currently available." }),
        );
    };
    let cost = mitch_lib::shop::shop_cost_for(&state.store, &item, &email);
    let balance = mitch_lib::coins::get_coins(&state.store, state.data_dir(), &email);
    if balance < cost as f64 {
        return json_response(
            400,
            json!({ "error": format!("Insufficient coins. Need {cost}, have {balance:.2}.") }),
        );
    }

    if cost > 0
        && mitch_lib::coins::spend_coins(
            &state.store,
            state.data_dir(),
            &email,
            cost as f64,
            "premium_purchase",
        )
        .is_err()
    {
        return json_response(
            400,
            json!({ "error": format!("Insufficient coins. Need {cost}.") }),
        );
    }
    crate::routes::admin::economy::grant_premium_purchase(
        state,
        &email,
        &format!("Purchased with {cost} coins"),
    );

    json_response(
        200,
        json!({ "ok": true, "message": "Welcome to Premium!", "cost": cost }),
    )
}

/// `POST /api/coins/gift-premium` — same missing-endpoint bug as
/// buy-premium, for shop/index.html's `giftPremium()`. One gift per account,
/// ever (`premium_gifts_sent.json`, see `grant_premium_purchase`'s doc
/// comment on why that file name already existed but was unused).
async fn gift_premium(
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
    let Some(gifter_email) = auth::email_from_sid(&state.store, &state.id_secret, &sid) else {
        return json_response(401, json!({ "error": "email not found" }));
    };
    if mitch_lib::tester::is_sandboxed(&state.store, &gifter_email) {
        return json_response(
            403,
            json!({ "error": "Tester accounts can't gift Premium." }),
        );
    }

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

    let target_raw = body
        .get("targetEmail")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if target_raw.is_empty() {
        return json_response(
            400,
            json!({ "error": "Please enter a friend's email address." }),
        );
    }
    let target_norm = auth::normalize_email(&target_raw);
    if target_norm == auth::normalize_email(&gifter_email) {
        return json_response(
            400,
            json!({ "error": "You can't gift Premium to yourself." }),
        );
    }
    if !valid_email_shape(&target_raw) {
        return json_response(
            400,
            json!({ "error": "That doesn't look like a valid email." }),
        );
    }

    if crate::routes::admin::economy::has_gifted_premium(state, &gifter_email) {
        return json_response(
            400,
            json!({ "error": "You've already used your one lifetime Premium gift." }),
        );
    }
    if auth::is_premium_email(&state.store, &target_norm) {
        return json_response(400, json!({ "error": "That account already has Premium." }));
    }

    let catalog = mitch_lib::shop::load_shop_catalog(&state.store, state.data_dir());
    let Some(item) = mitch_lib::shop::shop_item_by_id(&catalog, "premium").cloned() else {
        return json_response(
            400,
            json!({ "error": "Premium is not currently available." }),
        );
    };
    let cost = mitch_lib::shop::shop_cost_for(&state.store, &item, &gifter_email);
    let balance = mitch_lib::coins::get_coins(&state.store, state.data_dir(), &gifter_email);
    if balance < cost as f64 {
        return json_response(
            400,
            json!({ "error": format!("Insufficient coins. Need {cost}, have {balance:.2}.") }),
        );
    }

    if cost > 0
        && mitch_lib::coins::spend_coins(
            &state.store,
            state.data_dir(),
            &gifter_email,
            cost as f64,
            "premium_gift",
        )
        .is_err()
    {
        return json_response(
            400,
            json!({ "error": format!("Insufficient coins. Need {cost}.") }),
        );
    }
    crate::routes::admin::economy::mark_gifted_premium(state, &gifter_email);
    crate::routes::admin::economy::grant_premium_purchase(
        state,
        &target_raw,
        &format!("Gifted by {gifter_email}"),
    );

    json_response(
        200,
        json!({ "ok": true, "message": "Premium gifted successfully!", "cost": cost }),
    )
}

fn valid_email_shape(email: &str) -> bool {
    let mut parts = email.split('@');
    let local = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("");
    !local.is_empty() && !rest.is_empty() && rest.contains('.') && parts.next().is_none()
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

    let entry = mitch_lib::shop::cosmetics_get(&state.store, &norm).unwrap_or(json!({}));
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

    mitch_lib::shop::cosmetics_set(&state.store, &norm, &user_cosm);
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
        mitch_lib::coins::add_coins(
            &state.store,
            state.data_dir(),
            email,
            2000.0,
            1.0,
            "test_init",
        );
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
        let cosm = mitch_lib::shop::cosmetics_get(&state.store, &norm).unwrap_or(json!({}));
        let badges = cosm["badges"].as_array().cloned().unwrap_or_default();
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
        mitch_lib::coins::add_coins(
            &state.store,
            state.data_dir(),
            email,
            10.0,
            1.0,
            "test_init",
        );
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
        assert!(data["error"]
            .as_str()
            .unwrap()
            .contains("Insufficient coins"));
        let balance = mitch_lib::coins::get_coins(&state.store, state.data_dir(), email);
        assert_eq!(balance, 10.0, "a failed purchase must never charge coins");
    }
}
