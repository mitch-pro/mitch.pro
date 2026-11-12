//! Rate My Teacher, student side (`/api/teachers`). Storage, the anonymity
//! design and the rules live in `mitch_lib::teachers`; the staff side is
//! `routes/admin/teachers.rs`; the overview is `docs/rate-my-teacher.md`.
//!
//! - `GET  /api/teachers?school=KEY&q=TEXT`: the teacher list for a school with
//!   each teacher's star averages, and whether this account already rated them.
//! - `GET  /api/teachers/one?id=N`: one teacher with approved notes.
//! - `POST /api/teachers/rate`: `{teacherId, overall, homework?, teaching?,
//!   chill?, note?}`.
//!
//! Only student accounts (`@student.rjuhsd.us`) can rate. Staff can read the
//! pages but not rate. Under `GOD_MODE` (local development only) the dev admin
//! may rate too so the flow can be tried without a student login.

use crate::routes::dayboard::RJUHSD_SCHOOLS;
use crate::routes::me::{cookies_of, json_response, me_uid, parse_body_strict};
use crate::state::AppState;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use mitch_lib::teachers as db;
use serde_json::{json, Value};
use std::sync::Arc;

pub(crate) fn query_param(search: &str, key: &str) -> String {
    url::form_urlencoded::parse(search.trim_start_matches('?').as_bytes())
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

pub(crate) fn school_name(key: &str) -> Option<&'static str> {
    RJUHSD_SCHOOLS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, e)| e.name)
}

fn schools_json() -> Value {
    Value::Array(
        RJUHSD_SCHOOLS
            .iter()
            .map(|(k, e)| json!({ "key": k, "name": e.name }))
            .collect(),
    )
}

fn stars_json(s: &db::Summary) -> Value {
    let round = |v: Option<f64>| v.map(|x| (x * 10.0).round() / 10.0);
    json!({
        "count": s.count,
        "overall": round(s.overall),
        "homework": round(s.homework),
        "teaching": round(s.teaching),
        "chill": round(s.chill),
    })
}

fn star_value(body: &Value, key: &str) -> Option<i64> {
    body.get(key).and_then(Value::as_i64)
}

pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
    search: &str,
) -> Option<Response> {
    if !matches!(
        path,
        "/api/teachers" | "/api/teachers/one" | "/api/teachers/rate"
    ) {
        return None;
    }
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    let Some(email) = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid) else {
        return Some(json_response(
            401,
            json!({ "error": "Sign in to see teacher ratings." }),
        ));
    };
    let norm = mitch_lib::auth::normalize_email(&email);
    let is_student = mitch_lib::matrix::is_student_email(&norm);
    let is_staff = mitch_lib::auth::is_any_admin_id(&state.store, &state.id_secret, &sid, false);
    let can_rate = is_student || mitch_lib::auth::god_mode_enabled();
    if !(can_rate || is_staff) {
        return Some(json_response(
            403,
            json!({ "error": "Teacher ratings are for student accounts.", "code": "students_only" }),
        ));
    }
    let key = db::mark_key(&state.id_secret);

    Some(match (method.as_str(), path) {
        ("GET", "/api/teachers") => {
            let requested = query_param(search, "school");
            let school = if school_name(&requested).is_some() {
                requested
            } else {
                RJUHSD_SCHOOLS[0].0.to_string()
            };
            let summaries = db::summaries_for_school(&state.store, &school);
            let teachers: Vec<Value> =
                db::list_teachers(&state.store, &school, &query_param(search, "q"))
                    .iter()
                    .map(|t| {
                        let s = summaries.get(&t.id).cloned().unwrap_or_default();
                        json!({
                            "id": t.id,
                            "name": t.name,
                            "department": t.department,
                            "ratings": stars_json(&s),
                            "rated": db::has_rated(&state.store, &key, &norm, t.id),
                        })
                    })
                    .collect();
            json_response(
                200,
                json!({
                    "schools": schools_json(),
                    "school": school,
                    "canRate": can_rate,
                    "total": db::count_teachers(&state.store, &school),
                    "teachers": teachers,
                }),
            )
        }
        ("GET", "/api/teachers/one") => {
            let id = query_param(search, "id").parse::<i64>().unwrap_or(-1);
            let Some(t) = db::get_teacher(&state.store, id) else {
                return Some(json_response(
                    404,
                    json!({ "error": "That teacher isn't listed." }),
                ));
            };
            let notes: Vec<Value> = db::approved_notes(&state.store, id)
                .iter()
                .map(|n| json!({ "text": n.text, "day": n.day }))
                .collect();
            json_response(
                200,
                json!({
                    "teacher": {
                        "id": t.id,
                        "name": t.name,
                        "department": t.department,
                        "school": t.school,
                        "schoolName": school_name(&t.school).unwrap_or(""),
                    },
                    "ratings": stars_json(&db::summary(&state.store, id)),
                    "notes": notes,
                    "canRate": can_rate,
                    "rated": db::has_rated(&state.store, &key, &norm, id),
                }),
            )
        }
        ("POST", "/api/teachers/rate") => {
            if !can_rate {
                return Some(json_response(
                    403,
                    json!({ "error": "Only student accounts can rate teachers.", "code": "students_only" }),
                ));
            }
            let Some(body) = parse_body_strict(body_bytes) else {
                return Some(json_response(400, json!({ "error": "bad json" })));
            };
            let stars = db::Stars {
                overall: star_value(&body, "overall").unwrap_or(0),
                homework: star_value(&body, "homework"),
                teaching: star_value(&body, "teaching"),
                chill: star_value(&body, "chill"),
            };
            let id = star_value(&body, "teacherId").unwrap_or(-1);
            let note = body.get("note").and_then(Value::as_str).unwrap_or("");
            match db::rate(
                &state.store,
                &key,
                &norm,
                id,
                stars,
                note,
                mitch_lib::school::now_millis(),
            ) {
                Ok(()) => json_response(
                    200,
                    json!({
                        "ok": true,
                        "noteQueued": !note.trim().is_empty(),
                        "ratings": stars_json(&db::summary(&state.store, id)),
                    }),
                ),
                Err(db::RateError::AlreadyRated) => json_response(
                    409,
                    json!({ "error": "You've already rated this teacher." }),
                ),
                Err(db::RateError::InvalidStars) => {
                    json_response(400, json!({ "error": "Pick 1 to 5 stars." }))
                }
                Err(db::RateError::NoSuchTeacher) => {
                    json_response(404, json!({ "error": "That teacher isn't listed." }))
                }
                Err(db::RateError::Storage) => {
                    json_response(500, json!({ "error": "Could not save that. Try again." }))
                }
            }
        }
        _ => json_response(405, json!({ "error": "method not allowed" })),
    })
}
