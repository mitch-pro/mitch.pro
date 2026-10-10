//! The idle games: Adrian Clicker 2.0 (server.js:22866-22962) and Richard's
//! Riches (22964-23056), with their shared helpers (server.js:616-823):
//! `getAdrianPower` / `applyAdrianOffline` / `processAdrianSync`,
//! `RICHARD_BUSINESSES` / `getRichardSpeed` / `getRichardPower` /
//! `applyRichardOffline`, and the `ADRIAN_TECH` / `ADRIAN_UPGRADES` tables.
//!
//! Session objects live in the `mini_game_sessions` table
//! (`mitch_lib::minigames`, game = "clicker"/"richard") as verbatim
//! `serde_json::Value`s and every response echoes them back — so response
//! bodies go out through `mitch_lib::data::js_stringify` (exact
//! ECMAScript number rendering: `1e+22`, no `.0`), not serde_json.

use axum::http::{HeaderMap, Method};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::errors::json_resp_str;
use crate::state::AppState;
use mitch_lib::data::{js_stringify, DataStore};
use mitch_lib::jsval;
use std::path::Path;

/// `SAFE_CAP` — both games clamp point/cash accumulators at 1e290.
const SAFE_CAP: f64 = 1e290;

/// `ADRIAN_TECH` (server.js:493-510) — key order preserved (JSON.stringify
/// echoes these verbatim in the state response).
pub static ADRIAN_TECH: std::sync::LazyLock<Vec<(&'static str, Value)>> = std::sync::LazyLock::new(
    || {
        vec![
            (
                "desk_1",
                json!({"name": "Reinforced Desk", "cost": 100, "icon": "🪑", "target": "desk", "mult": 2, "unlock": {"id": "desk", "n": 1}}),
            ),
            (
                "desk_2",
                json!({"name": "Ergonomic Chair", "cost": 500, "icon": "💺", "target": "desk", "mult": 2, "unlock": {"id": "desk", "n": 10}}),
            ),
            (
                "desk_3",
                json!({"name": "Dual Monitor", "cost": 10000, "icon": "🖥️", "target": "desk", "mult": 2, "unlock": {"id": "desk", "n": 25}}),
            ),
            (
                "chrome_1",
                json!({"name": "Speed Extension", "cost": 1000, "icon": "⚡", "target": "chromebook", "mult": 2, "unlock": {"id": "chromebook", "n": 1}}),
            ),
            (
                "chrome_2",
                json!({"name": "Overclocked RAM", "cost": 5000, "icon": "🧠", "target": "chromebook", "mult": 2, "unlock": {"id": "chromebook", "n": 10}}),
            ),
            (
                "fiber_1",
                json!({"name": "Cat6 Cables", "cost": 11000, "icon": "🔌", "target": "fiber", "mult": 2, "unlock": {"id": "fiber", "n": 1}}),
            ),
            (
                "fiber_2",
                json!({"name": "Router Pro", "cost": 55000, "icon": "📡", "target": "fiber", "mult": 2, "unlock": {"id": "fiber", "n": 10}}),
            ),
            (
                "ai_1",
                json!({"name": "Neural Network", "cost": 120000, "icon": "🤖", "target": "ai_bot", "mult": 2, "unlock": {"id": "ai_bot", "n": 1}}),
            ),
            (
                "ai_2",
                json!({"name": "Quantum Training", "cost": 600000, "icon": "✨", "target": "ai_bot", "mult": 2, "unlock": {"id": "ai_bot", "n": 10}}),
            ),
            (
                "mitch_1",
                json!({"name": "Mitch's Advice", "cost": 5000, "icon": "💡", "target": "global", "mult": 2, "unlock": {"id": "desk", "n": 5}}),
            ),
            (
                "mitch_2",
                json!({"name": "Community Server", "cost": 500000, "icon": "🌐", "target": "global", "mult": 2, "unlock": {"id": "mainframe", "n": 5}}),
            ),
            (
                "star_1",
                json!({"name": "Pulsar Harvest", "cost": 1e15, "icon": "💫", "target": "neutron_star", "mult": 2, "unlock": {"id": "neutron_star", "n": 1}}),
            ),
            (
                "star_2",
                json!({"name": "Quasar Focus", "cost": 5e15, "icon": "💠", "target": "neutron_star", "mult": 2, "unlock": {"id": "neutron_star", "n": 10}}),
            ),
            (
                "void_1",
                json!({"name": "Void Insight", "cost": 1e18, "icon": "🌑", "target": "black_hole", "mult": 2, "unlock": {"id": "black_hole", "n": 1}}),
            ),
            (
                "googol_1",
                json!({"name": "Infinite Logic", "cost": 1e85, "icon": "♾️", "target": "global", "mult": 10, "unlock": {"id": "beyond_googol", "n": 1}}),
            ),
        ]
    },
);

/// `ADRIAN_UPGRADES` (server.js:511-538). `type: "c"` = click power,
/// `type: "a"` = auto power.
pub static ADRIAN_UPGRADES: std::sync::LazyLock<Vec<(&'static str, Value)>> =
    std::sync::LazyLock::new(|| {
        vec![
            (
                "desk",
                json!({"name": "Student Desk", "baseCost": 15, "power": 0.1, "type": "c", "desc": "Basic study station."}),
            ),
            (
                "chromebook",
                json!({"name": "Chromebook Script", "baseCost": 100, "power": 1, "type": "a", "desc": "Automated clicking script."}),
            ),
            (
                "fiber",
                json!({"name": "Fiber Connection", "baseCost": 1100, "power": 8, "type": "a", "desc": "Ultra-low latency clicks."}),
            ),
            (
                "ai_bot",
                json!({"name": "Assistant Bot", "baseCost": 12000, "power": 47, "type": "a", "desc": "AI-driven productivity."}),
            ),
            (
                "mainframe",
                json!({"name": "High-End Mainframe", "baseCost": 130000, "power": 260, "type": "a", "desc": "Enterprise-grade speed."}),
            ),
            (
                "quantum",
                json!({"name": "Quantum Core", "baseCost": 1400000, "power": 1400, "type": "a", "desc": "Beyond human limits."}),
            ),
            (
                "cloud_farm",
                json!({"name": "Cloud Computing Farm", "baseCost": 20000000, "power": 7800, "type": "a", "desc": "Distributed clicking power."}),
            ),
            (
                "satellite",
                json!({"name": "Orbital Uplink", "baseCost": 330000000, "power": 44000, "type": "a", "desc": "Interstellar bandwidth."}),
            ),
            (
                "dyson",
                json!({"name": "Dyson Swarm", "baseCost": 5100000000.0, "power": 260000, "type": "a", "desc": "Total solar output clicks."}),
            ),
            (
                "singularity",
                json!({"name": "AI Singularity", "baseCost": 75000000000.0, "power": 1600000, "type": "a", "desc": "Infinite intelligence."}),
            ),
            (
                "multiverse",
                json!({"name": "Multiverse Bridge", "baseCost": 1e12, "power": 10000000, "type": "a", "desc": "Harvesting other timelines."}),
            ),
            (
                "neutron_star",
                json!({"name": "Neutron Star Forge", "baseCost": 1.4e13, "power": 65000000, "type": "a", "desc": "High-density clicking."}),
            ),
            (
                "antimatter",
                json!({"name": "Antimatter Engine", "baseCost": 1.7e17, "power": 430000000, "type": "a", "desc": "Pure annihilation speed."}),
            ),
            (
                "black_hole",
                json!({"name": "Black Hole Event Horizon", "baseCost": 2.1e18, "power": 2.9e9, "type": "a", "desc": "Time-dilated clicking."}),
            ),
            (
                "galactic_cluster",
                json!({"name": "Galactic Cluster", "baseCost": 2.6e22, "power": 2.1e10, "type": "a", "desc": "A trillion worlds clicking."}),
            ),
            (
                "supercluster",
                json!({"name": "Laniakea Supercluster", "baseCost": 3.1e24, "power": 1.5e11, "type": "a", "desc": "The great attractor."}),
            ),
            (
                "dimension_rip",
                json!({"name": "Dimensional Rip", "baseCost": 7.1e28, "power": 1.1e12, "type": "a", "desc": "Bleeding points from 2D."}),
            ),
            (
                "hyper_dimension",
                json!({"name": "11th Dimension", "baseCost": 1.2e32, "power": 8.3e12, "type": "a", "desc": "Multi-dimensional input."}),
            ),
            (
                "string_theory",
                json!({"name": "String Theory Core", "baseCost": 1.9e38, "power": 6.4e13, "type": "a", "desc": "Vibrating atoms."}),
            ),
            (
                "quantum_foam",
                json!({"name": "Quantum Foam", "baseCost": 5.4e42, "power": 5.1e14, "type": "a", "desc": "Clicking at the Planck scale."}),
            ),
            (
                "beyond_googol",
                json!({"name": "Beyond Googol", "baseCost": 1e80, "power": 1e30, "type": "a", "desc": "Numbers without names."}),
            ),
            (
                "infinite_set",
                json!({"name": "Infinite Set", "baseCost": 1e100, "power": 1e45, "type": "a", "desc": "Cantor would be proud."}),
            ),
            (
                "aleph_null",
                json!({"name": "Aleph Null", "baseCost": 1e140, "power": 1e65, "type": "a", "desc": "Counting the uncountable."}),
            ),
            (
                "quantum_singularity",
                json!({"name": "Quantum Singularity", "baseCost": 1e200, "power": 1e85, "type": "a", "desc": "Crushing logic."}),
            ),
            (
                "omnipresence",
                json!({"name": "Omnipresence", "baseCost": 1e260, "power": 1e135, "type": "a", "desc": "Everywhere at once."}),
            ),
        ]
    });

fn adrian_upgrade(id: &str) -> Option<&Value> {
    ADRIAN_UPGRADES
        .iter()
        .find(|(k, _)| *k == id)
        .map(|(_, v)| v)
}

fn adrian_tech(id: &str) -> Option<&Value> {
    ADRIAN_TECH.iter().find(|(k, _)| *k == id).map(|(_, v)| v)
}

fn adrian_upgrades_object() -> Value {
    Value::Object(
        ADRIAN_UPGRADES
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}

fn adrian_tech_object() -> Value {
    Value::Object(
        ADRIAN_TECH
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}

/// `RICHARD_BUSINESSES` (server.js:683-696) — id → {name, baseCost,
/// baseRevenue, baseSpeed}, in the JS object-literal order.
pub static RICHARD_BUSINESSES: std::sync::LazyLock<Vec<(&'static str, Value)>> =
    std::sync::LazyLock::new(|| {
        vec![
            (
                "lemon",
                json!({"name": "Lemon Squeezer", "baseCost": 4, "baseRevenue": 1, "baseSpeed": 0.6}),
            ),
            (
                "news",
                json!({"name": "Newspaper Delivery", "baseCost": 60, "baseRevenue": 60, "baseSpeed": 3}),
            ),
            (
                "carwash",
                json!({"name": "Car Wash", "baseCost": 720, "baseRevenue": 540, "baseSpeed": 6}),
            ),
            (
                "pizza",
                json!({"name": "Pizza Delivery", "baseCost": 8640, "baseRevenue": 4320, "baseSpeed": 12}),
            ),
            (
                "donut",
                json!({"name": "Donut Shop", "baseCost": 103680, "baseRevenue": 51840, "baseSpeed": 24}),
            ),
            (
                "shrimp",
                json!({"name": "Shrimp Boat", "baseCost": 1244160, "baseRevenue": 622080, "baseSpeed": 96}),
            ),
            (
                "hockey",
                json!({"name": "Hockey Team", "baseCost": 14929920, "baseRevenue": 7464960, "baseSpeed": 384}),
            ),
            (
                "movie",
                json!({"name": "Movie Studio", "baseCost": 179159040, "baseRevenue": 89579520, "baseSpeed": 1536}),
            ),
            (
                "bank",
                json!({"name": "Bank", "baseCost": 2149908480.0, "baseRevenue": 1074954240.0, "baseSpeed": 6144}),
            ),
            (
                "oil",
                json!({"name": "Oil Company", "baseCost": 25798901760.0, "baseRevenue": 29668737024.0, "baseSpeed": 36864}),
            ),
        ]
    });

/// `getAdrianPower(s)` (server.js:618-638) → (click, auto).
pub fn get_adrian_power(upgrades: &Value) -> (f64, f64) {
    let mut s_click = 1.0;
    let mut s_auto = 0.0;
    let mut s_mult = 1.0;
    let mut build_mults: std::collections::HashMap<String, f64> = std::collections::HashMap::new();

    if let Some(obj) = upgrades.as_object() {
        for (id, count) in obj {
            let count = jsval::number(count).unwrap_or(0.0);
            if let Some(tech) = adrian_tech(id) {
                if count > 0.0 {
                    let mult = tech.get("mult").and_then(|v| v.as_f64()).unwrap_or(1.0);
                    match tech.get("target").and_then(|v| v.as_str()) {
                        Some("global") => s_mult *= mult,
                        Some(target) => {
                            *build_mults.entry(target.to_string()).or_insert(1.0) *= mult;
                        }
                        None => {}
                    }
                }
            }
        }
        for (id, count) in obj {
            let count = jsval::number(count).unwrap_or(0.0);
            let Some(def) = adrian_upgrade(id) else {
                continue;
            };
            let m = build_mults.get(id).copied().unwrap_or(1.0);
            let power = def.get("power").and_then(|v| v.as_f64()).unwrap_or(0.0);
            match def.get("type").and_then(|v| v.as_str()) {
                Some("c") => s_click += count * power * m,
                Some("a") => s_auto += count * power * m,
                _ => {}
            }
        }
    }
    (s_click * s_mult, s_auto * s_mult)
}

/// `applyAdrianOffline(s, email)` (server.js:641-663) — returns the offline
/// gain (0 when under the 15s threshold, which also skips the lastTs stamp).
pub fn apply_adrian_offline(
    store: &DataStore,
    _data_dir: &Path,
    s: &mut Value,
    email: &str,
    now: i64,
) -> f64 {
    let last_ts = s.get("lastTs").and_then(jsval::number).unwrap_or(0.0);
    let diff = now as f64 - last_ts;
    if diff < 15000.0 {
        return 0.0;
    }
    let offline_secs = (diff / 1000.0).min(43200.0);
    let is_premium = mitch_lib::auth::is_premium_email(store, email);
    let (click, auto) = get_adrian_power(s.get("upgrades").unwrap_or(&json!({})));
    let _ = click;
    let prem_mult = if is_premium { 2.0 } else { 1.0 };
    let efficiency = if is_premium { 1.0 } else { 0.5 };
    let offline_gain = auto * prem_mult * offline_secs * efficiency;

    if offline_gain > 0.0 {
        let points = s.get("points").and_then(jsval::number).unwrap_or(0.0);
        set_num(s, "points", (points + offline_gain).min(SAFE_CAP));
        set_num(s, "lastTs", now as f64);
        return offline_gain;
    }
    set_num(s, "lastTs", now as f64);
    0.0
}

/// `processAdrianSync(s, clientPoints, email, claim)` (server.js:767-823).
pub fn process_adrian_sync(
    state: &AppState,
    s: &mut Value,
    client_points: f64,
    email: &str,
    claim: bool,
    now: i64,
) {
    let now_f = now as f64;
    let last_ts = s.get("lastTs").and_then(jsval::number).unwrap_or(0.0);
    if now_f - last_ts > 30000.0 {
        apply_adrian_offline(&state.store, state.data_dir(), s, email, now);
    }

    let is_premium = mitch_lib::auth::is_premium_email(&state.store, email);
    let prem_mult = if is_premium { 2.0 } else { 1.0 };
    let (s_click, s_auto) = get_adrian_power(s.get("upgrades").unwrap_or(&json!({})));

    let last_ts = s.get("lastTs").and_then(jsval::number).unwrap_or(0.0);
    let elapsed = (now_f - last_ts) / 1000.0;
    let max_possible_gain = ((s_auto * elapsed) + (s_click * 60.0 * elapsed)) * prem_mult;
    let points = s.get("points").and_then(jsval::number).unwrap_or(0.0);
    let actual_gain = client_points - points;

    if actual_gain > max_possible_gain * 3.0 {
        mitch_lib::admin::log_cheat(
            &state.store,
            state.data_dir(),
            email,
            "Adrian Clicker 2.0",
            &format!(
                "Attempted impossible gain of {} points (max possible: {})",
                js_to_exponential(actual_gain, 2),
                js_to_exponential(max_possible_gain, 2)
            ),
            "unknown",
        );
        set_num(s, "points", (points + max_possible_gain).min(SAFE_CAP));
    } else {
        set_num(s, "points", points.max(client_points).min(SAFE_CAP));
    }

    let mut coins_to_grant = 0.0f64;
    let start_time = s.get("startTime").and_then(jsval::number).unwrap_or(now_f);
    let session_elapsed = now_f - if start_time == 0.0 { now_f } else { start_time };
    let total_playtime_coins = (session_elapsed / 600000.0).floor();
    let playtime_coins = s
        .get("playtimeCoins")
        .and_then(jsval::number)
        .unwrap_or(0.0);
    let pending_playtime_bonus = total_playtime_coins - playtime_coins;

    if claim {
        let mut coins = s.get("coins").and_then(jsval::number).unwrap_or(0.0);
        let mut points = s.get("points").and_then(jsval::number).unwrap_or(0.0);
        loop {
            let cost_of_next = (10.0 * 1.04f64.powf(coins)).floor();
            if points >= cost_of_next {
                points -= cost_of_next;
                coins += 1.0;
                coins_to_grant += 1.0;
            } else {
                break;
            }
        }
        set_num(s, "coins", coins);
        set_num(s, "points", points);
        if pending_playtime_bonus > 0.0 {
            coins_to_grant += pending_playtime_bonus;
            set_num(s, "playtimeCoins", playtime_coins + pending_playtime_bonus);
        }
    }

    if coins_to_grant > 0.0 {
        let bonus_count = if is_premium {
            coins_to_grant * 2.0
        } else {
            coins_to_grant
        };
        mitch_lib::coins::add_coins(
            &state.store,
            state.cfg.data_dir.as_path(),
            email,
            bonus_count,
            state.coin_multiplier(),
            "",
        );
        mitch_lib::achievements::update_stat(
            &state.store,
            state.data_dir(),
            email,
            "clicker_coins",
            bonus_count,
            state.coin_multiplier(),
        );
        tracing::info!(
            "[clicker] {email} earned {bonus_count} MitchCoins (Claim: {claim}, Playtime: {})",
            pending_playtime_bonus > 0.0
        );
    }

    if actual_gain > 0.0 {
        mitch_lib::achievements::update_stat(
            &state.store,
            state.data_dir(),
            email,
            "clicker_points",
            actual_gain.floor(),
            state.coin_multiplier(),
        );
    }
    set_num(s, "lastTs", now_f);
}

/// `getRichardSpeed(id, level, baseSpeed)` (server.js:698-712).
pub fn get_richard_speed(level: f64, base_speed: f64) -> f64 {
    let mut divisor = 1.0;
    for m in [
        25.0, 50.0, 100.0, 200.0, 300.0, 400.0, 500.0, 600.0, 700.0, 800.0, 900.0, 1000.0, 2000.0,
        3000.0, 4000.0, 5000.0,
    ] {
        if level >= m {
            divisor *= 2.0;
        } else {
            break;
        }
    }
    (base_speed / divisor).max(0.05)
}

/// `getRichardPower(s, checkManagers)` (server.js:714-741).
pub fn get_richard_power(s: &Value, check_managers: bool) -> f64 {
    let mut cash_per_sec = 0.0f64;
    let upgrades = s.get("upgrades").cloned().unwrap_or(json!([]));
    // `new Set(s.upgrades || [])` — when a client sends an OBJECT instead of
    // the expected array, the Set holds the object itself as its one element
    // and every has() is false; only the array case can match.
    let owned = |id: &str| -> bool {
        match &upgrades {
            Value::Array(a) => a.iter().any(|v| jsval::string(v) == id),
            _ => false,
        }
    };
    let mut global_mult = 1.0;
    if owned("all_1") {
        global_mult *= 2.0;
    }
    if owned("all_2") {
        global_mult *= 3.0;
    }
    if owned("all_3") {
        global_mult *= 5.0;
    }

    let levels = s.get("levels").cloned().unwrap_or(json!({}));
    let managers = s.get("managers").cloned().unwrap_or(json!({}));
    for (id, info) in RICHARD_BUSINESSES.iter() {
        let level = levels.get(*id).and_then(jsval::number).unwrap_or(0.0);
        let has_manager = managers.get(*id).map(jsval::truthy).unwrap_or(false);
        if level > 0.0 && (!check_managers || has_manager) {
            let mut mult = 1.0;
            if owned(&format!("{id}_1")) {
                mult *= 3.0;
            }
            let base_revenue = info
                .get("baseRevenue")
                .and_then(jsval::number)
                .unwrap_or(0.0);
            let base_speed = info.get("baseSpeed").and_then(jsval::number).unwrap_or(1.0);
            let revenue = level * base_revenue * mult * global_mult;
            let speed = get_richard_speed(level, base_speed);
            cash_per_sec += revenue / speed;
        }
    }
    cash_per_sec
}

/// `applyRichardOffline(s, email)` (server.js:743-766).
pub fn apply_richard_offline(
    store: &DataStore,
    _data_dir: &Path,
    s: &mut Value,
    email: &str,
    now: i64,
) -> f64 {
    let last_ts = s.get("lastTs").and_then(jsval::number).unwrap_or(0.0);
    let diff = now as f64 - last_ts;
    if diff < 15000.0 {
        return 0.0;
    }
    let offline_secs = (diff / 1000.0).min(43200.0); // 12h cap
    let is_premium = mitch_lib::auth::is_premium_email(store, email);
    let cash_per_sec = get_richard_power(s, true);
    let prem_mult = if is_premium { 2.0 } else { 1.0 };
    let efficiency = if is_premium { 1.0 } else { 0.5 };
    let offline_gain = cash_per_sec * prem_mult * offline_secs * efficiency;

    if offline_gain > 0.0 {
        let cash = s.get("cash").and_then(jsval::number).unwrap_or(0.0);
        set_num(s, "cash", (cash + offline_gain).min(SAFE_CAP));
        set_num(s, "lastTs", now as f64);
        return offline_gain;
    }
    set_num(s, "lastTs", now as f64);
    0.0
}

// ── Endpoint bodies ──────────────────────────────────────────────────────────

pub(crate) fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body: &Value,
    body_bytes: &[u8],
) -> Option<axum::response::Response> {
    match path {
        "/api/games/adrian-clicker/state" => Some(adrian_state(state, headers)),
        "/api/games/adrian-clicker/buy" if *method == Method::POST => {
            Some(adrian_buy(state, headers, body, body_bytes))
        }
        "/api/games/adrian-clicker/sync" if *method == Method::POST => {
            Some(adrian_sync(state, headers, body, body_bytes))
        }
        "/api/games/richard-riches/state" => Some(richard_state(state, headers)),
        "/api/games/richard-riches/sync" if *method == Method::POST => {
            Some(richard_sync(state, headers, body, body_bytes))
        }
        _ => None,
    }
}

fn js_body(value: &Value) -> axum::response::Response {
    json_resp_str(200, js_stringify(value))
}

/// `set_num` — `s.x = n` with an integral f64 kept JSON-integral (the session
/// Value round-trips through `js_stringify`, so `4.0` would print as `4.0`
/// in serde land; keep the Number i64 when exact).
fn set_num(obj: &mut Value, key: &str, n: f64) {
    if let Some(map) = obj.as_object_mut() {
        map.insert(key.to_string(), jsval::num_value(n));
    }
}

/// `(x).toExponential(2)` — JS style `1.23e+5` (Rust's `{:.2e}` drops the
/// `+`). Only used inside logCheat details / console strings.
fn js_to_exponential(x: f64, digits: usize) -> String {
    let s = format!("{x:.digits$e}");
    match s.split_once('e') {
        Some((mant, exp)) => {
            if let Some(stripped) = exp.strip_prefix('-') {
                format!("{mant}e-{stripped}")
            } else {
                format!("{mant}e+{exp}")
            }
        }
        None => s,
    }
}

fn default_clicker_session(now: i64) -> Value {
    json!({
        "points": 0, "coins": 0, "lastTs": now, "startTime": now,
        "playtimeCoins": 0, "upgrades": {}
    })
}

fn default_richard_session(now: i64) -> Value {
    json!({
        "cash": 0, "coins": 0, "lastTs": now, "startTime": now,
        "playtimeCoins": 0, "levels": {"lemon": 1}, "managers": {}, "upgrades": []
    })
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `GET /api/games/adrian-clicker/state` — server.js:22866-22885.
fn adrian_state(state: &Arc<AppState>, headers: &HeaderMap) -> axum::response::Response {
    let email = match crate::routes::games::games_email(state, headers) {
        Ok(e) => e,
        Err(resp) => return *resp,
    };
    let norm = mitch_lib::auth::normalize_email(&email);
    let now = now_millis();

    let existing = mitch_lib::minigames::mini_session_get(&state.store, &norm, "clicker");
    let (s, offline_gain) = match existing {
        None => {
            let s = default_clicker_session(now);
            mitch_lib::minigames::mini_session_set(&state.store, &norm, "clicker", &s);
            (s, 0.0)
        }
        Some(mut s) => {
            let gain = apply_adrian_offline(&state.store, state.data_dir(), &mut s, &email, now);
            if gain > 0.0 {
                mitch_lib::minigames::mini_session_set(&state.store, &norm, "clicker", &s);
            }
            (s, gain)
        }
    };

    // The gain is echoed in richard's response only; adrian's response shape
    // is { success, state, upgrades, tech }.
    let _ = offline_gain;
    js_body(&json!({
        "success": true,
        "state": s,
        "upgrades": adrian_upgrades_object(),
        "tech": adrian_tech_object(),
    }))
}

/// `POST /api/games/adrian-clicker/buy` — server.js:22887-22926.
fn adrian_buy(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    _body: &Value,
    body_bytes: &[u8],
) -> axum::response::Response {
    let Some(body) = crate::routes::me::parse_body_strict(body_bytes) else {
        return json_resp_str(400, js_stringify(&json!({ "success": false })));
    };
    let email = match crate::routes::games::games_email(state, headers) {
        Ok(e) => e,
        Err(resp) => return *resp,
    };
    let norm = mitch_lib::auth::normalize_email(&email);
    let Some(mut s) = mitch_lib::minigames::mini_session_get(&state.store, &norm, "clicker") else {
        return json_resp_str(400, js_stringify(&json!({ "success": false })));
    };

    // clientPoints = Number(body.points); if (Number.isFinite(clientPoints))
    let client_points = body.get("points").and_then(jsval::number);
    if let Some(cp) = client_points.filter(|v| v.is_finite()) {
        process_adrian_sync(state, &mut s, cp, &email, false, now_millis());
    }

    let up_id = body
        .get("id")
        .map(jsval::string)
        .unwrap_or_else(|| "undefined".to_string());
    let buy_amount = body
        .get("amount")
        .map(|v| parse_int_js(&jsval::string(v)))
        .unwrap_or(0.0)
        .max(1.0);

    let is_tech = adrian_tech(&up_id).is_some();
    let def = if is_tech {
        adrian_tech(&up_id)
    } else {
        adrian_upgrade(&up_id)
    };
    let Some(def) = def else {
        return json_resp_str(
            400,
            js_stringify(&json!({"success": false, "message": "Invalid upgrade"})),
        );
    };

    let count = s
        .get("upgrades")
        .and_then(|u| u.get(&up_id))
        .and_then(jsval::number)
        .unwrap_or(0.0);
    // `def.oneTime` — no ADRIAN_* definition carries it, so only isTech gates.
    if is_tech && count >= 1.0 {
        return json_resp_str(
            400,
            js_stringify(&json!({"success": false, "message": "Already owned"})),
        );
    }

    let points = s.get("points").and_then(jsval::number).unwrap_or(0.0);
    let mut total_cost = 0.0f64;
    let mut actual_buy_count = 0.0f64;

    if is_tech {
        // Math.floor(def.cost || def.baseCost) — JS truthiness fallback.
        let cost = def.get("cost").and_then(jsval::number).unwrap_or(0.0);
        total_cost = if cost != 0.0 && !cost.is_nan() {
            cost
        } else {
            def.get("baseCost").and_then(jsval::number).unwrap_or(0.0)
        }
        .floor();
        actual_buy_count = 1.0;
    } else {
        let base_cost = def.get("baseCost").and_then(jsval::number).unwrap_or(0.0);
        for i in 0..(buy_amount as u32) {
            let next_cost = (base_cost * 1.15f64.powf(count + i as f64)).floor();
            if total_cost + next_cost <= points {
                total_cost += next_cost;
                actual_buy_count += 1.0;
            } else {
                if i == 0 {
                    return json_resp_str(
                        400,
                        js_stringify(&json!({
                            "success": false,
                            "message": "Insufficient points",
                            "serverPoints": jsval::num_value(points),
                        })),
                    );
                }
                break;
            }
        }
    }

    if points < total_cost {
        return json_resp_str(
            400,
            js_stringify(&json!({"success": false, "message": "Insufficient points"})),
        );
    }

    set_num(&mut s, "points", points - total_cost);
    if let Some(upgrades) = s.get_mut("upgrades").and_then(|u| u.as_object_mut()) {
        upgrades.insert(up_id.clone(), jsval::num_value(count + actual_buy_count));
    }
    set_num(&mut s, "lastTs", now_millis() as f64);
    mitch_lib::minigames::mini_session_set(&state.store, &norm, "clicker", &s);

    json_resp_str(
        200,
        js_stringify(&json!({
            "success": true,
            "serverPoints": s.get("points").cloned().unwrap_or(json!(0)),
            "serverCoins": s.get("coins").cloned().unwrap_or(json!(0)),
            "count": s.get("upgrades").and_then(|u| u.get(&up_id)).cloned().unwrap_or(json!(0)),
        })),
    )
}

/// `POST /api/games/adrian-clicker/sync` — server.js:22928-22960.
fn adrian_sync(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    _body: &Value,
    body_bytes: &[u8],
) -> axum::response::Response {
    let Some(body) = crate::routes::me::parse_body_strict(body_bytes) else {
        return json_resp_str(400, js_stringify(&json!({ "success": false })));
    };
    let email = match crate::routes::games::games_email(state, headers) {
        Ok(e) => e,
        Err(resp) => return *resp,
    };
    let norm = mitch_lib::auth::normalize_email(&email);
    let now = now_millis();
    let mut s = mitch_lib::minigames::mini_session_get(&state.store, &norm, "clicker")
        .unwrap_or_else(|| default_clicker_session(now));

    let claim = body
        .get("claim")
        .map(|v| v == &json!(true) || v == &json!("true"))
        .unwrap_or(false);
    let client_points = body.get("points").and_then(jsval::number).unwrap_or(0.0);
    process_adrian_sync(state, &mut s, client_points, &email, claim, now);
    mitch_lib::minigames::mini_session_set(&state.store, &norm, "clicker", &s);

    json_resp_str(
        200,
        js_stringify(&json!({
            "success": true,
            "serverPoints": s.get("points").cloned().unwrap_or(json!(0)),
            "serverCoins": s.get("coins").cloned().unwrap_or(json!(0)),
        })),
    )
}

/// `GET /api/games/richard-riches/state` — server.js:22964-22986.
fn richard_state(state: &Arc<AppState>, headers: &HeaderMap) -> axum::response::Response {
    let email = match crate::routes::games::games_email(state, headers) {
        Ok(e) => e,
        Err(resp) => return *resp,
    };
    let norm = mitch_lib::auth::normalize_email(&email);
    let now = now_millis();

    let existing = mitch_lib::minigames::mini_session_get(&state.store, &norm, "richard");
    let (s, offline_gain) = match existing {
        None => {
            let s = default_richard_session(now);
            mitch_lib::minigames::mini_session_set(&state.store, &norm, "richard", &s);
            (s, 0.0)
        }
        Some(mut s) => {
            let gain = apply_richard_offline(&state.store, state.data_dir(), &mut s, &email, now);
            if gain > 0.0 {
                mitch_lib::minigames::mini_session_set(&state.store, &norm, "richard", &s);
            }
            (s, gain)
        }
    };

    json_resp_str(
        200,
        js_stringify(&json!({
            "success": true,
            "state": s,
            "offlineGain": jsval::num_value(offline_gain),
        })),
    )
}

/// `POST /api/games/richard-riches/sync` — server.js:22988-23056.
fn richard_sync(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    _body: &Value,
    body_bytes: &[u8],
) -> axum::response::Response {
    let Some(body) = crate::routes::me::parse_body_strict(body_bytes) else {
        return json_resp_str(400, js_stringify(&json!({ "success": false })));
    };
    let email = match crate::routes::games::games_email(state, headers) {
        Ok(e) => e,
        Err(resp) => return *resp,
    };
    let norm = mitch_lib::auth::normalize_email(&email);
    let now = now_millis();
    let mut s = mitch_lib::minigames::mini_session_get(&state.store, &norm, "richard")
        .unwrap_or_else(|| default_richard_session(now));

    // clientCash = Math.min(1e290, Number(body.state?.cash) || 0)
    let client_cash = body
        .get("state")
        .and_then(|st| st.get("cash"))
        .and_then(jsval::number)
        .unwrap_or(0.0)
        .min(1e290);

    let elapsed = (now as f64 - s.get("lastTs").and_then(jsval::number).unwrap_or(0.0)) / 1000.0;
    if elapsed > 0.0 {
        let is_premium = mitch_lib::auth::is_premium_email(&state.store, &email);
        let prem_mult = if is_premium { 2.0 } else { 1.0 };
        let cash_per_sec = get_richard_power(&s, false);
        let lemon_mult = if owned_upgrade(&s, "lemon_1") {
            3.0
        } else {
            1.0
        };
        let mut global_mult = 1.0;
        if owned_upgrade(&s, "all_1") {
            global_mult *= 2.0;
        }
        if owned_upgrade(&s, "all_2") {
            global_mult *= 3.0;
        }
        if owned_upgrade(&s, "all_3") {
            global_mult *= 5.0;
        }
        let lemon_lvl = s
            .get("levels")
            .and_then(|l| l.get("lemon"))
            .and_then(jsval::number)
            .unwrap_or(1.0);
        let lemon_revenue = lemon_lvl * 1.0 * lemon_mult * global_mult * prem_mult;
        let max_clicks_per_sec = 5.0;
        let max_possible_gain =
            ((cash_per_sec * elapsed) + (max_clicks_per_sec * lemon_revenue * elapsed)) * prem_mult;
        let cash = s.get("cash").and_then(jsval::number).unwrap_or(0.0);
        let actual_gain = client_cash - cash;

        if actual_gain > max_possible_gain * 3.0 && actual_gain > 100.0 {
            mitch_lib::admin::log_cheat(
                &state.store,
                state.data_dir(),
                &email,
                "Richard's Riches",
                &format!(
                    "Attempted impossible gain of {} cash (max possible: {})",
                    js_to_exponential(actual_gain, 2),
                    js_to_exponential(max_possible_gain, 2)
                ),
                "unknown",
            );
            set_num(&mut s, "cash", (cash + max_possible_gain).min(1e290));
        } else {
            set_num(&mut s, "cash", cash.max(client_cash).min(1e290));
        }
    }

    if body.get("state").map(jsval::truthy).unwrap_or(false) {
        let st = body.get("state").cloned().unwrap_or(json!({}));
        // s.levels = body.state.levels || s.levels (JS truthiness fallback).
        for key in ["levels", "managers", "upgrades"] {
            let incoming = st.get(key).cloned();
            let keep = s.get(key).cloned().unwrap_or(json!({}));
            let value = match incoming {
                Some(v) if jsval::truthy(&v) => v,
                _ => keep,
            };
            if let Some(map) = s.as_object_mut() {
                map.insert(key.to_string(), value);
            }
        }
    }

    let claim = body
        .get("claim")
        .map(|v| v == &json!(true) || v == &json!("true"))
        .unwrap_or(false);
    let mut coins_to_grant = 0.0f64;
    if claim {
        let mut coins = s.get("coins").and_then(jsval::number).unwrap_or(0.0);
        let mut cash = s.get("cash").and_then(jsval::number).unwrap_or(0.0);
        loop {
            let cost_of_next = (100.0 * 3f64.powf(coins)).floor();
            if cash >= cost_of_next {
                cash -= cost_of_next;
                coins += 1.0;
                coins_to_grant += 1.0;
            } else {
                break;
            }
        }
        set_num(&mut s, "coins", coins);
        set_num(&mut s, "cash", cash);
    }

    let start_time = s
        .get("startTime")
        .and_then(jsval::number)
        .unwrap_or(now as f64);
    let session_elapsed = now as f64
        - if start_time == 0.0 {
            now as f64
        } else {
            start_time
        };
    let total_playtime_coins = (session_elapsed / 600000.0).floor();
    let playtime_coins = s
        .get("playtimeCoins")
        .and_then(jsval::number)
        .unwrap_or(0.0);
    let pending_playtime_bonus = total_playtime_coins - playtime_coins;
    if pending_playtime_bonus > 0.0 {
        coins_to_grant += pending_playtime_bonus;
        set_num(
            &mut s,
            "playtimeCoins",
            playtime_coins + pending_playtime_bonus,
        );
    }

    if coins_to_grant > 0.0 {
        let is_premium = mitch_lib::auth::is_premium_email(&state.store, &email);
        let bonus_count = (if is_premium {
            coins_to_grant * 2.0
        } else {
            coins_to_grant
        }) * 100.0;
        mitch_lib::coins::add_coins(
            &state.store,
            state.cfg.data_dir.as_path(),
            &email,
            bonus_count,
            state.coin_multiplier(),
            "",
        );
        mitch_lib::achievements::update_stat(
            &state.store,
            state.data_dir(),
            &email,
            "richard_coins",
            bonus_count,
            state.coin_multiplier(),
        );
        tracing::info!(
            "[richard-riches] {email} earned {bonus_count} MitchCoins (Claim: {claim}, Playtime: {})",
            pending_playtime_bonus > 0.0
        );
    }

    set_num(&mut s, "lastTs", now as f64);
    mitch_lib::minigames::mini_session_set(&state.store, &norm, "richard", &s);

    json_resp_str(
        200,
        js_stringify(&json!({
            "success": true,
            "serverCash": s.get("cash").cloned().unwrap_or(json!(0)),
            "serverCoins": s.get("coins").cloned().unwrap_or(json!(0)),
            "globalCoins": jsval::num_value(mitch_lib::coins::get_coins(
                &state.store,
                state.data_dir(),
                &email,
            )),
        })),
    )
}

fn owned_upgrade(s: &Value, id: &str) -> bool {
    match s.get("upgrades") {
        Some(Value::Array(a)) => a.iter().any(|v| jsval::string(v) == id),
        Some(Value::Object(o)) => o.contains_key(id),
        _ => false,
    }
}


/// `parseInt(s, 10) || 0`-shaped decimal prefix parse (JS parseInt on the
/// String() of the value; "12.7abc" → 12, "" → NaN → caller's || 1).
pub(crate) fn parse_int_js(s: &str) -> f64 {
    let t = s.trim_start();
    let (sign, t) = match t.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, t.strip_prefix('+').unwrap_or(t)),
    };
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return f64::NAN;
    }
    sign * digits.parse::<f64>().unwrap_or(f64::NAN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_tables_match_js_shapes() {
        // Costs serialize in JS form through js_stringify (1e+22 style).
        let upgrades = adrian_upgrades_object();
        let s = mitch_lib::data::js_stringify(&upgrades);
        assert!(s.contains(r#""baseCost":2.6e+22"#), "{s}");
        assert!(s.contains(r#""baseCost":14000000000000"#), "{s}");
        assert!(s.contains(r#""baseCost":1e+260"#), "{s}");
        assert!(s.contains(r#""power":0.1"#), "{s}");
        // tech echo
        let tech = adrian_tech_object();
        let s = mitch_lib::data::js_stringify(&tech);
        assert!(s.contains(r#""cost":1e+85"#), "{s}");
        assert!(s.contains(r#""cost":1000000000000000"#), "{s}"); // 1e15
        assert!(s.contains(r#""cost":5000000000000000"#), "{s}"); // 5e15
                                                                  // richard businesses
        let s = mitch_lib::data::js_stringify(&Value::Object(
            RICHARD_BUSINESSES
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        ));
        assert!(s.contains(r#""baseCost":25798901760"#), "{s}");
        assert!(s.contains(r#""baseSpeed":0.6"#), "{s}");
    }

    #[test]
    fn adrian_power_math() {
        let upgrades = json!({"desk": 2, "chromebook": 3, "desk_1": 1, "mitch_1": 1});
        let (click, auto) = get_adrian_power(&upgrades);
        // desk_1 → buildMults.desk = 2; mitch_1 → global ×2.
        // click = (1 + 2*0.1*2) * 2 = 2.8; chromebook has no tech → auto = 3*2 = 6.
        assert!((click - 2.8).abs() < 1e-12, "{click}");
        assert!((auto - 6.0).abs() < 1e-12, "{auto}");
    }

    #[test]
    fn richard_speed_and_power() {
        assert!((get_richard_speed(10.0, 0.6) - 0.6).abs() < 1e-12);
        assert!((get_richard_speed(25.0, 0.6) - 0.3).abs() < 1e-12);
        assert!((get_richard_speed(100.0, 3.0) - 0.375).abs() < 1e-12); // 3 milestones → /8
        assert!((get_richard_speed(5000.0, 3.0) - 0.05).abs() < 1e-12); // floor at 0.05
        let s = json!({
            "levels": {"lemon": 25, "news": 1},
            "managers": {"lemon": true},
            "upgrades": ["lemon_1", "all_1"]
        });
        // lemon: 25*1*3*2 / max(0.05, 0.6/2=0.3) = 150/0.3 = 500
        // news: 1*60*1*2 / 3 = 40  (manager required but missing → skipped)
        assert!((get_richard_power(&s, true) - 500.0).abs() < 1e-9);
        assert!((get_richard_power(&s, false) - 540.0).abs() < 1e-9);
    }

    #[test]
    fn parse_int_semantics() {
        assert_eq!(parse_int_js("12.7abc"), 12.0);
        assert_eq!(parse_int_js("42"), 42.0);
        assert_eq!(parse_int_js("-7"), -7.0);
        assert_eq!(parse_int_js("  9"), 9.0);
        assert!(parse_int_js("").is_nan());
        assert!(parse_int_js("abc").is_nan());
        assert_eq!(parse_int_js("007"), 7.0);
    }

    #[test]
    fn to_exponential_matches_js() {
        // (1.23e5).toExponential(2) === "1.23e+5"
        assert_eq!(js_to_exponential(123000.0, 2), "1.23e+5");
        assert_eq!(js_to_exponential(1.5e-7, 2), "1.50e-7");
        assert_eq!(js_to_exponential(0.0, 2), "0.00e+0");
    }
}
