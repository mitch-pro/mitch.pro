//! Per-document presence, shared by HTTP heartbeats, WebSockets, and People.
use crate::state::{AppState, UserPresence, PRESENCE_FALLBACK_TTL_MS};
use axum::http::HeaderMap;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

const RETAIN_MS: i64 = 300_000;
const MAX_TABS: usize = 32;

#[derive(Clone)]
pub struct TabPresence {
    sequence: u64,
    interaction: u64,
    changed_at: i64,
    seen_at: i64,
    expires_at: i64,
    page: String,
    activity: String,
    visible: bool,
    focused: bool,
    ended: bool,
    socket_id: Option<u64>,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceView {
    pub online: bool,
    pub playing: String,
    pub activity: String,
    pub page: String,
    pub status: String,
    pub last_seen: i64,
    pub last_active: i64,
}

impl PresenceView {
    fn same_activity(&self, other: &Self) -> bool {
        self.online == other.online
            && self.activity == other.activity
            && self.page == other.page
            && self.status == other.status
    }
}

pub fn authenticated_email(state: &AppState, headers: &HeaderMap) -> Option<String> {
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|sid| !sid.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("")
        .to_string();
    if !mitch_lib::auth::valid_id(&sid, &state.id_secret)
        || state.is_revoked_id(&sid)
        || !state.check_password_cookie(headers, Some(&sid))
    {
        return None;
    }
    let email = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid)?;
    if mitch_lib::auth::banned_info_for_email(&state.store, &email).is_some() {
        return None;
    }
    let norm = mitch_lib::auth::normalize_email(&email);
    (!norm.is_empty()).then_some(norm)
}

fn text(value: &str, limit: usize) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(limit)
        .collect()
}

fn page_path(raw: &str) -> Option<String> {
    if !raw.starts_with('/') || raw.starts_with("//") || raw.len() > 2048 || raw.contains('\\') {
        return None;
    }
    let url = url::Url::parse(&format!("https://presence.invalid{raw}")).ok()?;
    Some(url.path().chars().take(240).collect())
}

fn fallback_activity(page: &str) -> String {
    let root = page.trim_matches('/').split('/').next().unwrap_or("");
    match root {
        "" | "index.html" => "On the homepage".into(),
        "matrix" => "In Matrix chat".into(),
        "game-portal" | "games" => "Browsing games".into(),
        "members" => "Viewing people".into(),
        "friends" => "Viewing friends".into(),
        "vms" => "Using Cloud Linux".into(),
        "proxy" if page.starts_with("/proxy/movies") => "Browsing movies".into(),
        _ => format!("Viewing {}", root.replace('-', " ")),
    }
}

fn apply(
    record: &mut UserPresence,
    payload: &Value,
    socket_id: Option<u64>,
    now: i64,
) -> Result<bool, &'static str> {
    let id = payload
        .get("clientId")
        .and_then(Value::as_str)
        .ok_or("missing clientId")?;
    if id.is_empty()
        || id.len() > 80
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err("invalid clientId");
    }
    let sequence = payload
        .get("seq")
        .and_then(Value::as_u64)
        .ok_or("invalid sequence")?;
    if record
        .sessions
        .get(id)
        .is_some_and(|old| sequence <= old.sequence)
    {
        return Ok(false);
    }
    record.hide_online = payload
        .get("hideOnline")
        .and_then(Value::as_bool)
        .unwrap_or(record.hide_online);
    record.share_activity = payload
        .get("shareActivity")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let hide_location = payload
        .get("hideLocation")
        .or_else(|| payload.get("hidePageLocation"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if hide_location {
        record.hide_location = true;
    } else if payload.get("hideLocation").is_some() || payload.get("hidePageLocation").is_some() {
        record.hide_location = false;
    }
    let raw_page = payload.get("page").and_then(Value::as_str).unwrap_or("");
    let page =
        if !record.share_activity || hide_location || record.hide_location || raw_page.is_empty() {
            String::new()
        } else {
            page_path(raw_page).ok_or("invalid page")?
        };
    let visible = payload
        .get("visible")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let focused = visible
        && payload
            .get("focused")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let interaction = payload
        .get("interaction")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let changed_at = match record.sessions.get(id) {
        Some(old)
            if old.page == page
                && old.visible == visible
                && old.focused == focused
                && old.interaction == interaction =>
        {
            old.changed_at
        }
        _ => now,
    };
    let activity = if hide_location || record.hide_location || page.is_empty() {
        String::new()
    } else {
        let act = text(
            payload
                .get("activity")
                .or_else(|| payload.get("playing"))
                .and_then(Value::as_str)
                .unwrap_or(""),
            120,
        );
        if act.is_empty() {
            fallback_activity(&page)
        } else {
            act
        }
    };
    record
        .sessions
        .retain(|_, tab| now - tab.seen_at < RETAIN_MS);
    if record.sessions.len() >= MAX_TABS && !record.sessions.contains_key(id) {
        if let Some(oldest) = record
            .sessions
            .iter()
            .min_by_key(|(_, tab)| tab.seen_at)
            .map(|(id, _)| id.clone())
        {
            record.sessions.remove(&oldest);
        }
    }
    record.sessions.insert(
        id.to_string(),
        TabPresence {
            sequence,
            interaction,
            changed_at,
            seen_at: now,
            expires_at: now + PRESENCE_FALLBACK_TTL_MS,
            page,
            activity,
            visible,
            focused,
            ended: payload
                .get("ended")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            socket_id,
        },
    );
    record.last_seen = now;
    record.modern_seen = now;
    Ok(true)
}

fn view(record: Option<&UserPresence>, socket_seen: i64, now: i64) -> PresenceView {
    let socket_online = socket_seen > 0 && now - socket_seen < PRESENCE_FALLBACK_TTL_MS;
    let Some(record) = record else {
        return PresenceView {
            online: socket_online,
            last_seen: socket_seen,
            status: if socket_online { "online" } else { "offline" }.into(),
            ..Default::default()
        };
    };
    if record.hide_online {
        return PresenceView {
            status: "offline".into(),
            ..Default::default()
        };
    }
    let selected = record
        .sessions
        .values()
        .filter(|tab| !tab.ended && now < tab.expires_at)
        .max_by_key(|tab| (tab.focused, tab.visible, tab.changed_at, &tab.page));
    let legacy_online =
        record.modern_seen == 0 && now - record.last_seen < PRESENCE_FALLBACK_TTL_MS;
    let online = selected.is_some() || legacy_online || (record.modern_seen == 0 && socket_online);
    let (activity, page, status) = if record.hide_location || !record.share_activity {
        (
            String::new(),
            String::new(),
            if let Some(tab) = selected {
                if tab.focused && now - tab.changed_at < 120_000 {
                    "active"
                } else {
                    "away"
                }
            } else if online {
                "online"
            } else {
                "offline"
            },
        )
    } else if let Some(tab) = selected {
        (
            tab.activity.clone(),
            tab.page.clone(),
            if tab.focused && now - tab.changed_at < 120_000 {
                "active"
            } else {
                "away"
            },
        )
    } else if legacy_online {
        let raw = text(&record.playing, 120);
        let page = page_path(&raw).unwrap_or_default();
        let activity = if !page.is_empty() {
            fallback_activity(&page)
        } else if raw == "Dashboard" {
            "On the homepage".into()
        } else {
            raw
        };
        (activity, page, "online")
    } else {
        (
            String::new(),
            String::new(),
            if online { "online" } else { "offline" },
        )
    };
    PresenceView {
        online,
        playing: if online {
            activity.clone()
        } else {
            String::new()
        },
        activity: if online { activity } else { String::new() },
        page: if online { page } else { String::new() },
        status: status.into(),
        last_seen: if record.share_activity && !record.hide_location {
            record.last_seen.max(socket_seen)
        } else {
            0
        },
        last_active: if record.share_activity && !record.hide_location {
            selected.map_or(0, |tab| tab.changed_at)
        } else {
            0
        },
    }
}

pub fn snapshot(state: &AppState, now: i64) -> HashMap<String, PresenceView> {
    let records = state
        .user_presence
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut socket_times = HashMap::<String, i64>::new();
    for socket in state
        .ws_broadcasts
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
    {
        let ended = socket.client_id.as_ref().is_some_and(|id| {
            records
                .get(&socket.email_norm)
                .and_then(|p| p.sessions.get(id))
                .is_some_and(|tab| tab.ended)
        });
        if !ended {
            socket_times
                .entry(socket.email_norm.clone())
                .and_modify(|seen| *seen = (*seen).max(socket.last_seen))
                .or_insert(socket.last_seen);
        }
    }
    let mut result: HashMap<_, _> = records
        .iter()
        .map(|(email, record)| {
            (
                email.clone(),
                view(Some(record), *socket_times.get(email).unwrap_or(&0), now),
            )
        })
        .collect();
    for (email, seen) in socket_times {
        result.entry(email).or_insert_with(|| view(None, seen, now));
    }
    result
}

pub fn current(state: &AppState, email: &str, now: i64) -> PresenceView {
    let norm = mitch_lib::auth::normalize_email(email);
    let records = state
        .user_presence
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let record = records.get(&norm);
    let sockets = state
        .ws_broadcasts
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let seen = sockets
        .values()
        .filter(|socket| {
            socket.email_norm == norm
                && !socket.client_id.as_ref().is_some_and(|id| {
                    record
                        .and_then(|record| record.sessions.get(id))
                        .is_some_and(|tab| tab.ended)
                })
        })
        .map(|socket| socket.last_seen)
        .max()
        .unwrap_or(0);
    view(record, seen, now)
}

pub fn update(
    state: &Arc<AppState>,
    email: &str,
    payload: &Value,
    socket_id: Option<u64>,
) -> Result<(), &'static str> {
    let norm = mitch_lib::auth::normalize_email(email);
    let now = mitch_lib::school::now_millis();
    let was_online = current(state, &norm, now).online;
    {
        let mut records = state
            .user_presence
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !apply(
            records.entry(norm.clone()).or_default(),
            payload,
            socket_id,
            now,
        )? {
            return Ok(());
        }
    }
    if let Some(id) = socket_id {
        if let Some(socket) = state
            .ws_broadcasts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(&id)
        {
            socket.client_id = payload
                .get("clientId")
                .and_then(Value::as_str)
                .map(str::to_string);
            socket.last_seen = now;
        }
    }
    let after = current(state, &norm, now);
    {
        let mut online = state.cv_online.lock().unwrap_or_else(|e| e.into_inner());
        if after.online {
            online.insert(email.to_string(), now);
            online.insert(norm.clone(), now);
        } else {
            online.remove(email);
            online.remove(&norm);
        }
    }
    if !was_online && after.online {
        let st = Arc::clone(state);
        let email = email.to_string();
        tokio::spawn(async move {
            crate::ws::notify_friends_online(&st, &email).await;
        });
    }
    publish(state, &norm, after);
    Ok(())
}

pub fn socket_closed(state: &Arc<AppState>, socket_id: u64) {
    let now = mitch_lib::school::now_millis();
    for record in state
        .user_presence
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values_mut()
    {
        for tab in record
            .sessions
            .values_mut()
            .filter(|tab| tab.socket_id == Some(socket_id))
        {
            tab.expires_at = tab.expires_at.min(now + 3000);
        }
    }
}

pub fn sweep(state: &Arc<AppState>) {
    let now = mitch_lib::school::now_millis();
    for (email, current) in snapshot(state, now) {
        publish(state, &email, current);
    }
    state
        .user_presence
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|_, record| now - record.last_seen < RETAIN_MS);
}

fn publish(state: &Arc<AppState>, email: &str, current: PresenceView) {
    let changed = {
        let mut records = state
            .user_presence
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let record = records.entry(email.to_string()).or_default();
        if record.last_seen == 0 {
            record.last_seen = current.last_seen;
        }
        let changed = record
            .last_reported
            .as_ref()
            .is_none_or(|old| !old.same_activity(&current));
        record.last_reported = Some(current.clone());
        changed
    };
    if changed {
        crate::ws::broadcast_presence_changed(state, email, current.online, &current.activity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn packet(id: &str, seq: u64, page: &str, focused: bool) -> Value {
        json!({"clientId":id,"seq":seq,"page":page,"activity":format!("Viewing {page}"),"visible":true,"focused":focused,"interaction":0,"shareActivity":true})
    }
    #[test]
    fn foreground_tab_wins_over_late_background_heartbeats() {
        let mut record = UserPresence::default();
        apply(&mut record, &packet("home", 1, "/", true), Some(1), 100).unwrap();
        apply(
            &mut record,
            &packet("chat", 1, "/matrix/", true),
            Some(2),
            200,
        )
        .unwrap();
        apply(&mut record, &packet("home", 2, "/", true), Some(1), 300).unwrap();
        assert_eq!(view(Some(&record), 0, 301).page, "/matrix/");
        let mut home = packet("home", 3, "/", true);
        home["interaction"] = json!(1);
        apply(&mut record, &home, Some(1), 400).unwrap();
        assert_eq!(view(Some(&record), 0, 401).page, "/");
    }
    #[test]
    fn ended_documents_cannot_be_revived_by_delayed_requests() {
        let mut record = UserPresence::default();
        apply(&mut record, &packet("a", 5, "/shop/", true), None, 100).unwrap();
        let mut ended = packet("a", 6, "/shop/", true);
        ended["ended"] = json!(true);
        apply(&mut record, &ended, None, 200).unwrap();
        apply(&mut record, &packet("a", 5, "/matrix/", true), None, 300).unwrap();
        let status = view(Some(&record), 0, 301);
        assert!(!status.online);
        assert!(status.page.is_empty());
        apply(&mut record, &packet("a", 7, "/shop/", true), None, 400).unwrap();
        assert_eq!(view(Some(&record), 0, 401).page, "/shop/");
    }
    #[test]
    fn background_and_expired_sessions_do_not_claim_the_current_page() {
        let mut record = UserPresence::default();
        let mut hidden = packet("hidden", 1, "/matrix/", false);
        hidden["visible"] = json!(false);
        apply(&mut record, &hidden, None, 100).unwrap();
        apply(
            &mut record,
            &packet("visible", 1, "/members/", false),
            None,
            200,
        )
        .unwrap();
        let current = view(Some(&record), 0, 201);
        assert_eq!(current.page, "/members/");
        assert_eq!(current.status, "away");
        let expired = view(Some(&record), 0, 200 + PRESENCE_FALLBACK_TTL_MS);
        assert!(!expired.online);
        assert!(expired.activity.is_empty());
        assert!(expired.page.is_empty());
        let connected = view(
            Some(&record),
            200 + PRESENCE_FALLBACK_TTL_MS,
            200 + PRESENCE_FALLBACK_TTL_MS,
        );
        assert!(!connected.online);
        assert!(connected.page.is_empty());
    }
    #[test]
    fn ending_one_tab_falls_back_to_another_without_leaking_query_parameters() {
        let mut record = UserPresence::default();
        apply(
            &mut record,
            &packet("first", 1, "/members/?token=private#private", false),
            None,
            100,
        )
        .unwrap();
        apply(&mut record, &packet("second", 1, "/shop/", true), None, 200).unwrap();
        let mut ended = packet("second", 2, "/shop/", true);
        ended["ended"] = json!(true);
        apply(&mut record, &ended, None, 300).unwrap();
        assert_eq!(view(Some(&record), 0, 301).page, "/members/");
        assert!(apply(
            &mut record,
            &packet("bad", 1, "//other.example/", true),
            None,
            400
        )
        .is_err());
    }
    #[test]
    fn privacy_defaults_and_hide_online_apply_to_every_consumer() {
        let mut record = UserPresence::default();
        let mut payload = packet("private", 1, "/shop/", true);
        payload.as_object_mut().unwrap().remove("shareActivity");
        apply(&mut record, &payload, None, 100).unwrap();
        let visible = view(Some(&record), 0, 101);
        assert!(visible.online);
        assert!(visible.activity.is_empty());
        assert_eq!(visible.last_active, 0);
        payload["seq"] = json!(2);
        payload["hideOnline"] = json!(true);
        apply(&mut record, &payload, None, 200).unwrap();
        let hidden = view(Some(&record), 201, 201);
        assert!(!hidden.online);
        assert_eq!(hidden.last_seen, 0);
        assert!(hidden.page.is_empty());
    }
    #[test]
    fn pure_heartbeats_do_not_broadcast_activity_changes() {
        let mut record = UserPresence::default();
        apply(&mut record, &packet("a", 1, "/members/", true), None, 100).unwrap();
        let before = view(Some(&record), 0, 100);
        apply(&mut record, &packet("a", 2, "/members/", true), None, 200).unwrap();
        assert!(before.same_activity(&view(Some(&record), 0, 200)));
    }
    #[test]
    fn document_storage_is_bounded() {
        let mut record = UserPresence::default();
        for i in 1..=100 {
            apply(
                &mut record,
                &packet(&format!("tab{i}"), 1, "/", true),
                None,
                i,
            )
            .unwrap();
        }
        assert_eq!(record.sessions.len(), MAX_TABS);
        assert!(!record.sessions.contains_key("tab1"));
        assert!(record.sessions.contains_key("tab100"));
    }
    #[test]
    fn hide_location_suppresses_page_and_activity_for_all_viewers() {
        let mut record = UserPresence::default();
        let mut pkt = packet("tab1", 1, "/shop/", true);
        pkt["hideLocation"] = json!(true);
        apply(&mut record, &pkt, None, 100).unwrap();
        let v = view(Some(&record), 0, 101);
        assert!(v.online);
        assert_eq!(v.page, "");
        assert_eq!(v.activity, "");
        assert_eq!(v.playing, "");
        assert_eq!(v.status, "active");
    }
}
