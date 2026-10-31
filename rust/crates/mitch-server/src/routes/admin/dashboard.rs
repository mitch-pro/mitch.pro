//! Admin dashboards & status — read-only endpoints:
//! resources (10490), app-logs (10503), docker-logs (10517), search-logs
//! (10567), live-feeds (10575), advanced-data (15736),
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

    // GET /api/admin/live-feeds — traffic is always empty.
    if path == "/api/admin/live-feeds" {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        return Some(json_response(200, json!({ "traffic": [] })));
    }

    // GET /api/admin/advanced-data.
    if path == "/api/admin/advanced-data" {
        if !ctx.is_any_admin(state) {
            return Some(forbidden());
        }
        // Only log full-admin views as an activity-log entry. Moderators
        // open this panel routinely to check the moderation queue, and a
        // view isn't a privileged change (the section's own "privileged
        // changes" tag), so logging it just buried real actions in noise.
        if ctx.is_admin(state) {
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                &ctx.email(state),
                "view_admin_tools",
                json!({}),
            );
        }
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
        // Delegate to the same loader /api/shop/items uses, rather than a
        // second hand-maintained catalog literal: a drifted-out-of-sync
        // duplicate here is exactly how a retired item (the casino buffs
        // this view used to still list) stays visible to admins, or worse
        // gets resaved to shop_catalog.json, after it's gone everywhere
        // else.
        let catalog = mitch_lib::shop::load_shop_catalog(&state.store, &state.cfg.data_dir);
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
            "totalMem": total_mem_bytes(),
            "cpus": cpu_count(),
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

fn total_mem_bytes() -> f64 {
    if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
        for line in meminfo.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                if let Some(kb) = rest
                    .split_whitespace()
                    .next()
                    .and_then(|v| v.parse::<f64>().ok())
                {
                    return kb * 1024.0;
                }
            }
        }
    }
    0.0
}

fn cpu_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
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
    const SERVICES: &[&str] = &["all", "reverse-proxy", "webserver-blue", "webserver-green"];
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
