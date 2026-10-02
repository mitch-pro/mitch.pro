//! Shop catalog + cosmetics (server.js:7208-7520). Byte-compatible port of
//! `SHOP_CATALOG`, `SHOP_TYPE_CONFIG`, the cost/tier/discount helpers, and the
//! cosmetics sanitizers used by `/api/me/*` and the chat surface.
//!
//! `SHOP_CATALOG` starts as `DEFAULT_SHOP_CATALOG` and is overridden at boot
//! by `data/shop_catalog.json` when that file holds a non-empty array
//! (`og_badge` filtered out, server.js:7256-7267).

use crate::auth;
use crate::data::DataStore;
use serde_json::{json, Value};
use std::path::Path;

/// `SHOP_TYPE_CONFIG` (server.js:7269-7276): equip type → (bucket, active key).
pub fn shop_type_config(cost_type: &str) -> Option<(&'static str, &'static str)> {
    match cost_type {
        "name_color" => Some(("colors", "activeColor")),
        "chat_badge" => Some(("badges", "activeBadge")),
        "chat_effect" => Some(("chatEffects", "activeChatEffect")),
        "profile_effect" => Some(("profileEffects", "activeProfileEffect")),
        "site_theme" => Some(("themes", "activeTheme")),
        "canvas_tool" => Some(("tools", "activeTool")),
        _ => None,
    }
}

/// `DEFAULT_SHOP_CATALOG` (server.js:7209-7255), verbatim.
pub fn default_shop_catalog() -> Value {
    json!([
      { "id": "premium", "name": "Premium", "section": "Premium", "type": "premium", "cost": 5000, "desc": "Unlock Premium Chat, 2X typing/logic coin rewards, 2X Clicker/Riches offline gains, larger canvas brushes, exclusive profile frames/badges, and the epic chance to have an arcade game named after you!" },
      { "id": "neon_purple", "name": "Neon Purple Name", "section": "Name Colors", "type": "cosmetic", "costType": "name_color", "cost": 500, "desc": "A bright purple username for chat, profiles, and leaderboards." },
      { "id": "electric_blue", "name": "Electric Blue Name", "section": "Name Colors", "type": "cosmetic", "costType": "name_color", "cost": 500, "desc": "A sharp electric-blue username style." },
      { "id": "mint_flash", "name": "Mint Flash Name", "section": "Name Colors", "type": "cosmetic", "costType": "name_color", "cost": 550, "desc": "A clean mint username with a fresh glow." },
      { "id": "rose_spark", "name": "Rose Spark Name", "section": "Name Colors", "type": "cosmetic", "costType": "name_color", "cost": 550, "desc": "A warm rose username style with a soft highlight." },
      { "id": "ember_red", "name": "Ember Red Name", "section": "Name Colors", "type": "cosmetic", "costType": "name_color", "cost": 650, "desc": "A deep red username with a bolder presence." },
      { "id": "void_white", "name": "Void White Name", "section": "Name Colors", "type": "cosmetic", "costType": "name_color", "cost": 750, "desc": "A high-contrast white username for dark pages." },
      { "id": "gold_glow", "name": "Golden Glow Name", "section": "Name Colors", "type": "cosmetic", "costType": "name_color", "cost": 900, "premiumOnly": true, "desc": "A premium gold username glow." },
      { "id": "rainbow_name", "name": "Rainbow Name", "section": "Name Colors", "type": "cosmetic", "costType": "name_color", "cost": 2500, "adminOnly": true, "desc": "An admin-only animated rainbow username." },
      { "id": "verified_badge", "name": "Verified Badge", "section": "Badges", "type": "cosmetic", "costType": "chat_badge", "cost": 1000, "desc": "Adds a verified check badge beside your name." },
      { "id": "artist_badge", "name": "Artist Badge", "section": "Badges", "type": "cosmetic", "costType": "chat_badge", "cost": 900, "desc": "A badge for canvas builders and pixel artists." },
      { "id": "chess_badge", "name": "Chess Badge", "section": "Badges", "type": "cosmetic", "costType": "chat_badge", "cost": 900, "desc": "A badge for chess regulars." },
      { "id": "builder_badge", "name": "Builder Badge", "section": "Badges", "type": "cosmetic", "costType": "chat_badge", "cost": 950, "desc": "A badge for people who help build the community." },
      { "id": "lucky_badge", "name": "Lucky Badge", "section": "Badges", "type": "cosmetic", "costType": "chat_badge", "cost": 1200, "desc": "A rare-feeling badge for casino winners." },
      { "id": "premium_star_badge", "name": "Premium Star Badge", "section": "Badges", "type": "cosmetic", "costType": "chat_badge", "cost": 1400, "premiumOnly": true, "desc": "A premium star badge for your profile and chats." },
      { "id": "owner_fan_badge", "name": "Mitch Fan Badge", "section": "Badges", "type": "cosmetic", "costType": "chat_badge", "cost": 800, "desc": "A simple badge for fans of the site." },
      { "id": "chat_sparkles", "name": "Chat Sparkles", "section": "Chat Effects", "type": "cosmetic", "costType": "chat_effect", "cost": 850, "desc": "Adds a subtle sparkle effect to your chat identity." },
      { "id": "chat_shadow", "name": "Chat Shadow", "section": "Chat Effects", "type": "cosmetic", "costType": "chat_effect", "cost": 850, "desc": "Adds a dark shadow accent to your chat identity." },
      { "id": "chat_wave", "name": "Chat Wave", "section": "Chat Effects", "type": "cosmetic", "costType": "chat_effect", "cost": 1000, "desc": "A gentle animated wave effect for your chat name." },
      { "id": "chat_terminal", "name": "Terminal Chat Style", "section": "Chat Effects", "type": "cosmetic", "costType": "chat_effect", "cost": 1100, "desc": "A monospace terminal-style chat accent." },
      { "id": "chat_prism", "name": "Prism Chat Style", "section": "Chat Effects", "type": "cosmetic", "costType": "chat_effect", "cost": 1600, "premiumOnly": true, "desc": "A premium prism accent for chat." },
      { "id": "profile_grid", "name": "Profile Grid Background", "section": "Profile Effects", "type": "cosmetic", "costType": "profile_effect", "cost": 900, "desc": "Adds a clean grid effect to your profile." },
      { "id": "profile_stars", "name": "Profile Starfield", "section": "Profile Effects", "type": "cosmetic", "costType": "profile_effect", "cost": 1200, "desc": "Adds a starfield-style profile effect." },
      { "id": "profile_scanlines", "name": "Profile Scanlines", "section": "Profile Effects", "type": "cosmetic", "costType": "profile_effect", "cost": 950, "desc": "Adds a retro scanline texture to your profile." },
      { "id": "profile_gold_frame", "name": "Gold Profile Frame", "section": "Profile Effects", "type": "cosmetic", "costType": "profile_effect", "cost": 1800, "premiumOnly": true, "desc": "A premium gold frame accent for your profile." },
      { "id": "profile_neon_frame", "name": "Neon Profile Frame", "section": "Profile Effects", "type": "cosmetic", "costType": "profile_effect", "cost": 1800, "premiumOnly": true, "desc": "A premium neon frame accent for your profile." },
      { "id": "focus_theme", "name": "Focus Theme", "section": "Site Themes", "type": "cosmetic", "costType": "site_theme", "cost": 700, "desc": "A calm, low-distraction site accent." },
      { "id": "arcade_theme", "name": "Arcade Theme", "section": "Site Themes", "type": "cosmetic", "costType": "site_theme", "cost": 900, "desc": "A brighter arcade-style site accent." },
      { "id": "midnight_theme", "name": "Midnight Theme", "section": "Site Themes", "type": "cosmetic", "costType": "site_theme", "cost": 900, "desc": "A darker midnight accent for the site." },
      { "id": "vip_pass", "name": "VIP Casino Pass (24h)", "section": "Passes", "type": "pass", "costType": "vip_casino_pass", "cost": 250, "desc": "Unlocks unlimited max bet amount in all casino games for 24 hours." },
      { "id": "canvas_lock_pass", "name": "Canvas Lock Pass", "section": "Passes", "type": "cosmetic", "costType": "canvas_tool", "cost": 1200, "desc": "Unlocks a saved canvas-tool preference toggle." },
      { "id": "quick_access_pass", "name": "Quick Access Pass", "section": "Passes", "type": "cosmetic", "costType": "canvas_tool", "cost": 800, "desc": "Unlocks a quick-access preference toggle." },
      { "id": "daily_bonus_plus", "name": "Daily Bonus Plus", "section": "Passes", "type": "cosmetic", "costType": "canvas_tool", "cost": 1500, "premiumOnly": true, "desc": "Unlocks a premium daily-bonus preference toggle." },
      { "id": "streak_freeze", "name": "Streak Freeze", "section": "Utility", "type": "utility", "costType": "streak_freeze", "cost": 150, "desc": "Automatically saves your Daily Login streak if you miss a day!" },
      { "id": "happy_hour_ticket", "name": "Personal Happy Hour (30m)", "section": "Utility", "type": "utility", "costType": "happy_hour_ticket", "cost": 350, "desc": "Trigger a personal 30-minute Happy Hour for 2X coins on all games and canvas!" },
      { "id": "double_down_ticket", "name": "Double Down Ticket (30m)", "section": "Utility", "type": "utility", "costType": "double_down_ticket", "cost": 500, "desc": "Active for 30 minutes. Doubles the payout of any casino game wins!" },
      { "id": "bad_beat_insurance", "name": "Bad Beat Insurance (30m)", "section": "Utility", "type": "utility", "costType": "bad_beat_insurance", "cost": 300, "desc": "Active for 30 minutes. Refunds your entire bet if you lose any casino game round." },
      { "id": "happy_hour_extension", "name": "Happy Hour Extension (15m)", "section": "Utility", "type": "utility", "costType": "happy_hour_extension", "cost": 250, "desc": "Extends your active Personal Happy Hour by an additional 15 minutes. Requires active Happy Hour to purchase." },
      { "id": "slots_free_spin", "name": "Slots Free Spins (5x)", "section": "Utility", "type": "utility", "costType": "slots_free_spin", "cost": 200, "desc": "Adds 5 free spins to your account. Free spins let you play slots with zero coins at risk while keeping all winnings!" }
    ])
}

/// The boot-time `SHOP_CATALOG`: shop_catalog.json override (minus `og_badge`)
/// when non-empty, else the default list. server.js:7256-7267.
pub fn load_shop_catalog(store: &DataStore, data_dir: &Path) -> Vec<Value> {
    let loaded = store.read_document(&data_dir.join("shop_catalog.json"), Value::Null);
    if let Some(arr) = loaded.as_array() {
        if !arr.is_empty() {
            return arr
                .iter()
                .filter(|item| item.get("id").and_then(|v| v.as_str()) != Some("og_badge"))
                .cloned()
                .collect();
        }
    }
    default_shop_catalog()
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// `shopItemById` (server.js:7305): catalog first, then the default list.
/// Callers hand the dynamic catalog; fall back with `default_shop_catalog()`.
pub fn shop_item_by_id<'a>(catalog: &'a [Value], item_id: &str) -> Option<&'a Value> {
    catalog
        .iter()
        .find(|item| item.get("id").and_then(|v| v.as_str()) == Some(item_id))
}

/// `shopTierFor` (server.js:7316-7327).
pub fn shop_tier_for(item: &Value) -> &'static str {
    if item.is_null() {
        return "common";
    }
    if item.get("type").and_then(|v| v.as_str()) == Some("premium") {
        return "legendary";
    }
    let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let premium_only = item
        .get("premiumOnly")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if premium_only
        || matches!(
            id,
            "rainbow_name" | "profile_gold_frame" | "profile_neon_frame" | "chat_prism"
        )
    {
        return "elite";
    }
    if matches!(
        id,
        "lucky_badge" | "chat_wave" | "daily_bonus_plus" | "profile_stars"
    ) {
        return "rare";
    }
    if matches!(
        id,
        "vip_pass" | "arcade_theme" | "midnight_theme" | "verified_badge" | "og_badge"
    ) {
        return "uncommon";
    }
    "common"
}

/// `shopPerkFor` (server.js:7329-7360).
pub fn shop_perk_for(item: &Value) -> String {
    if item.is_null() {
        return String::new();
    }
    let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let perks: &[(&str, &str)] = &[
        (
            "premium",
            "Best value: unlocks premium tools, chat, colors, and bigger brushes.",
        ),
        (
            "rainbow_name",
            "Animated rainbow name. Flashiest name style in the market.",
        ),
        (
            "gold_glow",
            "Premium gold glow that stands out on dark pages.",
        ),
        (
            "chat_prism",
            "Premium prism chat accent with the most noticeable chat style.",
        ),
        (
            "profile_gold_frame",
            "High-status profile frame for premium members.",
        ),
        (
            "profile_neon_frame",
            "Bright neon profile frame with stronger profile presence.",
        ),
        ("vip_pass", "24 hours of unlimited casino max bets."),
        ("daily_bonus_plus", "Premium daily-bonus preference toggle."),
        (
            "canvas_lock_pass",
            "Canvas-tool preference for protecting important pixel work.",
        ),
        (
            "quick_access_pass",
            "Convenience toggle for faster navigation.",
        ),
    ];
    if let Some((_, perk)) = perks.iter().find(|(k, _)| *k == id) {
        return (*perk).to_string();
    }
    let cost_type = item.get("costType").and_then(|v| v.as_str()).unwrap_or("");
    match cost_type {
        "name_color" => "Changes your visible identity color.",
        "chat_badge" => "Adds a visible badge beside your identity.",
        "chat_effect" => "Adds a style effect to chat identity.",
        "profile_effect" => "Upgrades your public profile look.",
        "site_theme" => "Unlocks a site accent you can toggle on or off.",
        "ai_personality" => "Unlocks a selectable AI assistant personality.",
        "canvas_tool" => "Unlocks a canvas or site preference toggle.",
        _ => "",
    }
    .to_string()
}

/// `shopBaseCostFor` (server.js:7362-7393): `Math.max(1, ceil(cost*mult/25)*25)`.
pub fn shop_base_cost_for(item: &Value) -> i64 {
    if item.is_null() {
        return 0;
    }
    let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let cost_type = item.get("costType").and_then(|v| v.as_str()).unwrap_or("");
    let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let id_multipliers: &[(&str, f64)] = &[
        ("rainbow_name", 2.35),
        ("gold_glow", 2.2),
        ("chat_prism", 2.35),
        ("profile_gold_frame", 2.35),
        ("profile_neon_frame", 2.35),
        ("daily_bonus_plus", 2.5),
    ];
    let mult = item
        .get("priceMultiplier")
        .and_then(|v| v.as_f64())
        .filter(|v| *v != 0.0)
        .or_else(|| {
            id_multipliers
                .iter()
                .find(|(k, _)| *k == id)
                .map(|(_, m)| *m)
        })
        .unwrap_or_else(|| {
            let by_type = |t: &str| -> f64 {
                match t {
                    "premium" => 1.8,
                    "name_color" => 2.05,
                    "chat_badge" => 1.95,
                    "chat_effect" => 2.1,
                    "profile_effect" => 2.15,
                    "site_theme" => 1.8,
                    "ai_personality" => 2.35,
                    "vip_casino_pass" => 3.2,
                    "canvas_tool" => 2.2,
                    _ => 0.0,
                }
            };
            let m = by_type(cost_type);
            if m != 0.0 {
                m
            } else {
                let m = by_type(item_type);
                if m != 0.0 {
                    m
                } else {
                    1.85 // SHOP_PRICE_MULTIPLIER
                }
            }
        });
    let cost = item.get("cost").and_then(|v| v.as_f64()).unwrap_or(0.0);
    ((cost * mult) / 25.0).ceil() as i64 * 25
}

/// `premiumDiscountFor` (server.js:7395-7433).
pub fn premium_discount_for(item: &Value) -> f64 {
    if item.is_null() || item.get("type").and_then(|v| v.as_str()) == Some("premium") {
        return 0.0;
    }
    let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("");
    if let Some(d) = item.get("premiumDiscount").and_then(|v| v.as_f64()) {
        return d.clamp(0.0, 0.5);
    }
    let id_discounts: &[(&str, f64)] = &[
        ("rainbow_name", 0.06),
        ("gold_glow", 0.08),
        ("premium_star_badge", 0.10),
        ("chat_prism", 0.09),
        ("profile_gold_frame", 0.07),
        ("profile_neon_frame", 0.07),
        ("vip_pass", 0.04),
        ("daily_bonus_plus", 0.06),
        ("focus_theme", 0.20),
        ("arcade_theme", 0.18),
        ("midnight_theme", 0.18),
    ];
    if let Some((_, d)) = id_discounts.iter().find(|(k, _)| *k == id) {
        return *d;
    }
    let cost_type = item.get("costType").and_then(|v| v.as_str()).unwrap_or("");
    let by_type: &[(&str, f64)] = &[
        ("name_color", 0.12),
        ("chat_badge", 0.14),
        ("chat_effect", 0.15),
        ("profile_effect", 0.10),
        ("site_theme", 0.18),
        ("ai_personality", 0.09),
        ("canvas_tool", 0.11),
        ("vip_casino_pass", 0.04),
    ];
    if let Some((_, d)) = by_type.iter().find(|(k, _)| *k == cost_type) {
        return *d;
    }
    if item
        .get("premiumOnly")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        0.06
    } else {
        0.10
    }
}

/// `shopCostFor` (server.js:7435-7462) — streak-freeze ladder + premium
/// discount. `daily_logins` is read from the store (JS uses its in-memory
/// `dailyLogins` cache; same values, single-instance assumption preserved).
pub fn shop_cost_for(store: &DataStore, data_dir: &Path, item: &Value, email: &str) -> i64 {
    if item.is_null() {
        return 0;
    }
    let mut base_cost = shop_base_cost_for(item);
    let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("");
    if id == "streak_freeze" && !email.is_empty() {
        let norm = auth::normalize_email(email);
        let daily = store
            .read_document(&data_dir.join("daily_logins.json"), json!({}))
            .get(norm.as_str())
            .cloned()
            .unwrap_or(json!({}));
        let freezes = daily
            .get("streakFreezes")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        base_cost = match freezes {
            0 => 150,
            1 => 600,
            2 => 2000,
            _ => 5000,
        };
    }
    if auth::is_premium_email(store, email)
        && item.get("type").and_then(|v| v.as_str()) != Some("premium")
    {
        let d = premium_discount_for(item);
        let discounted = (base_cost as f64 * (1.0 - d) / 25.0).ceil() as i64 * 25;
        return discounted.max(1);
    }
    base_cost
}

/// `shopItemsFor` (server.js:7435 area): catalog filtered for non-admins,
/// each item decorated with tier/perk/originalCost/cost/discountPct.
pub fn shop_items_for(
    store: &DataStore,
    data_dir: &Path,
    catalog: &[Value],
    email: &str,
) -> Vec<Value> {
    let premium = auth::is_premium_email(store, email);
    let admin = auth::is_admin_email(store, email);
    catalog
        .iter()
        .filter(|item| {
            admin
                || !item
                    .get("adminOnly")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
        })
        .map(|item| {
            let mut out = item.clone();
            if let Some(obj) = out.as_object_mut() {
                obj.insert("tier".into(), json!(shop_tier_for(item)));
                obj.insert("perk".into(), json!(shop_perk_for(item)));
                obj.insert("originalCost".into(), json!(shop_base_cost_for(item)));
                obj.insert(
                    "cost".into(),
                    json!(shop_cost_for(store, data_dir, item, email)),
                );
                let discount_pct =
                    if premium && item.get("type").and_then(|v| v.as_str()) != Some("premium") {
                        (premium_discount_for(item) * 100.0).round() as i64
                    } else {
                        0
                    };
                obj.insert("discountPct".into(), json!(discount_pct));
            }
            out
        })
        .collect()
}

/// `defaultCosmetics` (server.js:7278-7292).
pub fn default_cosmetics() -> Value {
    json!({
        "colors": [],
        "badges": [],
        "chatEffects": [],
        "profileEffects": [],
        "themes": [],
        "tools": [],
        "activeColor": "",
        "activeBadge": "",
        "activeChatEffect": "",
        "activeProfileEffect": "",
        "activeTheme": "",
        "activeTool": "",
        "activeAi": ""
    })
}

/// JS `Boolean(v)` for the value shapes cosmetics arrays hold.
fn js_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// `normalizeCosmetics` (server.js:7294-7303): arrays deduped of falsy values,
/// strings defaulted to ''.
pub fn normalize_cosmetics(entry: &Value) -> Value {
    let base = default_cosmetics();
    let mut out = base;
    let Some(base_obj) = out.as_object_mut() else {
        return out;
    };
    let entry_map = entry.as_object();
    for (key, default) in base_obj.iter_mut() {
        if default.is_array() {
            let items: Vec<Value> = entry_map
                .and_then(|m| m.get(key.as_str()))
                .and_then(|v| v.as_array())
                .map(|arr| {
                    let mut seen = std::collections::HashSet::new();
                    arr.iter()
                        .filter(|v| js_truthy(v))
                        .filter(|v| seen.insert((*v).clone()))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            *default = Value::Array(items);
        } else {
            let s = entry_map
                .and_then(|m| m.get(key.as_str()))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            *default = json!(s);
        }
    }
    out
}

/// `sanitizeCosmeticsForEmail` (server.js:6769-6777): strip `rainbow_name`
/// from non-admins.
pub fn sanitize_cosmetics_for_email(store: &DataStore, email: &str, entry: &Value) -> Value {
    let mut user_cosm = normalize_cosmetics(entry);
    if !auth::is_admin_email(store, email) {
        if let Some(obj) = user_cosm.as_object_mut() {
            if let Some(colors) = obj.get_mut("colors").and_then(|v| v.as_array_mut()) {
                colors.retain(|c| c.as_str() != Some("rainbow_name"));
            }
            if obj.get("activeColor").and_then(|v| v.as_str()) == Some("rainbow_name") {
                obj.insert("activeColor".into(), json!(""));
            }
        }
    }
    user_cosm
}

/// `publicActiveColor` (server.js:6763-6767).
pub fn public_active_color(store: &DataStore, email: &str, color: &Value) -> Option<String> {
    let active = color.as_str().unwrap_or("").to_string();
    if active == "rainbow_name" && !auth::is_admin_email(store, email) {
        return None;
    }
    if active.is_empty() {
        None
    } else {
        Some(active)
    }
}

/// `buildInventory` (server.js:7448-7489): sanitized cosmetics + AI unlocks +
/// item details + status block.
pub fn build_inventory(
    store: &DataStore,
    data_dir: &Path,
    catalog: &[Value],
    email: &str,
) -> Value {
    let norm = auth::normalize_email(email);
    let cosm = store.read_document(&data_dir.join("cosmetics.json"), json!({}));
    let unlocked_ai = store.read_document(&data_dir.join("unlocked_ai.json"), json!({}));
    let stats = store
        .read_document(&data_dir.join("user_stats.json"), json!({}))
        .get(norm.as_str())
        .cloned()
        .unwrap_or(json!({}));
    let daily = store
        .read_document(&data_dir.join("daily_logins.json"), json!({}))
        .get(norm.as_str())
        .cloned()
        .unwrap_or(json!({}));
    let entry = cosm.get(norm.as_str()).cloned().unwrap_or(json!({}));
    let user_cosm = sanitize_cosmetics_for_email(store, email, &entry);
    let ai: Vec<Value> = unlocked_ai
        .get(norm.as_str())
        .and_then(|v| v.as_array())
        .map(|arr| {
            let mut seen = std::collections::HashSet::new();
            arr.iter()
                .filter(|v| v.as_str().map(|s| !s.is_empty()).unwrap_or(false))
                .filter(|v| seen.insert((*v).clone()))
                .cloned()
                .collect()
        })
        .unwrap_or_default();

    let mut item_ids: Vec<String> = Vec::new();
    let mut push_ids = |bucket: &str| {
        if let Some(arr) = user_cosm.get(bucket).and_then(|v| v.as_array()) {
            for v in arr {
                if let Some(s) = v.as_str() {
                    if !s.is_empty() && !item_ids.iter().any(|x| x == s) {
                        item_ids.push(s.to_string());
                    }
                }
            }
        }
    };
    for bucket in [
        "colors",
        "badges",
        "chatEffects",
        "profileEffects",
        "themes",
        "tools",
    ] {
        push_ids(bucket);
    }
    for v in &ai {
        if let Some(s) = v.as_str() {
            if !item_ids.iter().any(|x| x == s) {
                item_ids.push(s.to_string());
            }
        }
    }

    let defaults = default_shop_catalog_vec();
    let items: Vec<Value> = item_ids
        .iter()
        .map(|id| {
            let item = shop_item_by_id(catalog, id)
                .or_else(|| shop_item_by_id(&defaults, id))
                .cloned()
                .unwrap_or_else(|| {
                    json!({
                        "id": id,
                        "name": id.replace(['_', '-'], " "),
                        "section": "Removed Items",
                        "type": "cosmetic",
                        "costType": "unknown",
                        "desc": "This item is no longer listed in the shop."
                    })
                });
            let mut out = item;
            let cost_type = out
                .get("costType")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let active_key = if cost_type == "ai_personality" {
                Some("activeAi".to_string())
            } else {
                shop_type_config(&cost_type).map(|(_, active)| active.to_string())
            };
            let removed = !catalog
                .iter()
                .any(|active| active.get("id").and_then(|v| v.as_str()) == Some(id));
            if let Some(obj) = out.as_object_mut() {
                obj.insert("removedFromShop".into(), json!(removed));
                let active = active_key
                    .and_then(|key| user_cosm.get(key.as_str()))
                    .and_then(|v| v.as_str())
                    .map(|s| s == id.as_str())
                    .unwrap_or(false);
                obj.insert("active".into(), json!(active));
            }
            out
        })
        .collect();

    json!({
        "cosmetics": user_cosm,
        "ai": ai,
        "items": items,
        "status": {
            "vipUntil": stats.get("vip_casino_until").cloned().unwrap_or(json!(0)),
            "happyHourUntil": stats.get("personal_happy_hour_until").cloned().unwrap_or(json!(0)),
            "doubleDownUntil": stats.get("double_down_until").cloned().unwrap_or(json!(0)),
            "badBeatInsuranceUntil": stats.get("bad_beat_insurance_until").cloned().unwrap_or(json!(0)),
            "slotsFreeSpins": stats.get("slots_free_spins").cloned().unwrap_or(json!(0)),
            "streakFreezes": daily.get("streakFreezes").cloned().unwrap_or(json!(0)),
        }
    })
}

fn default_shop_catalog_vec() -> Vec<Value> {
    default_shop_catalog()
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// `ownsShopItem` (server.js:7494-7509) — used by later purchase flows.
pub fn owns_shop_item(
    store: &DataStore,
    data_dir: &Path,
    catalog: &[Value],
    email: &str,
    item: &Value,
) -> bool {
    if item.is_null() {
        return false;
    }
    if item.get("type").and_then(|v| v.as_str()) == Some("premium") {
        return auth::is_premium_email(store, email);
    }
    let inventory = build_inventory(store, data_dir, catalog, email);
    let cost_type = item.get("costType").and_then(|v| v.as_str()).unwrap_or("");
    let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("");
    if cost_type == "ai_personality" {
        return inventory
            .get("ai")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().any(|v| v.as_str() == Some(id)))
            .unwrap_or(false);
    }
    if cost_type == "vip_casino_pass" {
        let norm = auth::normalize_email(email);
        let until = store
            .read_document(&data_dir.join("user_stats.json"), json!({}))
            .get(norm.as_str())
            .and_then(|s| s.get("vip_casino_until"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as f64)
            .unwrap_or(0.0);
        return until > now_ms;
    }
    if cost_type == "streak_freeze" || cost_type == "happy_hour_ticket" {
        return false;
    }
    match shop_type_config(cost_type) {
        Some((bucket, _)) => inventory
            .get("cosmetics")
            .and_then(|c| c.get(bucket))
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().any(|v| v.as_str() == Some(id)))
            .unwrap_or(false),
        None => false,
    }
}

/// Convenience for callers holding a `Value` entry: JS reads
/// `userCosm[cfg.bucket].includes(itemId)`.
pub fn cosmetics_bucket_has(user_cosm: &Value, bucket: &str, item_id: &str) -> bool {
    user_cosm
        .get(bucket)
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().any(|v| v.as_str() == Some(item_id)))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Vec<Value> {
        default_shop_catalog_vec()
    }

    #[test]
    fn base_cost_rounds_to_quarters() {
        let item = json!({"id": "x", "cost": 500, "costType": "name_color"});
        // 500 * 2.05 = 1025 -> ceil(1025/25)*25 = 1025
        assert_eq!(shop_base_cost_for(&item), 1025);
    }

    #[test]
    fn tier_ladder() {
        let cat = catalog();
        let premium = shop_item_by_id(&cat, "premium").unwrap();
        assert_eq!(shop_tier_for(premium), "legendary");
        let rainbow = shop_item_by_id(&cat, "rainbow_name").unwrap();
        assert_eq!(shop_tier_for(rainbow), "elite");
        let lucky = shop_item_by_id(&cat, "lucky_badge").unwrap();
        assert_eq!(shop_tier_for(lucky), "rare");
        let verified = shop_item_by_id(&cat, "verified_badge").unwrap();
        assert_eq!(shop_tier_for(verified), "uncommon");
        let neon = shop_item_by_id(&cat, "neon_purple").unwrap();
        assert_eq!(shop_tier_for(neon), "common");
    }

    #[test]
    fn normalize_cosmetics_defaults() {
        let out = normalize_cosmetics(&json!({"colors": ["a", "a", null], "activeColor": 5}));
        assert_eq!(out.get("colors"), Some(&json!(["a"])));
        assert_eq!(out.get("activeColor"), Some(&json!("")));
        assert_eq!(out.get("badges"), Some(&json!([])));
    }
}
