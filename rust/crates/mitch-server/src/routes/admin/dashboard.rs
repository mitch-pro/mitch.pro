//! Admin dashboards & status — read-only endpoints:
//! resources (10490), app-logs (10503), docker-logs (10517), search-logs
//! (10567), live-feeds (10575), casino/rtp (10482), advanced-data (15736),
//! maintenance-status (15507), shop/catalog (15516), moderators GET +
//! moderator-panel GET are in moderation.rs.

use super::{forbidden, AdminCtx, Resp};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    _headers: &HeaderMap,
    search: &str,
    ctx: &AdminCtx,
) -> Resp {
    // GET /api/admin/resources — process stats.
    if path == "/api/admin/resources" {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        return Some(resources());
    }

    // GET /api/admin/app-logs (admins only).
    if path == "/api/admin/app-logs" && *method == Method::GET {
        if !ctx.is_admin(state) {
            return Some(forbidden());
        }
        return Some(app_logs(state, search));
    }

    // GET /api/admin/docker-logs (admins only).
    if path == "/api/admin/docker-logs" && *method == Method::GET {
        if !ctx.is_admin(state) {
            return Some(forbidden());
        }
        return Some(docker_logs(search));
    }

    // GET /api/admin/search-logs.
    if path == "/api/admin/search-logs" {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        let logs = state
            .store
            .read_document(&state.cfg.data_dir.join("search_intent.json"), json!([]));
        return Some(json_response(200, json!({ "logs": logs })));
    }

    // GET /api/admin/live-feeds — traffic is always empty; bets from the
    // casino bettingFeed (in-process, empty until Step 12).
    if path == "/api/admin/live-feeds" {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        return Some(json_response(200, json!({ "traffic": [], "bets": [] })));
    }

    // GET /api/admin/casino/rtp.
    if path == "/api/admin/casino/rtp" {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        return Some(json_response(
            200,
            json!({
                "intake": f64::from_bits(state.casino_intake.load(std::sync::atomic::Ordering::Relaxed)),
                "payout": f64::from_bits(state.casino_payout.load(std::sync::atomic::Ordering::Relaxed)),
            }),
        ));
    }

    // GET /api/admin/advanced-data.
    if path == "/api/admin/advanced-data" {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &ctx.email(state),
            if ctx.is_admin(state) {
                "view_admin_tools"
            } else {
                "moderator_view_admin_tools"
            },
            json!({}),
        );
        return Some(json_response(
            200,
            mitch_lib::admin::build_advanced_admin_data(&state.store, &state.cfg.data_dir),
        ));
    }

    // GET /api/admin/maintenance-status.
    if path == "/api/admin/maintenance-status" {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        return Some(json_response(
            200,
            json!({ "success": true, "active": state.soft_maintenance_active() }),
        ));
    }

    // GET /api/admin/shop/catalog.
    if path == "/api/admin/shop/catalog" {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        let catalog = state
            .store
            .read_document(&state.cfg.data_dir.join("shop_catalog.json"), json!(null));
        let catalog = match catalog {
            Value::Array(items) if !items.is_empty() => items,
            _ => default_shop_catalog(),
        };
        let catalog: Vec<Value> = catalog
            .into_iter()
            .filter(|item| item.get("id").and_then(|v| v.as_str()) != Some("og_badge"))
            .collect();
        return Some(json_response(
            200,
            json!({ "success": true, "catalog": catalog }),
        ));
    }

    None
}

fn json_response(code: u16, obj: Value) -> Response {
    crate::errors::json_resp(code, obj)
}

/// `GET /api/admin/resources` — server.js:10491-10501. Memory/uptime/load in
/// the same keys as the JS (`process.memoryUsage()` field names).
fn resources() -> Response {
    json_response(
        200,
        json!({
            "memory": {
                "rss": rss_bytes(),
                "heapUsed": 0,
                "heapTotal": 0,
                "external": 0,
            },
            "uptime": uptime_seconds(),
            "load": load_avg(),
        }),
    )
}

fn rss_bytes() -> f64 {
    if let Ok(statm) = std::fs::read_to_string("/proc/self/statm") {
        let rss_pages = statm
            .split_whitespace()
            .nth(1)
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0);
        return rss_pages * page_size();
    }
    0.0
}

fn page_size() -> f64 {
    4096.0
}

fn uptime_seconds() -> f64 {
    if let Ok(boot) = std::fs::read_to_string("/proc/uptime") {
        return boot
            .split_whitespace()
            .next()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0);
    }
    0.0
}

fn load_avg() -> [f64; 3] {
    if let Ok(stat) = std::fs::read_to_string("/proc/loadavg") {
        let vals: Vec<f64> = stat
            .split_whitespace()
            .take(3)
            .filter_map(|v| v.parse().ok())
            .collect();
        if vals.len() == 3 {
            return [vals[0], vals[1], vals[2]];
        }
    }
    [0.0, 0.0, 0.0]
}

/// `GET /api/admin/app-logs` — server.js:10503-10515.
fn app_logs(state: &Arc<AppState>, search: &str) -> Response {
    let params = parse_query(search);
    let level = params.get("level").cloned().unwrap_or_else(|| "all".into());
    let category = params
        .get("category")
        .cloned()
        .unwrap_or_else(|| "all".into());
    let q = params.get("search").cloned().unwrap_or_default();
    let limit: usize = params
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let rows = state
        .store
        .query_app_logs_sync(&level, &category, &q, limit)
        .unwrap_or_default();
    let categories = state.store.app_log_categories_sync().unwrap_or_default();
    let limit_clean = limit.clamp(25, 2000);
    json_response(
        200,
        json!({
            "ok": true,
            "logs": rows.iter().map(|r| json!({
                "ts": r.ts,
                "level": r.level,
                "category": r.category,
                "message": r.message,
                "details": r.details,
            })).collect::<Vec<_>>(),
            "categories": categories.iter().map(|(c, n)| json!({
                "category": c, "count": n,
            })).collect::<Vec<_>>(),
            "limit": limit_clean,
        }),
    )
}

fn parse_query(search: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let query = search.trim_start_matches('?');
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        out.insert(percent_decode(k), percent_decode(v));
    }
    out
}

/// `GET /api/admin/docker-logs` — `docker compose logs` on the repo root.
fn docker_logs(search: &str) -> Response {
    const SERVICES: &[&str] = &[
        "all",
        "reverse-proxy",
        "webserver-blue",
        "webserver-green",
        "proxy",
        "ipserver",
    ];
    let params = parse_query(search);
    let service = params
        .get("service")
        .cloned()
        .unwrap_or_else(|| "all".into());
    let raw_tail = params.get("tail").and_then(|v| v.parse::<f64>().ok());
    let tail = raw_tail
        .map(|t| t.floor() as i64)
        .unwrap_or(200)
        .clamp(25, 2000);
    if !SERVICES.contains(&service.as_str()) {
        return json_response(400, json!({ "error": "invalid_service" }));
    }
    let tail_str = tail.to_string();
    let mut args: Vec<&str> = vec![
        "compose",
        "logs",
        "--no-color",
        "--timestamps",
        "--tail",
        &tail_str,
    ];
    if service != "all" {
        args.push(&service);
    }
    let output = std::process::Command::new("docker")
        .args(&args)
        .current_dir(repo_root())
        .output();
    match output {
        Ok(out) if out.status.success() => json_response(
            200,
            json!({
                "ok": true,
                "service": service,
                "tail": tail,
                "output": String::from_utf8_lossy(&out.stdout),
                "stderr": String::from_utf8_lossy(&out.stderr),
            }),
        ),
        Ok(out) => {
            let combined = if out.stderr.is_empty() {
                String::from_utf8_lossy(&out.stdout).to_string()
            } else {
                String::from_utf8_lossy(&out.stderr).to_string()
            };
            json_response(
                502,
                json!({
                    "error": "docker_logs_failed",
                    "message": combined.chars().take(2000).collect::<String>(),
                    "service": service,
                    "tail": tail,
                }),
            )
        }
        Err(e) => json_response(
            502,
            json!({
                "error": "docker_unavailable",
                "message": e.to_string(),
                "service": service,
                "tail": tail,
            }),
        ),
    }
}

/// Repo root (the JS uses `BASE`).
pub fn repo_root() -> std::path::PathBuf {
    crate::hosts::SiteConfig::load().base_dir
}

/// `DEFAULT_SHOP_CATALOG` — verbatim from server.js:7027-7073 (used when no
/// shop_catalog.json override exists).
pub fn default_shop_catalog() -> Vec<Value> {
    let items: &[(&str, &str, &str, &str, &str, i64)] = &[
        ("premium", "Premium", "Premium", "premium", "Unlock Premium Chat, 2X typing/logic coin rewards, 2X Clicker/Riches offline gains, larger canvas brushes, exclusive profile frames/badges, and the epic chance to have an arcade game named after you!", 5000),
        ("neon_purple", "Neon Purple Name", "Name Colors", "cosmetic", "A bright purple username for chat, profiles, and leaderboards.", 500),
        ("electric_blue", "Electric Blue Name", "Name Colors", "cosmetic", "A sharp electric-blue username style.", 500),
        ("mint_flash", "Mint Flash Name", "Name Colors", "cosmetic", "A clean mint username with a fresh glow.", 550),
        ("rose_spark", "Rose Spark Name", "Name Colors", "cosmetic", "A warm rose username style with a soft highlight.", 550),
        ("ember_red", "Ember Red Name", "Name Colors", "cosmetic", "A deep red username with a bolder presence.", 650),
        ("void_white", "Void White Name", "Name Colors", "cosmetic", "A high-contrast white username for dark pages.", 750),
        ("gold_glow", "Golden Glow Name", "Name Colors", "cosmetic", "A premium gold username glow.", 900),
        ("rainbow_name", "Rainbow Name", "Name Colors", "cosmetic", "An admin-only animated rainbow username.", 2500),
        ("verified_badge", "Verified Badge", "Badges", "cosmetic", "Adds a verified check badge beside your name.", 1000),
        ("artist_badge", "Artist Badge", "Badges", "cosmetic", "A badge for canvas builders and pixel artists.", 900),
        ("chess_badge", "Chess Badge", "Badges", "cosmetic", "A badge for chess regulars.", 900),
        ("builder_badge", "Builder Badge", "Badges", "cosmetic", "A badge for people who help build the community.", 950),
        ("lucky_badge", "Lucky Badge", "Badges", "cosmetic", "A rare-feeling badge for casino winners.", 1200),
        ("premium_star_badge", "Premium Star Badge", "Badges", "cosmetic", "A premium star badge for your profile and chats.", 1400),
        ("owner_fan_badge", "Mitch Fan Badge", "Badges", "cosmetic", "A simple badge for fans of the site.", 800),
        ("chat_sparkles", "Chat Sparkles", "Chat Effects", "cosmetic", "Adds a subtle sparkle effect to your chat identity.", 850),
        ("chat_shadow", "Chat Shadow", "Chat Effects", "cosmetic", "Adds a dark shadow accent to your chat identity.", 850),
        ("chat_wave", "Chat Wave", "Chat Effects", "cosmetic", "A gentle animated wave effect for your chat name.", 1000),
        ("chat_terminal", "Terminal Chat Style", "Chat Effects", "cosmetic", "A monospace terminal-style chat accent.", 1100),
        ("chat_prism", "Prism Chat Style", "Chat Effects", "cosmetic", "A premium prism accent for chat.", 1600),
        ("profile_grid", "Profile Grid Background", "Profile Effects", "cosmetic", "Adds a clean grid effect to your profile.", 900),
        ("profile_stars", "Profile Starfield", "Profile Effects", "cosmetic", "Adds a starfield-style profile effect.", 1200),
        ("profile_scanlines", "Profile Scanlines", "Profile Effects", "cosmetic", "Adds a retro scanline texture to your profile.", 950),
        ("profile_gold_frame", "Gold Profile Frame", "Profile Effects", "cosmetic", "A premium gold frame accent for your profile.", 1800),
        ("profile_neon_frame", "Neon Profile Frame", "Profile Effects", "cosmetic", "A premium neon frame accent for your profile.", 1800),
        ("focus_theme", "Focus Theme", "Site Themes", "cosmetic", "A calm, low-distraction site accent.", 700),
        ("arcade_theme", "Arcade Theme", "Site Themes", "cosmetic", "A brighter arcade-style site accent.", 900),
        ("midnight_theme", "Midnight Theme", "Site Themes", "cosmetic", "A darker midnight accent for the site.", 900),
        ("vip_pass", "VIP Casino Pass (24h)", "Passes", "pass", "Unlocks unlimited max bet amount in all casino games for 24 hours.", 250),
        ("canvas_lock_pass", "Canvas Lock Pass", "Passes", "cosmetic", "Unlocks a saved canvas-tool preference toggle.", 1200),
        ("quick_access_pass", "Quick Access Pass", "Passes", "cosmetic", "Unlocks a quick-access preference toggle.", 800),
        ("daily_bonus_plus", "Daily Bonus Plus", "Passes", "cosmetic", "Unlocks a premium daily-bonus preference toggle.", 1500),
    ];
    // JS items carry optional costType/premiumOnly/adminOnly extras; encode the
    // full variety via a second table.
    let extras: &[(&str, &str, bool, bool)] = &[
        ("premium", "", false, false),
        ("neon_purple", "name_color", false, false),
        ("electric_blue", "name_color", false, false),
        ("mint_flash", "name_color", false, false),
        ("rose_spark", "name_color", false, false),
        ("ember_red", "name_color", false, false),
        ("void_white", "name_color", false, false),
        ("gold_glow", "name_color", true, false),
        ("rainbow_name", "name_color", false, true),
        ("verified_badge", "chat_badge", false, false),
        ("artist_badge", "chat_badge", false, false),
        ("chess_badge", "chat_badge", false, false),
        ("builder_badge", "chat_badge", false, false),
        ("lucky_badge", "chat_badge", false, false),
        ("premium_star_badge", "chat_badge", true, false),
        ("owner_fan_badge", "chat_badge", false, false),
        ("chat_sparkles", "chat_effect", false, false),
        ("chat_shadow", "chat_effect", false, false),
        ("chat_wave", "chat_effect", false, false),
        ("chat_terminal", "chat_effect", false, false),
        ("chat_prism", "chat_effect", true, false),
        ("profile_grid", "profile_effect", false, false),
        ("profile_stars", "profile_effect", false, false),
        ("profile_scanlines", "profile_effect", false, false),
        ("profile_gold_frame", "profile_effect", true, false),
        ("profile_neon_frame", "profile_effect", true, false),
        ("focus_theme", "site_theme", false, false),
        ("arcade_theme", "site_theme", false, false),
        ("midnight_theme", "site_theme", false, false),
        ("vip_pass", "vip_casino_pass", false, false),
        ("canvas_lock_pass", "canvas_tool", false, false),
        ("quick_access_pass", "canvas_tool", false, false),
        ("daily_bonus_plus", "canvas_tool", true, false),
    ];
    items
        .iter()
        .zip(extras.iter())
        .map(
            |((id, name, section, typ, desc, cost), (id2, cost_type, premium_only, admin_only))| {
                debug_assert_eq!(id, id2, "catalog id mismatch");
                let mut item = json!({
                    "id": id, "name": name, "section": section, "type": typ,
                    "cost": cost, "desc": desc,
                });
                let obj = item.as_object_mut();
                if let Some(obj) = obj {
                    if !cost_type.is_empty() {
                        obj.insert("costType".into(), json!(cost_type));
                    }
                    if *premium_only {
                        obj.insert("premiumOnly".into(), json!(true));
                    }
                    if *admin_only {
                        obj.insert("adminOnly".into(), json!(true));
                    }
                }
                item
            },
        )
        .collect()
}
/// Minimal `+`-safe percent decoding for query strings.
fn percent_decode(s: &str) -> String {
    let plus_fixed = s.replace('+', " ");
    let bytes = plus_fixed.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let _hex = &plus_fixed[i + 1..i + 3];
            if let Ok(byte) = u8::from_str_radix(&plus_fixed[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
