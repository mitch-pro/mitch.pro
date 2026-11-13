//! Staff side of the special bell schedule (`/api/admin/special-schedule`),
//! used by `/admin/special-schedule/`. The public read side is
//! `GET /api/bell/override` in `routes/misc.rs`; the rules and storage are in
//! `mitch_lib::bell_overrides` and `docs/bell-overrides.md`.
//!
//! Admins and moderators. Admins also pass the passphrase gate in
//! `admin_gate`; moderators are exempt from it, as everywhere else.
//!
//! - `GET  /api/admin/special-schedule`: schools, preset schedules, current
//!   overrides and today's date in Los Angeles.
//! - `POST /api/admin/special-schedule`: set one school's schedule.
//! - `POST /api/admin/special-schedule/clear`: `{school}` or `{school: "all"}`.

use std::sync::Arc;

use axum::http::Method;
use serde_json::{json, Value};

use super::{forbidden, json_response, AdminCtx, Resp};
use crate::state::AppState;
use mitch_lib::bell_overrides as db;

const BASE: &str = "/api/admin/special-schedule";

fn text(body: &Value, key: &str) -> String {
    body.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn periods(body: &Value, key: &str) -> Vec<db::PeriodInput> {
    body.get(key)
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|r| db::PeriodInput {
                    name: text(r, "name"),
                    start: text(r, "start"),
                    end: text(r, "end"),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn error_message(err: db::SaveError) -> (u16, &'static str) {
    use db::SaveError::*;
    match err {
        UnknownSchool => (400, "Choose a school."),
        BadDate => (400, "Choose a valid date."),
        PastDate => (400, "That day has already passed."),
        TooFarAhead => (400, "Pick a day within the next 90 days."),
        BadName => (400, "Give the schedule a name (80 characters at most)."),
        BadEvent => (
            400,
            "The event notice is too long (200 characters at most).",
        ),
        NoPeriods => (400, "Add at least one period to each lunch."),
        TooManyPeriods => (400, "A lunch can have at most 20 periods."),
        BadPeriodName => (400, "Every period needs a name (60 characters at most)."),
        BadTime => (400, "Use times like 08:30."),
        EndBeforeStart => (400, "Each period must end after it starts."),
        Database => (500, "Could not save that. Try again."),
    }
}

pub fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    body: &Value,
    ctx: &AdminCtx,
) -> Resp {
    let rest = path.strip_prefix(BASE)?;
    if !(rest.is_empty() || rest == "/clear") {
        return None;
    }
    if !ctx.is_any_admin(state) {
        return Some(forbidden());
    }
    let email = ctx.email(state);
    let log = |action: &str, detail: Value| {
        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &email,
            action,
            detail,
        );
    };
    Some(match (method.as_str(), rest) {
        ("GET", "") => json_response(
            200,
            json!({
                "today": db::today_la(),
                "schools": db::SCHOOLS.iter().map(|(k, l)| json!({ "key": k, "label": l })).collect::<Vec<_>>(),
                "presets": db::presets_json(),
                "overrides": db::list(&state.store).iter().map(db::to_json).collect::<Vec<_>>(),
            }),
        ),
        ("POST", "") => {
            let today = db::today_la();
            let school = text(body, "school");
            let date = text(body, "date");
            let result = db::validate(
                &school,
                &date,
                &text(body, "name"),
                &text(body, "specialEvent"),
                &periods(body, "lunch1"),
                &periods(body, "lunch2"),
                &today,
            )
            .and_then(|o| db::set(&state.store, &o, &email, &today).map(|()| o));
            match result {
                Ok(o) => {
                    log(
                        "set_special_schedule",
                        json!({ "school": o.school, "date": o.date, "name": o.name }),
                    );
                    json_response(200, json!({ "ok": true, "override": db::to_json(&o) }))
                }
                Err(err) => {
                    let (code, message) = error_message(err);
                    json_response(code, json!({ "error": message }))
                }
            }
        }
        ("POST", "/clear") => {
            let school = text(body, "school");
            let target = match school.as_str() {
                "all" => None,
                key if db::school_label(key).is_some() => Some(key),
                _ => return Some(json_response(400, json!({ "error": "Choose a school." }))),
            };
            let removed = db::clear(&state.store, target);
            log(
                "clear_special_schedule",
                json!({ "school": school, "removed": removed }),
            );
            json_response(200, json!({ "ok": true, "removed": removed }))
        }
        _ => json_response(405, json!({ "error": "method not allowed" })),
    })
}
