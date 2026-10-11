//! Canvas painting endpoints (server.js:21752-22034) — `/api/canvas/pixel`,
//! `/api/canvas/pixels/bulk`, `/api/canvas/erase`, and the admin trio
//! admin-erase/admin-ban/admin-unban.
//!
//! Parity notes:
//! - The in-handler `checkRateLimit` calls are no-ops in JS (the prelude
//!   stamps `req._rateLimitChecked`), so the bypass-cooldown branches below it
//!   are dead code and are not ported.
//! - `broadcastCanvasDelta` fan-outs land with Step 11 (WS presence); the
//!   changedPixels/changedChunks bookkeeping only feeds them and is skipped.
//! - Guard order matches the JS ladder exactly: zone access → brush cap →
//!   missing fields → color regex → bounds → ban → premium color.

use super::reads::canvas_sid;
use super::state_core;
use crate::routes::me::{json_response, parse_body_strict};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::auth::normalize_email;
use mitch_lib::jsval;
use serde_json::{json, Map, Value};
use std::sync::Arc;

/// `isAnyAdminId(sid)` with the JS falsy-sid short-circuit made explicit.
pub(crate) fn admin_ok(state: &Arc<AppState>, sid: &str) -> bool {
    if sid.is_empty() {
        return false;
    }
    let node_env_test = std::env::var("NODE_ENV").unwrap_or_default() == "test";
    mitch_lib::auth::is_any_admin_id(&state.store, &state.id_secret, sid, node_env_test)
}

/// `Number(body.brushSz) || 1`, clamped `Math.min(50, Math.max(1, …))` —
/// JS falsy coerces 0/NaN to the 1 default before the clamp.
fn brush_size(v: Option<&Value>) -> f64 {
    let n = v.and_then(jsval::number).unwrap_or(f64::NAN);
    let n = if n.is_nan() || n == 0.0 { 1.0 } else { n };
    n.clamp(1.0, 50.0)
}

/// `Number.isInteger`.
fn is_integer(n: f64) -> bool {
    n.is_finite() && n.fract() == 0.0
}

/// `/^#[0-9a-fA-F]{6}$/.test(color)`.
fn valid_color(color: &str) -> bool {
    color.len() == 7 && color.starts_with('#') && color[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// A template-literal of a raw body value: `String(v)` for primitives, array
/// elements comma-joined, objects as `[object Object]`, missing as
/// `undefined` (JS `` `${undefined}` ``).
fn js_template(v: Option<&Value>) -> String {
    match v {
        None => "undefined".to_string(),
        Some(Value::Null) => "null".to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => jsval::string(&Value::Number(n.clone())),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|el| js_template(Some(el)))
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_string(),
    }
}

/// `Number(p?.x)` — a missing/non-object owner coerces to NaN.
fn num_field(p: Option<&Map<String, Value>>, key: &str) -> f64 {
    p.and_then(|o| o.get(key))
        .and_then(jsval::number)
        .unwrap_or(f64::NAN)
}

/// `String(p?.color || '')` — falsy falls to ''.
fn str_field(p: Option<&Map<String, Value>>, key: &str) -> String {
    p.and_then(|o| o.get(key))
        .filter(|v| jsval::truthy(v))
        .map(jsval::string)
        .unwrap_or_default()
}

fn now_ms() -> i64 {
    mitch_lib::school::now_millis()
}

/// `/api/canvas/pixel` POST (server.js:21852-22034).
pub fn pixel_post(state: &Arc<AppState>, headers: &HeaderMap, body: &Value) -> Response {
    let sid = canvas_sid(state, headers);
    let email =
        mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid).unwrap_or_default();
    let admin = admin_ok(state, &sid);
    let premium = !email.is_empty() && mitch_lib::auth::is_premium_email(&state.store, &email);

    // JS `typeof x !== 'number'` — no string coercion here.
    let x = body.get("x").and_then(|v| v.as_f64());
    let y = body.get("y").and_then(|v| v.as_f64());
    let color = body.get("color").cloned().unwrap_or(Value::Null);
    let painter = body.get("painter").cloned().unwrap_or(Value::Null);
    let zone_v = body.get("zoneId").cloned().unwrap_or(Value::Null);
    let has_zone = jsval::truthy(&zone_v);
    let zone_key = jsval::string(&zone_v);
    let zone_opt = if has_zone {
        Some(zone_key.as_str())
    } else {
        None
    };
    let brush_sz = brush_size(body.get("brushSz"));

    if has_zone
        && !state_core::check_zone_access(
            &state.store,
            state.data_dir(),
            &state.id_secret,
            std::env::var("NODE_ENV").unwrap_or_default() == "test",
            &zone_key,
            &email,
            &sid,
        )
    {
        return json_response(
            403,
            json!({ "error": "forbidden", "reason": "No access to this zone" }),
        );
    }

    // Zone members get full brush size (rate limits are the prelude's).
    let max_allowed_brush = if admin || has_zone {
        50.0
    } else if premium {
        16.0
    } else {
        8.0
    };
    if brush_sz > max_allowed_brush {
        return json_response(
            403,
            json!({ "error": "forbidden", "reason": "brush size too large" }),
        );
    }

    // `const rl = checkRateLimit(req, path); if (rl …)` — a no-op in JS and
    // covered by the prelude limiter here; the bypass-cooldown branch under it
    // is unreachable dead code.

    let Some((x, y)) = x.zip(y) else {
        return json_response(400, json!({ "error": "missing fields" }));
    };
    if !jsval::truthy(&color) || !jsval::truthy(&painter) {
        return json_response(400, json!({ "error": "missing fields" }));
    }
    let color_str = jsval::string(&color);
    if !valid_color(&color_str) {
        return json_response(400, json!({ "error": "invalid color" }));
    }
    // NaN passes in JS too (Math.abs(NaN) > 500000 is false).
    if x.abs() > 500_000.0 || y.abs() > 500_000.0 {
        return json_response(400, json!({ "error": "out of bounds" }));
    }
    if !admin {
        let banned = state.canvas.bans.read().unwrap_or_else(|e| e.into_inner());
        if let Some(reason) = banned
            .get(&jsval::string(&painter))
            .filter(|v| jsval::truthy(v))
        {
            let mut err = Map::new();
            err.insert("error".into(), json!("banned"));
            if let Some(r) = reason.get("reason").filter(|v| jsval::truthy(v)) {
                err.insert("reason".into(), r.clone());
            }
            return json_response(403, Value::Object(err));
        }
    }
    if !admin && !premium && state_core::PREMIUM_COLORS.contains(&color_str.to_lowercase().as_str())
    {
        return json_response(403, json!({ "error": "premium_color" }));
    }

    let half = (brush_sz / 2.0).floor();
    let mut to_paint: Vec<(f64, f64, String)> = Vec::new();
    {
        let pixels = state
            .canvas
            .pixels
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let locks = state.canvas.locks.read().unwrap_or_else(|e| e.into_inner());
        let mut expired_keys: Vec<String> = Vec::new();
        // JS `for (let bx = 0; bx < brushSz; bx++)` — exactly ceil(brushSz)
        // iterations (brushSz ≥ 1), which also handles a fractional brush.
        let steps = brush_sz.ceil() as i64;
        for i in 0..steps {
            for j in 0..steps {
                let px = x - half + i as f64;
                let py = y - half + j as f64;
                let pkey = state_core::pixel_key(px, py);
                if px.abs() > 500_000.0 || py.abs() > 500_000.0 {
                    continue;
                }
                if has_zone {
                    to_paint.push((px, py, pkey));
                    continue;
                }
                if !admin {
                    if let Some(lock) = locks.get(&pkey).filter(|v| jsval::truthy(v)) {
                        let lock_email = lock
                            .get("email")
                            .filter(|v| jsval::truthy(v))
                            .map(jsval::string)
                            .unwrap_or_default();
                        let is_lock_owner = (!email.is_empty()
                            && normalize_email(&lock_email) == normalize_email(&email))
                            || lock.get("painter") == Some(&painter);
                        let expired = now_ms() as f64
                            > lock
                                .get("expiresAt")
                                .and_then(jsval::number)
                                .unwrap_or(f64::NAN);
                        if !expired && !is_lock_owner {
                            continue;
                        }
                        if expired {
                            expired_keys.push(pkey.clone());
                        }
                    }
                    if let Some(existing) = pixels.get(&pkey).filter(|v| jsval::truthy(v)) {
                        let is_own = existing.get("painter") == Some(&painter)
                            || (!email.is_empty() && existing.get("email") == Some(&json!(email)));
                        if !premium || !is_own {
                            continue;
                        }
                    }
                }
                to_paint.push((px, py, pkey));
            }
        }
        if !expired_keys.is_empty() {
            drop(pixels);
            drop(locks);
            let mut locks = state
                .canvas
                .locks
                .write()
                .unwrap_or_else(|e| e.into_inner());
            for k in expired_keys {
                locks.remove(&k);
            }
        }
    }

    // Shadow-banned painters get a shadow success before any paint lands.
    if !email.is_empty()
        && state
            .shadow_bans
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&normalize_email(&email))
    {
        return json_response(200, json!({ "ok": true }));
    }

    // Key order matters for persisted byte parity: color, painter, ts,
    // email?, premium?, admin?.
    let now = now_ms();
    let mut pixel_data = Map::new();
    pixel_data.insert("color".into(), color.clone());
    pixel_data.insert("painter".into(), painter.clone());
    pixel_data.insert("ts".into(), json!(now));
    if !email.is_empty() {
        pixel_data.insert("email".into(), json!(email));
    }
    if premium {
        pixel_data.insert("premium".into(), json!(true));
    }
    if admin {
        pixel_data.insert("admin".into(), json!(true));
    }
    let pixel_data = Value::Object(pixel_data);

    // changedPixels/changedChunks (server.js:21935-21941) — the paint loop's
    // bookkeeping, now feeding the canvas_delta WS broadcast below.
    let mut changed_pixels: Vec<Value> = Vec::new();
    let mut changed_chunks: Vec<String> = Vec::new();

    let center_key = state_core::pixel_key(x, y);
    for (px, py, pkey) in &to_paint {
        state.canvas.set_pixel(
            &state.store,
            state.data_dir(),
            *px,
            *py,
            pixel_data.clone(),
            zone_opt,
        );
        changed_pixels.push(json!({
            "x": jsval::num_value(*px),
            "y": jsval::num_value(*py),
            "data": pixel_data.clone(),
        }));
        let ck = format!(
            "{},{}",
            (px / 64.0).floor() as i64,
            (py / 64.0).floor() as i64
        );
        if !changed_chunks.contains(&ck) {
            changed_chunks.push(ck);
        }
        if !has_zone {
            state.canvas.heatmap_set(pkey, now);
            // body.lock + the exact center pixel + premium: the weekly lock
            // budget ladder (server.js:21993-22011).
            if jsval::truthy(&body.get("lock").cloned().unwrap_or(Value::Null))
                && *pkey == center_key
                && premium
                && !email.is_empty()
            {
                let norm = normalize_email(&email);
                let mut entry = mitch_lib::user_stats::user_stats_get(&state.store, &norm);
                if !entry.is_object() {
                    entry = json!({});
                }
                let now_f = now as f64;
                let last_reset = entry
                    .get("last_lock_reset")
                    .and_then(jsval::number)
                    .unwrap_or(0.0);
                if now_f - last_reset > 7.0 * 86_400_000.0 {
                    if let Some(obj) = entry.as_object_mut() {
                        obj.insert("last_lock_reset".into(), json!(now_f));
                        obj.insert("week_locks".into(), json!(0));
                    }
                }
                let week_locks = entry
                    .get("week_locks")
                    .and_then(jsval::number)
                    .unwrap_or(0.0);
                if week_locks < 16.0 {
                    if let Some(obj) = entry.as_object_mut() {
                        obj.insert("week_locks".into(), json!(week_locks + 1.0));
                    }
                    mitch_lib::user_stats::user_stats_set(&state.store, &norm, &entry);
                    let mut locks = state
                        .canvas
                        .locks
                        .write()
                        .unwrap_or_else(|e| e.into_inner());
                    locks.insert(
                        pkey.clone(),
                        json!({ "email": email, "painter": painter, "expiresAt": now + 86_400_000 }),
                    );
                    let _ = state.store.write_document(
                        &state_core::canvas_locks_file(state.data_dir()),
                        &Value::Object(locks.clone()),
                    );
                }
            }
        }
    }

    if has_zone {
        state
            .canvas
            .save_zone_pixels(&state.store, state.data_dir(), &zone_key);
    } else if !email.is_empty() {
        mitch_lib::achievements::add_painting_coin(
            &state.store,
            state.data_dir(),
            &email,
            state.coin_multiplier(),
        );
    }
    // canvas_delta broadcast (server.js:21969-21975).
    if !changed_pixels.is_empty() {
        if changed_pixels.len() > 300 {
            super::broadcast_canvas_delta(
                state,
                "invalidate",
                zone_opt,
                "chunks",
                json!(changed_chunks),
            );
        } else {
            super::broadcast_canvas_delta(state, "set", zone_opt, "pixels", json!(changed_pixels));
        }
    }
    json_response(200, json!({ "ok": true }))
}

/// `/api/canvas/pixels/bulk` POST (server.js:21797-21850).
pub fn bulk_post(state: &Arc<AppState>, headers: &HeaderMap, body: &Value) -> Response {
    let sid = canvas_sid(state, headers);
    let email =
        mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid).unwrap_or_default();
    let admin = admin_ok(state, &sid);
    let premium = !email.is_empty() && mitch_lib::auth::is_premium_email(&state.store, &email);

    let zone_v = body.get("zoneId").cloned().unwrap_or(Value::Null);
    let has_zone = jsval::truthy(&zone_v);
    let zone_key = jsval::string(&zone_v);
    if has_zone {
        if !state_core::check_zone_access(
            &state.store,
            state.data_dir(),
            &state.id_secret,
            std::env::var("NODE_ENV").unwrap_or_default() == "test",
            &zone_key,
            &email,
            &sid,
        ) {
            return json_response(
                403,
                json!({ "error": "forbidden", "reason": "No access to this zone" }),
            );
        }
    } else if !admin && !premium {
        return json_response(403, json!({ "error": "forbidden" }));
    }

    let points: Vec<Value> = body
        .get("pixels")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().take(2500).cloned().collect())
        .unwrap_or_default();
    if points.is_empty() {
        return json_response(400, json!({ "error": "pixels required" }));
    }

    let now = now_ms();
    let mut count: i64 = 0;
    // changedChunks (server.js:21817) — insertion-ordered for the broadcast.
    let mut changed_chunks: Vec<String> = Vec::new();
    for p in &points {
        let obj = p.as_object();
        let x = num_field(obj, "x");
        let y = num_field(obj, "y");
        let color = str_field(obj, "color");
        // String(p?.painter || email || 'admin').slice(0, 120).
        let painter_raw = obj
            .and_then(|o| o.get("painter"))
            .filter(|v| jsval::truthy(v));
        let painter = match painter_raw {
            Some(v) => jsval::string(v),
            None if !email.is_empty() => email.clone(),
            None => "admin".to_string(),
        };
        let painter = jsval::js_slice_utf16(&painter, 120);
        if !is_integer(x) || !is_integer(y) || !valid_color(&color) {
            continue;
        }
        if x.abs() > 500_000.0 || y.abs() > 500_000.0 {
            continue;
        }
        // Key order differs from pixel POST: admin BEFORE premium here.
        let mut pixel_data = Map::new();
        pixel_data.insert("color".into(), json!(color));
        pixel_data.insert("painter".into(), json!(painter));
        pixel_data.insert("ts".into(), json!(now));
        if admin {
            pixel_data.insert("admin".into(), json!(true));
        }
        if premium {
            pixel_data.insert("premium".into(), json!(true));
        }
        if !email.is_empty() {
            pixel_data.insert("email".into(), json!(email));
        }
        let zone_opt = if has_zone {
            Some(zone_key.as_str())
        } else {
            None
        };
        state.canvas.set_pixel(
            &state.store,
            state.data_dir(),
            x,
            y,
            Value::Object(pixel_data),
            zone_opt,
        );
        if !has_zone {
            state.canvas.heatmap_set(&state_core::pixel_key(x, y), now);
        }
        let ck = format!(
            "{},{}",
            (x / 64.0).floor() as i64,
            (y / 64.0).floor() as i64
        );
        if !changed_chunks.contains(&ck) {
            changed_chunks.push(ck);
        }
        count += 1;
    }

    if has_zone {
        state
            .canvas
            .save_zone_pixels(&state.store, state.data_dir(), &zone_key);
    } else if !email.is_empty() && count > 0 {
        mitch_lib::achievements::add_painting_coin(
            &state.store,
            state.data_dir(),
            &email,
            state.coin_multiplier(),
        );
    }
    // canvas_delta broadcast (server.js:21844-21846).
    if !changed_chunks.is_empty() {
        super::broadcast_canvas_delta(
            state,
            "invalidate",
            if has_zone { Some(&zone_key) } else { None },
            "chunks",
            json!(changed_chunks),
        );
    }
    json_response(200, json!({ "ok": true, "count": count }))
}

/// `/api/canvas/erase` POST (server.js:22006-22034).
pub fn erase_post(state: &Arc<AppState>, headers: &HeaderMap, body: &Value) -> Response {
    // JS `typeof x !== 'number'` — no string coercion here.
    let x = body.get("x").and_then(|v| v.as_f64());
    let y = body.get("y").and_then(|v| v.as_f64());
    let painter = body.get("painter").cloned().unwrap_or(Value::Null);
    let Some((x, y)) = x.zip(y) else {
        return json_response(400, json!({ "error": "missing fields" }));
    };
    let brush_sz = brush_size(body.get("brushSz"));

    let sid = canvas_sid(state, headers);
    let email =
        mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid).unwrap_or_default();
    let admin = admin_ok(state, &sid);
    let premium = !email.is_empty() && mitch_lib::auth::is_premium_email(&state.store, &email);

    let zone_v = body.get("zoneId").cloned().unwrap_or(Value::Null);
    let has_zone = jsval::truthy(&zone_v);
    let zone_key = jsval::string(&zone_v);
    if has_zone
        && !state_core::check_zone_access(
            &state.store,
            state.data_dir(),
            &state.id_secret,
            std::env::var("NODE_ENV").unwrap_or_default() == "test",
            &zone_key,
            &email,
            &sid,
        )
    {
        return json_response(
            403,
            json!({ "error": "forbidden", "reason": "No access to this zone" }),
        );
    }

    let max_allowed_brush = if admin {
        50.0
    } else if premium {
        16.0
    } else {
        8.0
    };
    if brush_sz > max_allowed_brush {
        return json_response(
            403,
            json!({ "error": "forbidden", "reason": "brush size too large" }),
        );
    }

    let half = (brush_sz / 2.0).floor();
    // Snapshot of the source pixels (JS reads the live map, but every brush
    // coordinate is distinct so the snapshot is equivalent).
    let source: Map<String, Value> = if has_zone {
        state
            .canvas
            .get_zone_pixels(&state.store, state.data_dir(), &zone_key)
            .unwrap_or_default()
    } else {
        state
            .canvas
            .pixels
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    };
    let zone_info = if has_zone {
        state
            .store
            .read_document(&state_core::zones_file(state.data_dir()), json!({}))
            .get(&zone_key)
            .cloned()
            .filter(jsval::truthy)
    } else {
        None
    };
    let is_owner = zone_info
        .map(|z| {
            normalize_email(&jsval::string(z.get("owner").unwrap_or(&Value::Null)))
                == normalize_email(&email)
        })
        .unwrap_or(false);

    let painter_str = jsval::string(&painter);
    let steps = brush_sz.ceil() as i64;
    // erasedPixels/erasedChunks (server.js:22010-22011).
    let mut erased_pixels: Vec<Value> = Vec::new();
    let mut erased_chunks: Vec<String> = Vec::new();
    for i in 0..steps {
        for j in 0..steps {
            let px = x - half + i as f64;
            let py = y - half + j as f64;
            let pkey = state_core::pixel_key(px, py);
            let Some(existing) = source.get(&pkey).filter(|v| jsval::truthy(v)) else {
                continue;
            };
            if !admin && !is_owner {
                let is_creator = existing.get("painter") == Some(&painter)
                    || (!email.is_empty() && existing.get("email") == Some(&json!(email)));
                if !is_creator {
                    continue;
                }
            }
            let zone_opt = if has_zone {
                Some(zone_key.as_str())
            } else {
                None
            };
            state
                .canvas
                .delete_pixel(state.data_dir(), px, py, &painter_str, &email, zone_opt);
            erased_pixels.push(json!({
                "x": jsval::num_value(px),
                "y": jsval::num_value(py),
            }));
            let ck = format!(
                "{},{}",
                (px / 64.0).floor() as i64,
                (py / 64.0).floor() as i64
            );
            if !erased_chunks.contains(&ck) {
                erased_chunks.push(ck);
            }
        }
    }
    if has_zone {
        state
            .canvas
            .save_zone_pixels(&state.store, state.data_dir(), &zone_key);
    }
    // canvas_delta broadcast (server.js:22025-22031).
    if !erased_pixels.is_empty() {
        if erased_pixels.len() > 300 {
            super::broadcast_canvas_delta(
                state,
                "invalidate",
                if has_zone { Some(&zone_key) } else { None },
                "chunks",
                json!(erased_chunks),
            );
        } else {
            super::broadcast_canvas_delta(
                state,
                "delete",
                if has_zone { Some(&zone_key) } else { None },
                "pixels",
                json!(erased_pixels),
            );
        }
    }
    json_response(200, json!({ "ok": true }))
}

/// The JS object literal `{ x, y }` with possibly-`undefined` members —
/// JSON.stringify drops undefined keys, so details are built key-by-key.
fn xy_details(x: Option<&Value>, y: Option<&Value>) -> Value {
    let mut m = Map::new();
    if let Some(x) = x {
        m.insert("x".into(), x.clone());
    }
    if let Some(y) = y {
        m.insert("y".into(), y.clone());
    }
    Value::Object(m)
}

/// `/api/canvas/admin-erase` POST (server.js:21752-21767).
pub fn admin_erase_post(state: &Arc<AppState>, headers: &HeaderMap, body: &Value) -> Response {
    let sid = canvas_sid(state, headers);
    if !admin_ok(state, &sid) {
        return json_response(403, json!({ "error": "forbidden" }));
    }
    let admin_email = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid)
        .unwrap_or_else(|| "admin".to_string());
    let x = body.get("x");
    let y = body.get("y");
    let key = format!("{},{}", js_template(x), js_template(y));
    let pixel_exists = {
        let pixels = state
            .canvas
            .pixels
            .read()
            .unwrap_or_else(|e| e.into_inner());
        pixels.get(&key).map(jsval::truthy).unwrap_or(false)
    };
    if !pixel_exists {
        return json_response(404, json!({ "error": "no pixel" }));
    }
    // Chunk key from Number(x)/Number(y); pixel key from the raw template.
    let ck = state_core::chunk_key(
        x.and_then(jsval::number).unwrap_or(f64::NAN),
        y.and_then(jsval::number).unwrap_or(f64::NAN),
    );
    state.canvas.delete_pixel_raw(
        state.data_dir(),
        state_core::RawPixelDelete {
            key: &key,
            ck: &ck,
            x: x.cloned(),
            y: y.cloned(),
            painter: "admin",
            email: &admin_email,
        },
        None,
    );
    mitch_lib::admin::log_admin_action(
        &state.store,
        state.data_dir(),
        &admin_email,
        "canvas_erase",
        xy_details(x, y),
    );
    // canvas_delta broadcast (server.js:21760).
    super::broadcast_canvas_delta(
        state,
        "delete",
        None,
        "pixels",
        json!([{ "x": jsval::num_value(x.and_then(jsval::number).unwrap_or(f64::NAN)), "y": jsval::num_value(y.and_then(jsval::number).unwrap_or(f64::NAN)) }]),
    );
    json_response(200, json!({ "ok": true }))
}

/// `/api/canvas/admin-ban` POST (server.js:21769-21787).
pub fn admin_ban_post(state: &Arc<AppState>, headers: &HeaderMap, body: &Value) -> Response {
    let sid = canvas_sid(state, headers);
    if !admin_ok(state, &sid) {
        return json_response(403, json!({ "error": "forbidden" }));
    }
    let painter = body.get("painter").cloned().unwrap_or(Value::Null);
    let reason = body.get("reason").cloned().unwrap_or(Value::Null);
    if !jsval::truthy(&painter) {
        return json_response(400, json!({ "error": "painter required" }));
    }
    // The painter's email, looked up from the live pixels.
    let p_email = {
        let pixels = state
            .canvas
            .pixels
            .read()
            .unwrap_or_else(|e| e.into_inner());
        pixels
            .values()
            .filter_map(|p| p.as_object())
            .find(|p| p.get("painter") == Some(&painter))
            .and_then(|p| p.get("email"))
            .filter(|v| jsval::truthy(v))
            .cloned()
    };
    if let Some(email_v) = &p_email {
        if normalize_email(&jsval::string(email_v)) == normalize_email("admin@mitch.pro") {
            return json_response(403, json!({ "error": "cannot ban admin" }));
        }
    }
    let mut banned: Map<String, Value> = state_core::load_bans(&state.store, state.data_dir())
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut ban_entry = Map::new();
    ban_entry.insert(
        "reason".into(),
        if jsval::truthy(&reason) {
            reason.clone()
        } else {
            json!("banned by admin")
        },
    );
    ban_entry.insert("ts".into(), json!(now_ms()));
    ban_entry.insert("byAdmin".into(), json!(true));
    banned.insert(jsval::string(&painter), Value::Object(ban_entry));
    state_core::save_bans(&state.store, state.data_dir(), &state.canvas, banned);
    let painter_str = jsval::string(&painter);
    let log_reason = if jsval::truthy(&reason) {
        jsval::string(&reason)
    } else {
        "no reason".to_string()
    };
    mitch_lib::admin::log_admin_action(
        &state.store,
        state.data_dir(),
        &mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid)
            .unwrap_or_else(|| "admin".to_string()),
        "canvas_ban",
        json!({
            "painter": jsval::js_slice_utf16(&painter_str, 12),
            "reason": log_reason,
        }),
    );
    json_response(200, json!({ "ok": true }))
}

/// `/api/canvas/admin-unban` POST (server.js:21789-21802).
pub fn admin_unban_post(state: &Arc<AppState>, headers: &HeaderMap, body: &Value) -> Response {
    let sid = canvas_sid(state, headers);
    if !admin_ok(state, &sid) {
        return json_response(403, json!({ "error": "forbidden" }));
    }
    let painter = body.get("painter").cloned().unwrap_or(Value::Null);
    if !jsval::truthy(&painter) {
        return json_response(400, json!({ "error": "painter required" }));
    }
    let mut banned: Map<String, Value> = state_core::load_bans(&state.store, state.data_dir())
        .as_object()
        .cloned()
        .unwrap_or_default();
    let key = jsval::string(&painter);
    if !banned.get(&key).map(jsval::truthy).unwrap_or(false) {
        return json_response(404, json!({ "error": "not banned" }));
    }
    banned.remove(&key);
    state_core::save_bans(&state.store, state.data_dir(), &state.canvas, banned);
    mitch_lib::admin::log_admin_action(
        &state.store,
        state.data_dir(),
        &mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid)
            .unwrap_or_else(|| "admin".to_string()),
        "canvas_unban",
        json!({ "painter": jsval::js_slice_utf16(&key, 12) }),
    );
    json_response(200, json!({ "ok": true }))
}

/// Shared dispatch for the batch-2 POST endpoints — called from mod.rs after
/// the strict body parse (a bad body is 400 `bad json` first, like JS's
/// `tryParseJson`).
pub fn dispatch(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Option<Response> {
    if *method != Method::POST {
        return None;
    }
    let body = match parse_body_strict(body_bytes) {
        Some(b) => b,
        None => return Some(json_response(400, json!({ "error": "bad json" }))),
    };
    let resp = match path {
        "/api/canvas/pixel" => pixel_post(state, headers, &body),
        "/api/canvas/pixels/bulk" => bulk_post(state, headers, &body),
        "/api/canvas/erase" => erase_post(state, headers, &body),
        "/api/canvas/admin-erase" => admin_erase_post(state, headers, &body),
        "/api/canvas/admin-ban" => admin_ban_post(state, headers, &body),
        "/api/canvas/admin-unban" => admin_unban_post(state, headers, &body),
        _ => return None,
    };
    Some(resp)
}
