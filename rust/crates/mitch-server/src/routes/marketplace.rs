//! Player Marketplace endpoints (server.js:12040-12390) + auto-finalize sweep (server.js:3283-3328).
//!
//! - `GET /api/marketplace/items` — active listings, user inventory, user coins
//! - `POST /api/marketplace/list` — list a cosmetic or text service
//! - `POST /api/marketplace/buy` — purchase listing (immediate or via mediator escrow)
//! - `POST /api/marketplace/cancel` — seller cancels active listing
//! - `POST /api/marketplace/mediate` — mediator resolves or undos escrow transaction
//! - `POST /api/marketplace/appeal` — buyer/seller appeals to moderator chat reports

use super::me::{cookies_of, data_file, json_response, me_uid, parse_body_strict};
use super::push::{ntfy_notify, send_email_bg, verify_recaptcha};
use crate::handler::get_real_ip;
use crate::routes::admin::legacy::html_base_template;
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::{admin, auth, coins, jsval, shop};
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Option<Response> {
    match (method.as_str(), path) {
        ("GET", "/api/marketplace/items") => Some(marketplace_items(state, headers)),
        ("POST", "/api/marketplace/list") => {
            Some(marketplace_list(state, headers, body_bytes).await)
        }
        ("POST", "/api/marketplace/buy") => Some(marketplace_buy(state, headers, body_bytes).await),
        ("POST", "/api/marketplace/cancel") => Some(marketplace_cancel(state, headers, body_bytes)),
        ("POST", "/api/marketplace/mediate") => {
            Some(marketplace_mediate(state, headers, body_bytes))
        }
        ("POST", "/api/marketplace/appeal") => Some(marketplace_appeal(state, headers, body_bytes)),
        _ => None,
    }
}

/// `autoFinalizeMarketplace` (server.js:3283-3328): sweeps pending listings older than 24 hours.
pub fn auto_finalize_marketplace(state: &AppState) {
    let file = data_file(state, "marketplace.json");
    let mut list = state.store.read_document(&file, json!([]));
    let Some(arr) = list.as_array_mut() else {
        return;
    };
    let mut modified = false;
    let now = mitch_lib::school::now_millis();
    let expiry_window = 24 * 3600 * 1000;

    let catalog = shop::load_shop_catalog(&state.store, state.data_dir());

    for item in arr.iter_mut() {
        let status = item.get("status").and_then(|v| v.as_str()).unwrap_or("");
        let bought_at = item.get("bought_at").and_then(jsval::number).unwrap_or(0.0) as i64;
        if status == "pending" && bought_at > 0 && (now - bought_at > expiry_window) {
            item["status"] = json!("finalized");
            item["finalized_at"] = json!(now);
            item["updated_at"] = json!(now);
            modified = true;

            let price = item.get("price").and_then(jsval::number).unwrap_or(0.0);
            let seller = item.get("seller").and_then(|v| v.as_str()).unwrap_or("");
            if !seller.is_empty() {
                coins::add_coins(
                    &state.store,
                    state.data_dir(),
                    seller,
                    price,
                    1.0,
                    "marketplace_sale",
                );
            }

            let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let item_id = item.get("itemId").and_then(|v| v.as_str()).unwrap_or("");
            let buyer = item.get("buyer").and_then(|v| v.as_str()).unwrap_or("");
            if item_type == "cosmetic" && !item_id.is_empty() && !buyer.is_empty() {
                if let Some(shop_item) = catalog
                    .iter()
                    .find(|i| i.get("id").and_then(|v| v.as_str()) == Some(item_id))
                {
                    let cost_type = shop_item
                        .get("costType")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if let Some((bucket, _)) = shop::shop_type_config(cost_type) {
                        let buyer_norm = auth::normalize_email(buyer);
                        let mut owned = shop::normalize_cosmetics(
                            &shop::cosmetics_get(&state.store, &buyer_norm).unwrap_or(json!({})),
                        );
                        if let Some(bucket_arr) =
                            owned.get_mut(bucket).and_then(|v| v.as_array_mut())
                        {
                            if !bucket_arr.iter().any(|v| v.as_str() == Some(item_id)) {
                                bucket_arr.push(json!(item_id));
                            }
                        }
                        shop::cosmetics_set(&state.store, &buyer_norm, &owned);
                    }
                }
            }
        }
    }

    if modified {
        let _ = state.store.write_document(&file, &list);
    }
}

/// `GET /api/marketplace/items` (server.js:12040-12071).
fn marketplace_items(state: &Arc<AppState>, headers: &HeaderMap) -> Response {
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    if !auth::valid_id(&sid, &state.id_secret) {
        return json_response(401, json!({ "error": "not logged in" }));
    }
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &sid) else {
        return json_response(401, json!({ "error": "email not found" }));
    };
    auto_finalize_marketplace(state);

    let listings = state
        .store
        .read_document(&data_file(state, "marketplace.json"), json!([]));
    let catalog = shop::load_shop_catalog(&state.store, state.data_dir());
    let inventory = shop::build_inventory(&state.store, state.data_dir(), &catalog, &email);
    let user_coins = coins::get_coins(&state.store, state.data_dir(), &email);
    let my_norm = auth::normalize_email(&email);

    let formatted: Vec<Value> = listings
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|l| {
            let seller = l.get("seller").and_then(|v| v.as_str()).unwrap_or("");
            let buyer = l.get("buyer").and_then(|v| v.as_str());
            let mediator = l.get("mediator").and_then(|v| v.as_str());
            json!({
                "id": l.get("id").cloned().unwrap_or(Value::Null),
                "seller": admin::mask_email(seller),
                "type": l.get("type").cloned().unwrap_or(Value::Null),
                "itemId": l.get("itemId").cloned().unwrap_or(Value::Null),
                "description": l.get("description").cloned().unwrap_or(Value::Null),
                "price": l.get("price").cloned().unwrap_or(Value::Null),
                "mediator": mediator.map(admin::mask_email).map(|s| json!(s)).unwrap_or(Value::Null),
                "status": l.get("status").cloned().unwrap_or(Value::Null),
                "buyer": buyer.map(admin::mask_email).map(|s| json!(s)).unwrap_or(Value::Null),
                "created_at": l.get("created_at").cloned().unwrap_or(Value::Null),
                "bought_at": l.get("bought_at").cloned().unwrap_or(Value::Null),
                "isSeller": auth::normalize_email(seller) == my_norm,
                "isBuyer": buyer.map(|b| auth::normalize_email(b) == my_norm).unwrap_or(false),
                "isMediator": mediator.map(|m| auth::normalize_email(m) == my_norm).unwrap_or(false),
            })
        })
        .collect();

    json_response(
        200,
        json!({ "listings": formatted, "inventory": inventory, "coins": user_coins }),
    )
}

/// `POST /api/marketplace/list` (server.js:12073-12152).
async fn marketplace_list(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Response {
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    if !auth::valid_id(&sid, &state.id_secret) {
        return json_response(401, json!({ "error": "not logged in" }));
    }
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &sid) else {
        return json_response(401, json!({ "error": "email not found" }));
    };
    let ip = get_real_ip(headers, None);
    let token = body
        .get("recaptcha_token")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !verify_recaptcha(state, token, &ip, &sid).await {
        return json_response(
            400,
            json!({ "error": "reCAPTCHA failed. Please try again." }),
        );
    }
    let norm = auth::normalize_email(&email);
    let mp_file = data_file(state, "marketplace.json");
    let mut listings = state.store.read_document(&mp_file, json!([]));
    let active_count = listings
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter(|l| {
            let s = l.get("seller").and_then(|v| v.as_str()).unwrap_or("");
            let st = l.get("status").and_then(|v| v.as_str()).unwrap_or("");
            auth::normalize_email(s) == norm && st == "active"
        })
        .count();
    if active_count >= 10 {
        return json_response(
            400,
            json!({ "error": "You cannot have more than 10 active listings on the marketplace." }),
        );
    }

    let list_type = body.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if list_type != "cosmetic" && list_type != "text" {
        return json_response(400, json!({ "error": "invalid listing type" }));
    }
    let price = body.get("price").and_then(jsval::number).unwrap_or(0.0);
    if !price.is_finite() || price <= 0.0 || price > 1_000_000_000.0 {
        return json_response(400, json!({ "error": "price must be a positive number" }));
    }

    let mut mediator_email: Option<String> = None;
    if let Some(med) = body.get("mediator").and_then(|v| v.as_str()) {
        let trimmed = med.trim().to_lowercase();
        if !trimmed.is_empty() {
            if trimmed == norm {
                return json_response(400, json!({ "error": "you cannot mediate your own trade" }));
            }
            mediator_email = Some(trimmed);
        }
    }

    let item_id = body.get("itemId").and_then(|v| v.as_str()).unwrap_or("");
    let description = body
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();

    if list_type == "cosmetic" {
        let catalog = shop::load_shop_catalog(&state.store, state.data_dir());
        let Some(item) = catalog
            .iter()
            .find(|i| i.get("id").and_then(|v| v.as_str()) == Some(item_id))
        else {
            return json_response(400, json!({ "error": "invalid cosmetic item" }));
        };
        let cost_type = item.get("costType").and_then(|v| v.as_str()).unwrap_or("");
        let Some((bucket, active_key)) = shop::shop_type_config(cost_type) else {
            return json_response(400, json!({ "error": "untradeable cosmetic item type" }));
        };
        let mut owned =
            shop::normalize_cosmetics(&shop::cosmetics_get(&state.store, &norm).unwrap_or(json!({})));
        let has_item = owned
            .get(bucket)
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().any(|x| x.as_str() == Some(item_id)))
            .unwrap_or(false);
        if !has_item {
            return json_response(
                403,
                json!({ "error": "You do not own this cosmetic item!" }),
            );
        }

        // Strip item from seller
        if let Some(arr) = owned.get_mut(bucket).and_then(|v| v.as_array_mut()) {
            arr.retain(|x| x.as_str() != Some(item_id));
        }
        if owned.get(active_key).and_then(|v| v.as_str()) == Some(item_id) {
            owned[active_key] = json!("");
        }
        shop::cosmetics_set(&state.store, &norm, &owned);
    } else if description.is_empty() {
        return json_response(
            400,
            json!({ "error": "text listings require a description" }),
        );
    }

    let id = mitch_lib::crypto::random_bytes_hex(6);
    let now = mitch_lib::school::now_millis();
    let desc_clipped = &description[..description.len().min(500)];
    let new_listing = json!({
        "id": id,
        "seller": email,
        "type": list_type,
        "itemId": if list_type == "cosmetic" { json!(item_id) } else { Value::Null },
        "description": desc_clipped,
        "price": price,
        "mediator": mediator_email.map(|m| json!(m)).unwrap_or(Value::Null),
        "status": "active",
        "buyer": Value::Null,
        "created_at": now,
        "bought_at": Value::Null,
        "finalized_at": Value::Null,
        "updated_at": now,
    });

    if let Some(arr) = listings.as_array_mut() {
        arr.push(new_listing);
    }
    let _ = state.store.write_document(&mp_file, &listings);
    json_response(
        200,
        json!({ "ok": true, "message": "Listing created successfully!" }),
    )
}

/// `POST /api/marketplace/buy` (server.js:12154-12228).
async fn marketplace_buy(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Response {
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    if !auth::valid_id(&sid, &state.id_secret) {
        return json_response(401, json!({ "error": "not logged in" }));
    }
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &sid) else {
        return json_response(401, json!({ "error": "email not found" }));
    };
    let ip = get_real_ip(headers, None);
    let token = body
        .get("recaptcha_token")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !verify_recaptcha(state, token, &ip, &sid).await {
        return json_response(
            400,
            json!({ "error": "reCAPTCHA failed. Please try again." }),
        );
    }
    let norm = auth::normalize_email(&email);
    let listing_id = body.get("listingId").and_then(|v| v.as_str()).unwrap_or("");

    let mp_file = data_file(state, "marketplace.json");
    let mut listings = state.store.read_document(&mp_file, json!([]));
    let Some(arr) = listings.as_array_mut() else {
        return json_response(404, json!({ "error": "listing not found" }));
    };
    let Some(pos) = arr
        .iter()
        .position(|l| l.get("id").and_then(|v| v.as_str()) == Some(listing_id))
    else {
        return json_response(404, json!({ "error": "listing not found" }));
    };

    let listing = &mut arr[pos];
    let status = listing.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if status != "active" {
        return json_response(400, json!({ "error": "listing is no longer active" }));
    }
    let seller = listing
        .get("seller")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if auth::normalize_email(&seller) == norm {
        return json_response(400, json!({ "error": "you cannot buy your own listing" }));
    }
    let mediator = listing
        .get("mediator")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    if let Some(ref m) = mediator {
        if auth::normalize_email(m) == norm {
            return json_response(
                400,
                json!({ "error": "mediators cannot purchase the listings they mediate" }),
            );
        }
    }

    let price = listing.get("price").and_then(jsval::number).unwrap_or(0.0);
    let balance = coins::get_coins(&state.store, state.data_dir(), &email);
    if balance < price {
        return json_response(
            400,
            json!({ "error": format!("insufficient coins. Need {price}, have {:.2}.", balance) }),
        );
    }

    // Deduct coins immediately from buyer
    coins::add_coins(
        &state.store,
        state.data_dir(),
        &email,
        -price,
        1.0,
        "marketplace_buy",
    );

    let item_type = listing
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let item_id = listing
        .get("itemId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let item_desc_str = listing
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let now = mitch_lib::school::now_millis();
    listing["buyer"] = json!(email);
    listing["bought_at"] = json!(now);
    listing["updated_at"] = json!(now);

    if let Some(med) = mediator {
        listing["status"] = json!("pending");
        let _ = state.store.write_document(&mp_file, &listings);

        let email_subject = "Mitch.pro Marketplace — You are a mediator!";
        let item_desc = if item_type == "cosmetic" {
            item_id.clone()
        } else {
            format!("Custom: {item_desc_str}")
        };
        let m_url = "https://mitchdog.com/marketplace/";
        let html = make_mediator_escrow_html(
            state,
            &med,
            &admin::mask_email(&seller),
            &admin::mask_email(&email),
            price,
            &item_desc,
            m_url,
        );
        send_email_bg(state, &med, email_subject, &html);

        return json_response(
            200,
            json!({ "ok": true, "message": "Purchase placed in mediator escrow successfully!" }),
        );
    }

    // Finalize immediately
    listing["status"] = json!("finalized");
    listing["finalized_at"] = json!(now);
    let _ = state.store.write_document(&mp_file, &listings);

    // 1. Transfer coins to seller
    coins::add_coins(
        &state.store,
        state.data_dir(),
        &seller,
        price,
        1.0,
        "marketplace_sale",
    );

    // 2. Award item to buyer if cosmetic
    if item_type == "cosmetic" && !item_id.is_empty() {
        let catalog = shop::load_shop_catalog(&state.store, state.data_dir());
        if let Some(shop_item) = catalog
            .iter()
            .find(|i| i.get("id").and_then(|v| v.as_str()) == Some(&item_id))
        {
            let cost_type = shop_item
                .get("costType")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if let Some((bucket, _)) = shop::shop_type_config(cost_type) {
                let mut owned = shop::normalize_cosmetics(
                    &shop::cosmetics_get(&state.store, &norm).unwrap_or(json!({})),
                );
                if let Some(arr) = owned.get_mut(bucket).and_then(|v| v.as_array_mut()) {
                    if !arr.iter().any(|v| v.as_str() == Some(&item_id)) {
                        arr.push(json!(item_id));
                    }
                }
                shop::cosmetics_set(&state.store, &norm, &owned);
            }
        }
    }

    json_response(
        200,
        json!({ "ok": true, "message": "Purchase finalized successfully!" }),
    )
}

/// `POST /api/marketplace/cancel` (server.js:12230-12258).
fn marketplace_cancel(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    if !auth::valid_id(&sid, &state.id_secret) {
        return json_response(401, json!({ "error": "not logged in" }));
    }
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &sid) else {
        return json_response(401, json!({ "error": "email not found" }));
    };
    let listing_id = body.get("listingId").and_then(|v| v.as_str()).unwrap_or("");
    let mp_file = data_file(state, "marketplace.json");
    let mut listings = state.store.read_document(&mp_file, json!([]));
    let Some(arr) = listings.as_array_mut() else {
        return json_response(404, json!({ "error": "listing not found" }));
    };
    let Some(pos) = arr
        .iter()
        .position(|l| l.get("id").and_then(|v| v.as_str()) == Some(listing_id))
    else {
        return json_response(404, json!({ "error": "listing not found" }));
    };

    let listing = &mut arr[pos];
    let seller = listing.get("seller").and_then(|v| v.as_str()).unwrap_or("");
    if auth::normalize_email(seller) != auth::normalize_email(&email) {
        return json_response(
            403,
            json!({ "error": "only the seller can cancel this listing" }),
        );
    }
    let status = listing.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if status != "active" {
        return json_response(
            400,
            json!({ "error": "only active listings can be cancelled" }),
        );
    }

    let item_type = listing
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let item_id = listing
        .get("itemId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if item_type == "cosmetic" && !item_id.is_empty() {
        let catalog = shop::load_shop_catalog(&state.store, state.data_dir());
        if let Some(shop_item) = catalog
            .iter()
            .find(|i| i.get("id").and_then(|v| v.as_str()) == Some(&item_id))
        {
            let cost_type = shop_item
                .get("costType")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if let Some((bucket, _)) = shop::shop_type_config(cost_type) {
                let norm = auth::normalize_email(&email);
                let mut owned = shop::normalize_cosmetics(
                    &shop::cosmetics_get(&state.store, &norm).unwrap_or(json!({})),
                );
                if let Some(arr) = owned.get_mut(bucket).and_then(|v| v.as_array_mut()) {
                    if !arr.iter().any(|v| v.as_str() == Some(&item_id)) {
                        arr.push(json!(item_id));
                    }
                }
                shop::cosmetics_set(&state.store, &norm, &owned);
            }
        }
    }

    listing["status"] = json!("cancelled");
    listing["updated_at"] = json!(mitch_lib::school::now_millis());
    let _ = state.store.write_document(&mp_file, &listings);

    json_response(
        200,
        json!({ "ok": true, "message": "Listing cancelled and item returned." }),
    )
}

/// `POST /api/marketplace/mediate` (server.js:12260-12338).
fn marketplace_mediate(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    if !auth::valid_id(&sid, &state.id_secret) {
        return json_response(401, json!({ "error": "not logged in" }));
    }
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &sid) else {
        return json_response(401, json!({ "error": "email not found" }));
    };
    let norm = auth::normalize_email(&email);
    let listing_id = body.get("listingId").and_then(|v| v.as_str()).unwrap_or("");
    let action = body.get("action").and_then(|v| v.as_str()).unwrap_or("");

    let mp_file = data_file(state, "marketplace.json");
    let mut listings = state.store.read_document(&mp_file, json!([]));
    let Some(arr) = listings.as_array_mut() else {
        return json_response(404, json!({ "error": "listing not found" }));
    };
    let Some(pos) = arr
        .iter()
        .position(|l| l.get("id").and_then(|v| v.as_str()) == Some(listing_id))
    else {
        return json_response(404, json!({ "error": "listing not found" }));
    };

    let listing = &mut arr[pos];
    let status = listing.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if status != "pending" {
        return json_response(
            400,
            json!({ "error": "this listing is not pending mediation" }),
        );
    }
    let mediator = listing
        .get("mediator")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if auth::normalize_email(mediator) != norm {
        return json_response(
            403,
            json!({ "error": "you are not authorized as mediator for this trade" }),
        );
    }

    let price = listing.get("price").and_then(jsval::number).unwrap_or(0.0);
    let seller = listing
        .get("seller")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let buyer = listing
        .get("buyer")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let item_type = listing
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let item_id = listing
        .get("itemId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let now = mitch_lib::school::now_millis();

    if action == "resolve" {
        listing["status"] = json!("finalized");
        listing["finalized_at"] = json!(now);
        listing["updated_at"] = json!(now);
        let _ = state.store.write_document(&mp_file, &listings);

        coins::add_coins(
            &state.store,
            state.data_dir(),
            &seller,
            price,
            1.0,
            "marketplace_sale",
        );

        if item_type == "cosmetic" && !item_id.is_empty() && !buyer.is_empty() {
            let catalog = shop::load_shop_catalog(&state.store, state.data_dir());
            if let Some(shop_item) = catalog
                .iter()
                .find(|i| i.get("id").and_then(|v| v.as_str()) == Some(&item_id))
            {
                let cost_type = shop_item
                    .get("costType")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if let Some((bucket, _)) = shop::shop_type_config(cost_type) {
                    let buyer_norm = auth::normalize_email(&buyer);
                    let mut owned = shop::normalize_cosmetics(
                        &shop::cosmetics_get(&state.store, &buyer_norm).unwrap_or(json!({})),
                    );
                    if let Some(arr) = owned.get_mut(bucket).and_then(|v| v.as_array_mut()) {
                        if !arr.iter().any(|v| v.as_str() == Some(&item_id)) {
                            arr.push(json!(item_id));
                        }
                    }
                    shop::cosmetics_set(&state.store, &buyer_norm, &owned);
                }
            }
        }
        json_response(
            200,
            json!({ "ok": true, "message": "Transaction resolved and finalized!" }),
        )
    } else if action == "undo" {
        listing["status"] = json!("undone");
        listing["updated_at"] = json!(now);
        let _ = state.store.write_document(&mp_file, &listings);

        coins::add_coins(
            &state.store,
            state.data_dir(),
            &buyer,
            price,
            1.0,
            "marketplace_refund",
        );

        if item_type == "cosmetic" && !item_id.is_empty() && !seller.is_empty() {
            let catalog = shop::load_shop_catalog(&state.store, state.data_dir());
            if let Some(shop_item) = catalog
                .iter()
                .find(|i| i.get("id").and_then(|v| v.as_str()) == Some(&item_id))
            {
                let cost_type = shop_item
                    .get("costType")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if let Some((bucket, _)) = shop::shop_type_config(cost_type) {
                    let seller_norm = auth::normalize_email(&seller);
                    let mut owned = shop::normalize_cosmetics(
                        &shop::cosmetics_get(&state.store, &seller_norm).unwrap_or(json!({})),
                    );
                    if let Some(arr) = owned.get_mut(bucket).and_then(|v| v.as_array_mut()) {
                        if !arr.iter().any(|v| v.as_str() == Some(&item_id)) {
                            arr.push(json!(item_id));
                        }
                    }
                    shop::cosmetics_set(&state.store, &seller_norm, &owned);
                }
            }
        }
        json_response(
            200,
            json!({ "ok": true, "message": "Transaction undone and refunded successfully!" }),
        )
    } else {
        json_response(400, json!({ "error": "invalid mediation action" }))
    }
}

/// `POST /api/marketplace/appeal` (server.js:12340-12390).
fn marketplace_appeal(state: &Arc<AppState>, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let Some(body) = parse_body_strict(body_bytes) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    if !auth::valid_id(&sid, &state.id_secret) {
        return json_response(401, json!({ "error": "not logged in" }));
    }
    let Some(email) = auth::email_from_sid(&state.store, &state.id_secret, &sid) else {
        return json_response(401, json!({ "error": "email not found" }));
    };
    let norm = auth::normalize_email(&email);
    let listing_id = body.get("listingId").and_then(|v| v.as_str()).unwrap_or("");
    let reason = body.get("reason").and_then(|v| v.as_str()).unwrap_or("");

    let mp_file = data_file(state, "marketplace.json");
    let mut listings = state.store.read_document(&mp_file, json!([]));
    let Some(arr) = listings.as_array_mut() else {
        return json_response(404, json!({ "error": "listing not found" }));
    };
    let Some(pos) = arr
        .iter()
        .position(|l| l.get("id").and_then(|v| v.as_str()) == Some(listing_id))
    else {
        return json_response(404, json!({ "error": "listing not found" }));
    };

    let listing = &mut arr[pos];
    let seller = listing
        .get("seller")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let buyer = listing
        .get("buyer")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let is_seller = auth::normalize_email(&seller) == norm;
    let is_buyer = !buyer.is_empty() && auth::normalize_email(&buyer) == norm;
    if !is_seller && !is_buyer {
        return json_response(
            403,
            json!({ "error": "only the buyer or seller can appeal this transaction" }),
        );
    }

    let item_type = listing
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let item_desc = if item_type == "cosmetic" {
        listing
            .get("itemId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    } else {
        format!(
            "Text: {}",
            listing
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
        )
    };
    let price = listing.get("price").and_then(jsval::number).unwrap_or(0.0);
    let mediator = listing
        .get("mediator")
        .and_then(|v| v.as_str())
        .unwrap_or("None")
        .to_string();

    let now = mitch_lib::school::now_millis();
    listing["status"] = json!("disputed");
    listing["updated_at"] = json!(now);
    let _ = state.store.write_document(&mp_file, &listings);

    // Create moderator chat report
    let reports_file = data_file(state, "chat_reports.json");
    let mut reports = state.store.read_document(&reports_file, json!([]));

    let reason_clipped = &reason[..reason.len().min(500)];
    let new_report = json!({
        "id": format!("appeal-{}", listing_id),
        "reason": format!("Marketplace Dispute Appeal by {email}: {reason_clipped}"),
        "reportedBy": email,
        "ts": now,
        "status": "Needs review",
        "context": [
            {
                "from": "system",
                "to": "admin",
                "text": format!("Listing ID: {listing_id} | Seller: {seller} | Buyer: {buyer} | Item: {item_desc} | Price: {price} coins | Mediator: {mediator} | Status: disputed. Reason for appeal: {reason}"),
                "ts": now,
                "reported": true
            }
        ]
    });

    if let Some(rarr) = reports.as_array_mut() {
        rarr.push(new_report);
        if rarr.len() > 5000 {
            rarr.drain(0..(rarr.len() - 5000));
        }
    }
    let _ = state.store.write_document(&reports_file, &reports);

    ntfy_notify(
        &format!("Marketplace dispute appealed by {email} for listing {listing_id}"),
        "Security",
        "high",
    );

    json_response(
        200,
        json!({ "ok": true, "message": "Transaction appealed to moderators successfully!" }),
    )
}

fn make_mediator_escrow_html(
    state: &AppState,
    email: &str,
    seller: &str,
    buyer: &str,
    price: f64,
    item: &str,
    marketplace_url: &str,
) -> String {
    let content = format!(
        r#"
    <h2 style="margin: 0 0 16px; font-size: 20px; font-weight: 700; color: #fbbf24; text-align: center;">⚖️ Marketplace Mediation</h2>
    <div style="background-color: rgba(251, 191, 36, 0.08); border: 1px solid rgba(251, 191, 36, 0.25); border-radius: 12px; padding: 20px; margin-bottom: 24px;">
      <p style="margin: 0 0 12px; font-weight: 700; color: #f4f4f5; text-align: center;">You have been selected as a mediator!</p>
      <table border="0" cellpadding="0" cellspacing="0" width="100%" style="font-size: 14px; color: #cbd5e1; line-height: 1.8;">
        <tr><td><strong>Seller:</strong></td><td>{seller}</td></tr>
        <tr><td><strong>Buyer:</strong></td><td>{buyer}</td></tr>
        <tr><td><strong>Price:</strong></td><td>{price} MitchCoins</td></tr>
        <tr><td><strong>Item:</strong></td><td>{item}</td></tr>
      </table>
      <p style="margin: 16px 0 0; font-size: 13px; color: #ef4444; text-align: center;">🚨 Please resolve or undo this deal within 24 hours. Otherwise, it will auto-finalize.</p>
    </div>
    <div style="text-align: center; margin-bottom: 8px;">
      <a href="{marketplace_url}" style="display: inline-block; background-color: #fbbf24; color: #1e1b4b; text-decoration: none; padding: 12px 24px; border-radius: 10px; font-weight: 700; box-shadow: 0 10px 20px rgba(251, 191, 36, 0.2);">Go to Marketplace</a>
    </div>
        "#
    );
    html_base_template(
        state,
        email,
        "Mitch.pro Marketplace Escrow Mediation",
        &content,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::HeaderValue;

    fn test_state() -> (Arc<AppState>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "mitch-server-marketplace-test-{}",
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

    #[tokio::test]
    async fn marketplace_list_buy_and_items() {
        let (state, _dir) = test_state();
        let seller = "seller@student.rjuhsd.us";
        let buyer = "buyer@student.rjuhsd.us";

        // Give buyer 100 coins
        coins::add_coins(
            &state.store,
            state.data_dir(),
            buyer,
            100.0,
            1.0,
            "test_init",
        );

        // Give seller a cosmetic badge
        let seller_norm = auth::normalize_email(seller);
        shop::cosmetics_set(
            &state.store,
            &seller_norm,
            &json!({
                "badges": ["verified_badge"],
                "colors": [],
                "effects": []
            }),
        );

        let seller_headers = auth_headers(&state, seller);
        let buyer_headers = auth_headers(&state, buyer);

        // 1. Seller lists cosmetic item
        let list_body = json!({
            "type": "cosmetic",
            "itemId": "verified_badge",
            "price": 50.0,
            "recaptcha_token": "test"
        });
        let list_resp = marketplace_list(
            &state,
            &seller_headers,
            &serde_json::to_vec(&list_body).unwrap(),
        )
        .await;
        assert_eq!(list_resp.status(), 200);

        // Verify item was stripped from seller
        let seller_cosm = shop::cosmetics_get(&state.store, &seller_norm).unwrap();
        let seller_badges = seller_cosm
            .get("badges")
            .and_then(|v| v.as_array())
            .unwrap();
        assert!(seller_badges.is_empty());

        // 2. Fetch marketplace items as buyer
        let items_resp = marketplace_items(&state, &buyer_headers);
        assert_eq!(items_resp.status(), 200);
        let items_bytes = to_bytes(items_resp.into_body(), usize::MAX).await.unwrap();
        let items_val: Value = serde_json::from_slice(&items_bytes).unwrap();
        let listings = items_val
            .get("listings")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(listings.len(), 1);
        let listing_id = listings[0]
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        // 3. Buyer purchases listing
        let buy_body = json!({
            "listingId": listing_id,
            "recaptcha_token": "test"
        });
        let buy_resp = marketplace_buy(
            &state,
            &buyer_headers,
            &serde_json::to_vec(&buy_body).unwrap(),
        )
        .await;
        assert_eq!(buy_resp.status(), 200);

        // Verify coins transferred
        let buyer_coins = coins::get_coins(&state.store, state.data_dir(), buyer);
        let seller_coins = coins::get_coins(&state.store, state.data_dir(), seller);
        assert_eq!(buyer_coins, 50.0);
        assert_eq!(seller_coins, 50.0);

        // Verify cosmetic awarded to buyer
        let buyer_norm = auth::normalize_email(buyer);
        let buyer_cosm = shop::cosmetics_get(&state.store, &buyer_norm).unwrap();
        let buyer_badges = buyer_cosm.get("badges").and_then(|v| v.as_array()).unwrap();
        assert!(buyer_badges
            .iter()
            .any(|b| b.as_str() == Some("verified_badge")));
    }

    #[tokio::test]
    async fn marketplace_mediator_escrow_and_resolve() {
        let (state, _dir) = test_state();
        let seller = "seller@student.rjuhsd.us";
        let buyer = "buyer@student.rjuhsd.us";
        let mediator = "mediator@student.rjuhsd.us";

        coins::add_coins(
            &state.store,
            state.data_dir(),
            buyer,
            100.0,
            1.0,
            "test_init",
        );

        let seller_headers = auth_headers(&state, seller);
        let buyer_headers = auth_headers(&state, buyer);
        let mediator_headers = auth_headers(&state, mediator);

        // List text service with mediator
        let list_body = json!({
            "type": "text",
            "description": "Custom Art Commission",
            "price": 40.0,
            "mediator": mediator,
            "recaptcha_token": "test"
        });
        let list_resp = marketplace_list(
            &state,
            &seller_headers,
            &serde_json::to_vec(&list_body).unwrap(),
        )
        .await;
        assert_eq!(list_resp.status(), 200);

        let mp_file = data_file(&state, "marketplace.json");
        let mp = state.store.read_document(&mp_file, json!([]));
        let listing_id = mp.as_array().unwrap()[0]
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        // Buy listing -> placed into escrow
        let buy_body = json!({
            "listingId": listing_id,
            "recaptcha_token": "test"
        });
        let buy_resp = marketplace_buy(
            &state,
            &buyer_headers,
            &serde_json::to_vec(&buy_body).unwrap(),
        )
        .await;
        assert_eq!(buy_resp.status(), 200);

        // Buyer coins deducted, seller hasn't received them yet
        assert_eq!(
            coins::get_coins(&state.store, state.data_dir(), buyer),
            60.0
        );
        assert_eq!(
            coins::get_coins(&state.store, state.data_dir(), seller),
            0.0
        );

        // Mediator resolves trade
        let resolve_body = json!({
            "listingId": listing_id,
            "action": "resolve"
        });
        let med_resp = marketplace_mediate(
            &state,
            &mediator_headers,
            &serde_json::to_vec(&resolve_body).unwrap(),
        );
        assert_eq!(med_resp.status(), 200);

        // Seller now has coins
        assert_eq!(
            coins::get_coins(&state.store, state.data_dir(), seller),
            40.0
        );
    }

    #[tokio::test]
    async fn marketplace_cancel_and_appeal() {
        let (state, _dir) = test_state();
        let seller = "seller@student.rjuhsd.us";
        let seller_headers = auth_headers(&state, seller);

        // 1. Cancel active listing
        let list_body = json!({
            "type": "text",
            "description": "Tutoring Session",
            "price": 20.0,
            "recaptcha_token": "test"
        });
        let _ = marketplace_list(
            &state,
            &seller_headers,
            &serde_json::to_vec(&list_body).unwrap(),
        )
        .await;

        let mp_file = data_file(&state, "marketplace.json");
        let mp = state.store.read_document(&mp_file, json!([]));
        let listing_id = mp.as_array().unwrap()[0]
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        let cancel_body = json!({ "listingId": listing_id });
        let cancel_resp = marketplace_cancel(
            &state,
            &seller_headers,
            &serde_json::to_vec(&cancel_body).unwrap(),
        );
        assert_eq!(cancel_resp.status(), 200);

        // 2. Appeal transaction
        let appeal_body = json!({
            "listingId": listing_id,
            "reason": "Seller was unresponsive"
        });
        let appeal_resp = marketplace_appeal(
            &state,
            &seller_headers,
            &serde_json::to_vec(&appeal_body).unwrap(),
        );
        assert_eq!(appeal_resp.status(), 200);

        // Verify report was written to chat_reports.json
        let reports_file = data_file(&state, "chat_reports.json");
        let reports = state.store.read_document(&reports_file, json!([]));
        let report_arr = reports.as_array().unwrap();
        assert_eq!(report_arr.len(), 1);
        assert_eq!(
            report_arr[0].get("status").and_then(|v| v.as_str()),
            Some("Needs review")
        );
    }

    #[tokio::test]
    async fn marketplace_mediator_undo_refunds_buyer() {
        let (state, _dir) = test_state();
        let seller = "seller@student.rjuhsd.us";
        let buyer = "buyer@student.rjuhsd.us";
        let mediator = "mediator@student.rjuhsd.us";

        let seller_headers = auth_headers(&state, seller);
        let buyer_headers = auth_headers(&state, buyer);
        let mediator_headers = auth_headers(&state, mediator);

        coins::add_coins(&state.store, state.data_dir(), buyer, 100.0, 1.0, "init");

        // List text service with mediator
        let list_body = json!({
            "type": "text",
            "description": "Tutoring Session",
            "price": 40.0,
            "mediator": mediator,
            "recaptcha_token": "test"
        });
        let _ = marketplace_list(
            &state,
            &seller_headers,
            &serde_json::to_vec(&list_body).unwrap(),
        )
        .await;

        let mp_file = data_file(&state, "marketplace.json");
        let mp = state.store.read_document(&mp_file, json!([]));
        let listing_id = mp.as_array().unwrap()[0]
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        // Buy listing -> placed into escrow
        let buy_body = json!({
            "listingId": listing_id,
            "recaptcha_token": "test"
        });
        let buy_resp = marketplace_buy(
            &state,
            &buyer_headers,
            &serde_json::to_vec(&buy_body).unwrap(),
        )
        .await;
        assert_eq!(buy_resp.status(), 200);

        // Buyer coins deducted
        assert_eq!(
            coins::get_coins(&state.store, state.data_dir(), buyer),
            60.0
        );

        // Mediator undos trade
        let undo_body = json!({
            "listingId": listing_id,
            "action": "undo"
        });
        let med_resp = marketplace_mediate(
            &state,
            &mediator_headers,
            &serde_json::to_vec(&undo_body).unwrap(),
        );
        assert_eq!(med_resp.status(), 200);

        // Buyer gets full refund (60 + 40 = 100)
        assert_eq!(
            coins::get_coins(&state.store, state.data_dir(), buyer),
            100.0
        );
        // Seller gets nothing
        assert_eq!(
            coins::get_coins(&state.store, state.data_dir(), seller),
            0.0
        );

        // Listing status is undone
        let mp_after = state.store.read_document(&mp_file, json!([]));
        assert_eq!(
            mp_after.as_array().unwrap()[0]
                .get("status")
                .and_then(|v| v.as_str()),
            Some("undone")
        );
    }

    #[tokio::test]
    async fn test_auto_finalize_sweep() {
        let (state, _dir) = test_state();
        let seller = "seller@student.rjuhsd.us";
        let buyer = "buyer@student.rjuhsd.us";

        let now = mitch_lib::school::now_millis();
        let bought_at = now - (25 * 3600 * 1000); // 25 hours ago

        let mp_file = data_file(&state, "marketplace.json");
        let initial_listing = json!([{
            "id": "mp_test_123",
            "type": "text",
            "description": "Old Tutoring Session",
            "price": 35.0,
            "seller": seller,
            "buyer": buyer,
            "status": "pending",
            "bought_at": bought_at
        }]);
        let _ = state.store.write_document(&mp_file, &initial_listing);

        assert_eq!(
            coins::get_coins(&state.store, state.data_dir(), seller),
            0.0
        );

        // Run sweep
        auto_finalize_marketplace(&state);

        // Verify seller received coins
        assert_eq!(
            coins::get_coins(&state.store, state.data_dir(), seller),
            35.0
        );

        // Verify listing status is now finalized
        let mp_after = state.store.read_document(&mp_file, json!([]));
        let arr = mp_after.as_array().unwrap();
        assert_eq!(
            arr[0].get("status").and_then(|v| v.as_str()),
            Some("finalized")
        );
    }
}
