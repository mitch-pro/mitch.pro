//! Staff side of Rate My Teacher (`/api/admin/teachers`). Student routes are in
//! `routes/teachers.rs`, storage and rules in `mitch_lib::teachers`, the page is
//! `/moderator/teachers/`, and the overview is `docs/rate-my-teacher.md`.
//!
//! Moderators and admins review notes, add teachers, paste lists and run the
//! directory sync. Only full admins can delete a note or remove a teacher
//! (that also removes the teacher's ratings). Nothing here can see who rated
//! or wrote anything: that is not stored.

use std::sync::Arc;

use axum::http::Method;
use serde_json::{json, Value};

use super::{forbidden, json_response, AdminCtx, Resp};
use crate::routes::dayboard::RJUHSD_SCHOOLS;
use crate::routes::teachers::{query_param, school_name};
use crate::state::AppState;
use mitch_lib::teachers as db;

const BASE: &str = "/api/admin/teachers";

/// Where each school's staff directory may live on its Finalsite site, tried
/// in order. The sync only ever requests these paths on the schools' own
/// domains from `RJUHSD_SCHOOLS`.
const DIRECTORY_PATHS: &[&str] = &[
    "/about-us/staff-directory",
    "/staff-directory",
    "/about/staff-directory",
    "/our-staff",
];
/// Finalsite directories are paged with `const_page`; stop after this many.
const MAX_PAGES: usize = 30;
const MAX_PAGE_BYTES: usize = 3_000_000;

fn note_json(n: &db::Note) -> Value {
    json!({
        "id": n.id,
        "teacher": n.teacher_name,
        "school": school_name(&n.school).unwrap_or(&n.school),
        "text": n.text,
        "day": n.day,
        "status": n.status,
        "reviewedBy": n.reviewed_by,
    })
}

/// Fetches one school's directory, adds anyone new and records when it was
/// read. Returns what happened for the staff page. Teacher names come only from
/// the school's own site at run time; none are kept in the repository.
async fn sync_school(
    state: &Arc<AppState>,
    client: &reqwest::Client,
    key: &str,
    base: &str,
) -> Value {
    let mut last_error = String::from("no staff directory found at the usual addresses");
    for path in DIRECTORY_PATHS {
        let mut found: Vec<(String, String)> = Vec::new();
        for page in 1..=MAX_PAGES {
            let url = if page == 1 {
                format!("{base}{path}")
            } else {
                format!("{base}{path}?const_page={page}")
            };
            let res = match client
                .get(&url)
                .header(reqwest::header::USER_AGENT, "mitch.pro teacher list sync")
                .send()
                .await
            {
                Ok(r) if r.status().is_success() => r,
                Ok(r) => {
                    last_error = format!("{path} answered {}", r.status().as_u16());
                    break;
                }
                Err(e) => {
                    last_error = format!("{path}: {}", e.without_url());
                    break;
                }
            };
            let Ok(bytes) = res.bytes().await else { break };
            if bytes.len() > MAX_PAGE_BYTES {
                last_error = format!("{path} was too large");
                break;
            }
            let people = db::parse_finalsite_directory(&String::from_utf8_lossy(&bytes));
            let before = found.len();
            for p in people {
                if !found.iter().any(|(n, _)| *n == p.0) {
                    found.push(p);
                }
            }
            if found.len() == before {
                break;
            }
        }
        if !found.is_empty() {
            let added = db::add_many(&state.store, key, &found, "sync");
            return json!({ "school": key, "found": found.len(), "added": added });
        }
    }
    json!({ "school": key, "found": 0, "added": 0, "error": last_error })
}

/// Reads every school whose list is due: never read, read over a day ago, or a
/// failed read over six hours ago. Called by the site worker.
pub async fn sync_due_schools(state: &Arc<AppState>) {
    let now = mitch_lib::school::now_millis();
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .build()
    else {
        return;
    };
    for (key, entry) in RJUHSD_SCHOOLS {
        if db::sync_due(&state.store, key, now, 86_400_000, 6 * 3_600_000) {
            sync_school(state, &client, key, entry.url).await;
        }
    }
}

pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    search: &str,
    body: &Value,
    ctx: &AdminCtx,
) -> Resp {
    let rest = path.strip_prefix(BASE)?;
    if !matches!(
        rest,
        "" | "/list" | "/note-status" | "/note-delete" | "/add" | "/import" | "/remove" | "/sync"
    ) {
        return None;
    }
    if !ctx.is_any_admin(state) {
        return Some(forbidden());
    }
    let id = body.get("id").and_then(Value::as_i64).unwrap_or(-1);
    let school_of = |v: &str| school_name(v).map(|_| v.to_string());
    Some(match (method.as_str(), rest) {
        ("GET", "") => {
            let (pending, approved, rejected) = db::note_counts(&state.store);
            let notes: Vec<Value> = db::list_notes(&state.store, &query_param(search, "status"))
                .iter()
                .map(note_json)
                .collect();
            json_response(
                200,
                json!({ "notes": notes, "counts": { "pending": pending, "approved": approved, "rejected": rejected } }),
            )
        }
        ("GET", "/list") => {
            let school = school_of(&query_param(search, "school"))
                .unwrap_or_else(|| RJUHSD_SCHOOLS[0].0.to_string());
            let summaries = db::summaries_for_school(&state.store, &school);
            let teachers: Vec<Value> = db::list_teachers(&state.store, &school, "")
                .iter()
                .map(|t| {
                    json!({
                        "id": t.id,
                        "name": t.name,
                        "department": t.department,
                        "ratings": summaries.get(&t.id).map(|s| s.count).unwrap_or(0),
                    })
                })
                .collect();
            let schools: Vec<Value> = RJUHSD_SCHOOLS
                .iter()
                .map(|(k, e)| {
                    let sync = db::sync_status(&state.store, k)
                        .map(|(at, found, error)| json!({ "at": at, "found": found, "error": error }));
                    json!({ "key": k, "name": e.name, "teachers": db::count_teachers(&state.store, k), "sync": sync })
                })
                .collect();
            json_response(
                200,
                json!({ "school": school, "schools": schools, "teachers": teachers }),
            )
        }
        ("POST", "/note-status") => {
            let status = body.get("status").and_then(Value::as_str).unwrap_or("");
            if db::set_note_status(
                &state.store,
                id,
                status,
                &ctx.email(state),
                mitch_lib::school::now_millis(),
            ) {
                json_response(200, json!({ "ok": true }))
            } else {
                json_response(404, json!({ "error": "That note no longer exists." }))
            }
        }
        ("POST", "/note-delete") => {
            if !ctx.is_admin(state) {
                return Some(forbidden());
            }
            if db::delete_note(&state.store, id) {
                mitch_lib::admin::log_admin_action(
                    &state.store,
                    &state.cfg.data_dir,
                    &ctx.email(state),
                    "delete_teacher_note",
                    json!({ "id": id }),
                );
                json_response(200, json!({ "ok": true }))
            } else {
                json_response(404, json!({ "error": "That note no longer exists." }))
            }
        }
        ("POST", "/add") => {
            let Some(school) = school_of(body.get("school").and_then(Value::as_str).unwrap_or(""))
            else {
                return Some(json_response(400, json!({ "error": "Pick a school." })));
            };
            let name = body.get("name").and_then(Value::as_str).unwrap_or("");
            let dept = body.get("department").and_then(Value::as_str).unwrap_or("");
            if db::add_teacher(&state.store, &school, name, dept, "manual") {
                json_response(200, json!({ "ok": true }))
            } else {
                json_response(
                    400,
                    json!({ "error": "Enter a name that isn't already listed." }),
                )
            }
        }
        ("POST", "/import") => {
            let Some(school) = school_of(body.get("school").and_then(Value::as_str).unwrap_or(""))
            else {
                return Some(json_response(400, json!({ "error": "Pick a school." })));
            };
            let list =
                db::parse_pasted_list(body.get("text").and_then(Value::as_str).unwrap_or(""));
            let added = db::add_many(&state.store, &school, &list, "manual");
            json_response(
                200,
                json!({ "ok": true, "found": list.len(), "added": added }),
            )
        }
        ("POST", "/remove") => {
            if !ctx.is_admin(state) {
                return Some(forbidden());
            }
            if db::remove_teacher(&state.store, id) {
                mitch_lib::admin::log_admin_action(
                    &state.store,
                    &state.cfg.data_dir,
                    &ctx.email(state),
                    "remove_teacher",
                    json!({ "id": id }),
                );
                json_response(200, json!({ "ok": true }))
            } else {
                json_response(404, json!({ "error": "That teacher no longer exists." }))
            }
        }
        ("POST", "/sync") => {
            let only = body.get("school").and_then(Value::as_str).unwrap_or("");
            let Ok(client) = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(12))
                .build()
            else {
                return Some(json_response(
                    500,
                    json!({ "error": "Could not start the sync." }),
                ));
            };
            let mut results = Vec::new();
            for (key, entry) in RJUHSD_SCHOOLS {
                if only.is_empty() || only == *key {
                    results.push(sync_school(state, &client, key, entry.url).await);
                }
            }
            json_response(200, json!({ "ok": true, "results": results }))
        }
        _ => json_response(405, json!({ "error": "method not allowed" })),
    })
}
