//! Admin side of the homepage update log (`/api/admin/updates`). The public
//! read side is `GET /api/updates` in `routes/misc.rs`; the rules and storage
//! are in `mitch_lib::site_updates` and `docs/site-updates.md`.
//!
//! Full admins only (not moderators), behind the usual passphrase gate.

use std::sync::Arc;

use axum::http::Method;
use serde_json::{json, Value};

use super::{forbidden, json_response, AdminCtx, Resp};
use crate::state::AppState;
use mitch_lib::site_updates as db;

const BASE: &str = "/api/admin/updates";

/// `YYYY-MM-DD` as noon UTC, so the day reads the same in every time zone.
fn parse_date(value: &str) -> Option<i64> {
    let date = chrono::NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").ok()?;
    Some(date.and_hms_opt(12, 0, 0)?.and_utc().timestamp_millis())
}

fn text(body: &Value, key: &str) -> String {
    body.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn error_message(err: &db::SaveError) -> (u16, &'static str) {
    match err {
        db::SaveError::EmptyTitle => (400, "Add a title."),
        db::SaveError::TitleTooLong => (400, "That title is too long."),
        db::SaveError::BodyTooLong => (400, "That text is too long."),
        db::SaveError::NotFound => (404, "That entry no longer exists."),
        db::SaveError::Database => (500, "Could not save that. Try again."),
    }
}

fn season_label(season: &str) -> &'static str {
    match season {
        "halloween" => "Halloween (Oct 15)",
        "thanksgiving" => "Thanksgiving (Nov 1)",
        "christmas" => "Christmas (day after Thanksgiving)",
        "newyear" => "New Year's (Dec 27)",
        _ => "Valentine's (Feb 7)",
    }
}

pub fn public_json(update: &db::Update) -> Value {
    json!({
        "id": update.id,
        "title": update.title,
        "body": update.body,
        "postedAt": update.posted_at,
    })
}

pub fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    body: &Value,
    ctx: &AdminCtx,
) -> Resp {
    let rest = path.strip_prefix(BASE)?;
    if !(rest.is_empty() || rest == "/delete" || rest == "/auto") {
        return None;
    }
    if !ctx.is_admin(state) {
        return Some(forbidden());
    }
    let email = ctx.email(state);
    let log = |action: &str, id: i64| {
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &email,
            action,
            json!({ "id": id }),
        );
    };
    Some(match (method.as_str(), rest) {
        ("GET", "") => {
            let entries: Vec<Value> = db::list(&state.store, db::ADMIN_LIST_LIMIT)
                .iter()
                .map(|u| {
                    let mut value = public_json(u);
                    value["author"] = json!(u.author);
                    value
                })
                .collect();
            json_response(200, json!({ "updates": entries }))
        }
        ("POST", "") => {
            let title = text(body, "title");
            let text_body = text(body, "body");
            let date = text(body, "date");
            let posted_at = if date.trim().is_empty() {
                None
            } else {
                match parse_date(&date) {
                    Some(ms) => Some(ms),
                    None => {
                        return Some(json_response(
                            400,
                            json!({ "error": "Use a date like 2026-11-04." }),
                        ))
                    }
                }
            };
            let id = body.get("id").and_then(Value::as_i64);
            let result = match id {
                Some(id) => {
                    db::update(&state.store, id, &title, &text_body, posted_at).map(|()| id)
                }
                None => db::create(&state.store, &title, &text_body, posted_at, &email),
            };
            match result {
                Ok(id) => {
                    log(
                        if body.get("id").is_some() {
                            "edit_site_update"
                        } else {
                            "post_site_update"
                        },
                        id,
                    );
                    json_response(200, json!({ "ok": true, "id": id }))
                }
                Err(err) => {
                    let (code, message) = error_message(&err);
                    json_response(code, json!({ "error": message }))
                }
            }
        }
        ("GET", "/auto") => {
            let templates: Vec<Value> = db::templates(&state.store)
                .iter()
                .map(|t| {
                    json!({
                        "season": t.season,
                        "label": season_label(&t.season),
                        "title": t.title,
                        "body": t.body,
                        "enabled": t.enabled,
                    })
                })
                .collect();
            json_response(200, json!({ "templates": templates }))
        }
        ("POST", "/auto") => {
            let season = text(body, "season");
            let enabled = body.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            match db::save_template(
                &state.store,
                &season,
                &text(body, "title"),
                &text(body, "body"),
                enabled,
            ) {
                Ok(()) => {
                    log("edit_auto_update", 0);
                    json_response(200, json!({ "ok": true }))
                }
                Err(err) => {
                    let (code, message) = error_message(&err);
                    json_response(code, json!({ "error": message }))
                }
            }
        }
        ("POST", "/delete") => {
            let id = body.get("id").and_then(Value::as_i64).unwrap_or(-1);
            if db::delete(&state.store, id) {
                log("delete_site_update", id);
                json_response(200, json!({ "ok": true }))
            } else {
                json_response(404, json!({ "error": "That entry no longer exists." }))
            }
        }
        _ => json_response(405, json!({ "error": "method not allowed" })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_read_as_noon_utc() {
        assert_eq!(parse_date("2026-11-04"), Some(1_793_793_600_000));
        assert_eq!(parse_date(" 2026-11-04 "), Some(1_793_793_600_000));
        assert_eq!(parse_date("11/04/2026"), None);
        assert_eq!(parse_date("2026-13-40"), None);
        assert_eq!(parse_date(""), None);
    }
}
