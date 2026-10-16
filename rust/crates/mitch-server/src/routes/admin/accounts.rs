use super::{forbidden, json_response, AdminCtx, Resp};
use crate::state::AppState;
use axum::http::Method;
use mitch_lib::{auth, profile, profiles};
use serde_json::{json, Value};
use std::{collections::BTreeSet, sync::Arc};

pub fn directory(state: &AppState) -> Vec<Value> {
    let profiles = profiles::profile_all(&state.store);
    let mut emails: BTreeSet<String> = auth::names_all_emails(&state.store)
        .into_iter()
        .map(|email| auth::normalize_email(&email))
        .collect();
    emails.extend(mitch_lib::passwords::password_all_emails(&state.store));
    if let Some(map) = profiles.as_object() {
        emails.extend(map.keys().cloned());
    }
    let matrix_users = state
        .store
        .read_document(&state.data_dir().join("matrix_users.json"), json!({}));
    let mut matrix_ids = std::collections::HashMap::new();
    if let Some(map) = matrix_users.as_object() {
        for (key, value) in map {
            let email = if key.contains('@') {
                Some(auth::normalize_email(key))
            } else {
                auth::email_from_sid(&state.store, &state.id_secret, key)
            };
            if let (Some(email), Some(username)) = (email, value.as_str()) {
                matrix_ids.insert(email, format!("@{username}:mitch.pro"));
            }
        }
    }
    emails
        .into_iter()
        .filter(|email| email.contains('@') && !email.starts_with('@'))
        .map(|email| {
            let p = &profiles[&email];
            let username = p["username"]
                .as_str()
                .filter(|name| !name.is_empty())
                .map(String::from)
                .unwrap_or_else(|| profile::default_username_for_email(&email));
            let display = p["displayName"]
                .as_str()
                .or_else(|| p["nickname"].as_str())
                .filter(|name| !name.is_empty())
                .unwrap_or(&username);
            json!({
                "email":email,
                "username":username,
                "displayName":display,
                "protected":auth::is_owner_email(&state.store,&email),
                "matrixUserId":matrix_ids.get(&email).cloned().unwrap_or_default(),
            })
        })
        .collect()
}

pub fn resolve(state: &AppState, raw: &str) -> Option<String> {
    let target = raw.trim().to_lowercase();
    if target.is_empty() {
        return None;
    }
    if let Some(email) = auth::names_get_email(&state.store, raw.trim()) {
        return Some(email);
    }
    if target.contains('@') && !target.starts_with('@') {
        return Some(auth::normalize_email(&target));
    }
    if target.starts_with('@') && target.contains(':') {
        return directory(state)
            .iter()
            .find(|account| {
                account["matrixUserId"]
                    .as_str()
                    .is_some_and(|id| id.eq_ignore_ascii_case(&target))
            })
            .and_then(|account| account["email"].as_str().map(String::from));
    }
    let query = target
        .trim_start_matches('@')
        .split(':')
        .next()
        .unwrap_or("");
    if query.is_empty() {
        return None;
    }
    let all = directory(state);
    let exact: Vec<_> = all
        .iter()
        .filter(|account| {
            ["username", "displayName"].iter().any(|key| {
                account[key]
                    .as_str()
                    .unwrap_or("")
                    .trim()
                    .eq_ignore_ascii_case(query)
            })
        })
        .collect();
    if exact.len() == 1 {
        return exact[0]["email"].as_str().map(String::from);
    }
    if !exact.is_empty() {
        return None;
    }
    let partial: Vec<_> = all
        .iter()
        .filter(|account| {
            ["email", "username", "displayName"].iter().any(|key| {
                account[key]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(query)
            })
        })
        .collect();
    (partial.len() == 1).then(|| partial[0]["email"].as_str().unwrap_or("").to_string())
}

pub fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    search: &str,
    ctx: &AdminCtx,
) -> Resp {
    if path != "/api/admin/account-search" && path != "/api/admin/accounts" {
        return None;
    }
    if !ctx.is_any_admin(state) || (path == "/api/admin/accounts" && !ctx.is_owner(state)) {
        return Some(forbidden());
    }
    if *method != Method::GET {
        return Some(json_response(405, json!({"error":"Method not allowed."})));
    }
    let query = crate::handler::query(search)
        .get("q")
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    if query.len() < 2 && path != "/api/admin/accounts" {
        return Some(json_response(200, json!({"accounts":[]})));
    }
    let mut accounts = directory(state);
    accounts.retain(|account| {
        query.is_empty()
            || ["email", "username", "displayName"].iter().any(|key| {
                account[key]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&query)
            })
    });
    accounts.sort_by_key(|account| {
        let username = account["username"].as_str().unwrap_or("").to_lowercase();
        let display = account["displayName"].as_str().unwrap_or("").to_lowercase();
        (
            if username == query || display == query {
                0
            } else if username.starts_with(&query) || display.starts_with(&query) {
                1
            } else {
                2
            },
            display,
        )
    });
    if path != "/api/admin/accounts" {
        accounts.truncate(12);
    }
    Some(json_response(200, json!({"accounts":accounts})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_resolve_uniquely_and_matrix_ids_keep_their_server() {
        let base = std::env::temp_dir().join(format!(
            "mitch-account-search-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        let store = Arc::new(mitch_lib::data::DataStore::open(&base, &base.join("data")).unwrap());
        let config = crate::hosts::SiteConfig {
            data_dir: base.join("data"),
            ..crate::hosts::SiteConfig::load()
        };
        let state = AppState::new(config, store);
        for (email, username) in [
            ("one@example.com", "alex_one"),
            ("two@example.com", "alex_two"),
        ] {
            profiles::profile_set(
                &state.store,
                email,
                &json!({"username":username,"displayName":"Alex"}),
            );
        }
        state
            .store
            .write_document(
                &state.data_dir().join("matrix_users.json"),
                &json!({"one@example.com":"stable_alex"}),
            )
            .unwrap();
        assert_eq!(resolve(&state, "Alex"), None);
        assert_eq!(resolve(&state, "alex_"), None);
        assert_eq!(resolve(&state, "@"), None);
        assert_eq!(
            resolve(&state, " ALEX_ONE "),
            Some("one@example.com".into())
        );
        assert_eq!(
            resolve(&state, "@stable_alex:mitch.pro"),
            Some("one@example.com".into())
        );
        assert_eq!(resolve(&state, "@stable_alex:unrelated.test"), None);
        assert_eq!(
            resolve(&state, "new@example.com"),
            Some("new@example.com".into())
        );
        drop(state);
        let _ = std::fs::remove_dir_all(base);
    }
}
