//! Matrix Conduit & Mitch.pro SSO Integration, Moderation, and Reverse Proxy.
//!
//! Ports:
//! - server.js:8006-8705 (Conduit call, SSO, room sync, moderation helpers, outbound push)
//! - server.js:9574-10021 (Well-known discovery, VOIP discovery, and /_matrix/* reverse proxy)
//! - server.js:10002-10227 (Cinny config, SSO login/status, notifications read, report room)
//! - server.js:14228-14690 (Matrix moderation API suite: overview, set-role, kick, ban, redact, slowmode, mute-user, unmute-user, mute-room, prune-stale)

#![allow(clippy::expect_used)]

use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::hosts::request_host;
use crate::state::AppState;

/// Browser-like UA for fetching GIFs/stickers from Tenor/Giphy — their CDNs
/// hotlink-block or throttle reqwest's default "reqwest/x.y.z" UA.
const GIF_FETCH_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36";

pub const OFFICIAL_ROOMS: &[(&str, &str, &str)] = &[
    (
        "general",
        "General",
        "Welcome to Mitch.pro Official Matrix Chat!",
    ),
    (
        "tech",
        "Tech",
        "Technology, software development, coding, and projects",
    ),
    (
        "python-hate",
        "Python Hate",
        "Discussions about Python limitations, quirks, and alternatives",
    ),
    (
        "rust",
        "Rust",
        "Rust programming, systems programming, and performance",
    ),
    (
        "memory-safe",
        "Memory Safe",
        "Memory safety, verified systems, and modern safe languages",
    ),
    (
        "biking",
        "Biking",
        "Cycling, bikes, trails, maintenance, and gear",
    ),
    (
        "gaming",
        "Gaming",
        "Video games, arcade high scores, speedruns, and tips",
    ),
    (
        "computers",
        "Computers",
        "PC hardware, Linux, VMs, custom builds, and setups",
    ),
    (
        "random",
        "Random",
        "Off-topic discussions, casual chat, and memes",
    ),
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatrixAccount {
    pub uid: String,
    pub norm_email: String,
    pub user_id: String,
}

static TOKEN_TO_ACCOUNT: OnceLock<Mutex<HashMap<String, MatrixAccount>>> = OnceLock::new();
static ROOM_LAST_MESSAGE_TIMES: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
static OFFICIAL_ROOM_IDS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
static SYSTEM_ADMIN_TOKEN: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn token_to_account_map() -> &'static Mutex<HashMap<String, MatrixAccount>> {
    TOKEN_TO_ACCOUNT.get_or_init(|| Mutex::new(HashMap::new()))
}

fn room_last_message_map() -> &'static Mutex<HashMap<String, i64>> {
    ROOM_LAST_MESSAGE_TIMES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn official_room_id_map() -> &'static Mutex<HashMap<String, String>> {
    OFFICIAL_ROOM_IDS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn system_admin_token_cell() -> &'static Mutex<Option<String>> {
    SYSTEM_ADMIN_TOKEN.get_or_init(|| Mutex::new(None))
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn cors_response(status: StatusCode, body: Bytes, content_type: Option<&str>) -> Response {
    let mut builder = Response::builder()
        .status(status)
        .header("Access-Control-Allow-Origin", "*")
        .header(
            "Access-Control-Allow-Methods",
            "GET, POST, PUT, DELETE, OPTIONS",
        )
        .header(
            "Access-Control-Allow-Headers",
            "Origin, X-Requested-With, Content-Type, Accept, Authorization",
        )
        .header("Access-Control-Max-Age", "86400");
    if let Some(ct) = content_type {
        builder = builder.header("Content-Type", ct);
    }
    builder
        .body(axum::body::Body::from(body))
        .unwrap_or_else(|_| {
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(axum::body::Body::empty())
                .expect("static empty response")
        })
}

fn cors_json_response(status: u16, val: Value) -> Response {
    let body = serde_json::to_vec(&val).unwrap_or_default();
    cors_response(
        StatusCode::from_u16(status).unwrap_or(StatusCode::OK),
        Bytes::from(body),
        Some("application/json; charset=utf-8"),
    )
}

fn proxy_response_with_cors(
    status: StatusCode,
    upstream_headers: &HeaderMap,
    body: Bytes,
) -> Response {
    let mut builder = Response::builder()
        .status(status)
        .header("Access-Control-Allow-Origin", "*")
        .header(
            "Access-Control-Allow-Methods",
            "GET, POST, PUT, DELETE, OPTIONS",
        )
        .header(
            "Access-Control-Allow-Headers",
            "Origin, X-Requested-With, Content-Type, Accept, Authorization",
        )
        .header("Access-Control-Max-Age", "86400");

    if let Some(headers_mut) = builder.headers_mut() {
        for (name, value) in upstream_headers.iter() {
            let lower = name.as_str().to_ascii_lowercase();
            if lower == "connection"
                || lower == "keep-alive"
                || lower == "proxy-authenticate"
                || lower == "proxy-authorization"
                || lower == "te"
                || lower == "trailers"
                || lower == "transfer-encoding"
                || lower == "upgrade"
                || lower == "content-length"
                || lower.starts_with("access-control-")
            {
                continue;
            }
            headers_mut.insert(name.clone(), value.clone());
        }
    }

    builder
        .body(axum::body::Body::from(body))
        .unwrap_or_else(|_| {
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(axum::body::Body::empty())
                .expect("static empty response")
        })
}

fn unreachable_regex() -> regex::Regex {
    regex::Regex::new("a^").unwrap_or_else(|_| match regex::Regex::new("") {
        Ok(r) => r,
        Err(_) => unreachable!(),
    })
}

pub fn get_matrix_password_for_uid(uid: &str, secret: &[u8]) -> String {
    mitch_lib::crypto::hmac_sha256_hex(secret, format!("matrix-account:{uid}").as_bytes())
}

pub fn sanitize_matrix_device_id(value: &str) -> String {
    let trimmed = value.trim();
    static DEVICE_RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = DEVICE_RE.get_or_init(|| {
        regex::Regex::new(r"^[A-Za-z0-9._~-]{1,255}$").unwrap_or_else(|_| unreachable_regex())
    });
    if re.is_match(trimmed) {
        trimmed.to_string()
    } else {
        String::new()
    }
}

pub fn get_matrix_power_level_for_sid(state: &AppState, sid: &str) -> i64 {
    if sid.is_empty() {
        return 0;
    }
    if let Some(email) = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, sid) {
        if mitch_lib::auth::is_owner_email(&state.store, &email) {
            return 100;
        }
        if mitch_lib::auth::is_admin_id(&state.store, &state.id_secret, sid, false) {
            return 100;
        }
        if mitch_lib::auth::is_moderator_email(&state.store, &email) {
            return 50;
        }
    }
    0
}

fn conduit_candidate_hosts() -> Vec<String> {
    let conduit_host = std::env::var("CONDUIT_HOST").unwrap_or_else(|_| {
        if std::env::var("DOCKER_ENV").unwrap_or_default() == "1"
            || std::path::Path::new("/.dockerenv").exists()
        {
            "conduit".to_string()
        } else {
            "127.0.0.1".to_string()
        }
    });

    let mut hosts = vec![conduit_host.clone()];
    if conduit_host == "127.0.0.1" {
        hosts.push("conduit".to_string());
    } else {
        hosts.push("127.0.0.1".to_string());
    }
    hosts.push("mitch-matrix-conduit".to_string());
    hosts.dedup();
    hosts
}

fn conduit_port() -> String {
    std::env::var("CONDUIT_PORT").unwrap_or_else(|_| "6167".to_string())
}

static CONDUIT_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn conduit_client() -> &'static reqwest::Client {
    CONDUIT_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .pool_max_idle_per_host(0)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

pub async fn call_conduit(
    subpath: &str,
    method: Method,
    headers: Option<HeaderMap>,
    body: Option<Bytes>,
) -> Result<(StatusCode, HeaderMap, Bytes), String> {
    call_conduit_with_timeout(
        subpath,
        method,
        headers,
        body,
        Some(std::time::Duration::from_secs(30)),
    )
    .await
}

pub async fn call_conduit_with_timeout(
    subpath: &str,
    method: Method,
    headers: Option<HeaderMap>,
    body: Option<Bytes>,
    timeout: Option<std::time::Duration>,
) -> Result<(StatusCode, HeaderMap, Bytes), String> {
    let port = conduit_port();
    let client = conduit_client();

    let req_method = match method {
        Method::POST => reqwest::Method::POST,
        Method::PUT => reqwest::Method::PUT,
        Method::DELETE => reqwest::Method::DELETE,
        _ => reqwest::Method::GET,
    };

    let mut last_err = String::from("No candidate hosts reachable");
    for host in conduit_candidate_hosts() {
        let url = format!("http://{host}:{port}{subpath}");
        let mut rb = client.request(req_method.clone(), &url);
        if let Some(to) = timeout {
            rb = rb.timeout(to);
        }
        rb = rb.header("Host", "mitch.pro");
        rb = rb.header("Connection", "close");

        if let Some(ref h) = headers {
            for (k, v) in h.iter() {
                let name = k.as_str().to_lowercase();
                if name != "host"
                    && name != "content-length"
                    && name != "connection"
                    && name != "keep-alive"
                    && name != "transfer-encoding"
                    && name != "upgrade"
                {
                    rb = rb.header(k.as_str(), v.as_bytes());
                }
            }
        }

        if let Some(ref b) = body {
            rb = rb.body(b.clone());
        }

        match rb.send().await {
            Ok(resp) => {
                let status =
                    StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
                let mut out_headers = HeaderMap::new();
                for (k, v) in resp.headers().iter() {
                    if let (Ok(name), Ok(val)) = (
                        axum::http::HeaderName::from_bytes(k.as_str().as_bytes()),
                        HeaderValue::from_bytes(v.as_bytes()),
                    ) {
                        out_headers.insert(name, val);
                    }
                }
                let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
                return Ok((status, out_headers, bytes));
            }
            Err(e) => {
                last_err = e.to_string();
            }
        }
    }

    Err(last_err)
}

pub async fn get_system_admin_matrix_token(secret: &[u8]) -> Result<String, String> {
    {
        let cell = system_admin_token_cell()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(ref tok) = *cell {
            return Ok(tok.clone());
        }
    }

    let admin_username = "mitch_admin";
    let admin_password = mitch_lib::crypto::hmac_sha256_hex(secret, b"matrix-sysadmin-2026");

    let login_payload = json!({
        "type": "m.login.password",
        "identifier": { "type": "m.id.user", "user": admin_username },
        "password": admin_password,
        "initial_device_display_name": "Mitch.pro System Admin"
    });

    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_static("application/json"));

    let res = call_conduit(
        "/_matrix/client/v3/login",
        Method::POST,
        Some(headers.clone()),
        Some(Bytes::from(
            serde_json::to_vec(&login_payload).unwrap_or_default(),
        )),
    )
    .await;

    let token = match res {
        Ok((status, _, bytes)) if status.is_success() => {
            let data: Value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
            data.get("access_token")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        }
        _ => None,
    };

    if let Some(t) = token {
        let mut cell = system_admin_token_cell()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *cell = Some(t.clone());
        return Ok(t);
    }

    // Attempt to register system admin if login failed
    let reg_payload = json!({
        "username": admin_username,
        "password": admin_password,
        "auth": { "type": "m.login.dummy" }
    });
    let reg_res = call_conduit(
        "/_matrix/client/v3/register",
        Method::POST,
        Some(headers),
        Some(Bytes::from(
            serde_json::to_vec(&reg_payload).unwrap_or_default(),
        )),
    )
    .await?;

    if reg_res.0.is_success() {
        let data: Value = serde_json::from_slice(&reg_res.2).unwrap_or(json!({}));
        if let Some(t) = data.get("access_token").and_then(|v| v.as_str()) {
            let mut cell = system_admin_token_cell()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *cell = Some(t.to_string());
            return Ok(t.to_string());
        }
    }

    Err("Failed to acquire system admin Matrix token".to_string())
}

pub async fn ensure_official_room(
    secret: &[u8],
    alias: &str,
    name: &str,
    topic: &str,
) -> Result<String, String> {
    {
        let cache = official_room_id_map()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(id) = cache.get(alias) {
            return Ok(id.clone());
        }
    }

    let full_alias = format!("#{alias}:mitch.pro");

    // 1. Check directory
    if let Ok((status, _, bytes)) = call_conduit(
        &format!(
            "/_matrix/client/v3/directory/room/{}",
            url::form_urlencoded::byte_serialize(full_alias.as_bytes()).collect::<String>()
        ),
        Method::GET,
        None,
        None,
    )
    .await
    {
        if status.is_success() {
            let data: Value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
            if let Some(room_id) = data.get("room_id").and_then(|v| v.as_str()) {
                let mut cache = official_room_id_map()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                cache.insert(alias.to_string(), room_id.to_string());
                return Ok(room_id.to_string());
            }
        }
    }

    // 2. Create room
    let admin_tok = get_system_admin_matrix_token(secret).await?;
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_static("application/json"));
    if let Ok(hv) = HeaderValue::from_str(&format!("Bearer {admin_tok}")) {
        headers.insert("Authorization", hv);
    }

    let create_payload = json!({
        "room_version": "10",
        "name": name,
        "topic": topic,
        "room_alias_name": alias,
        "visibility": "public",
        "preset": "public_chat",
        "initial_state": [
            {
                "type": "m.room.history_visibility",
                "state_key": "",
                "content": { "history_visibility": "world_readable" }
            },
            {
                "type": "m.room.guest_access",
                "state_key": "",
                "content": { "guest_access": "can_join" }
            }
        ]
    });

    if let Ok((status, _, bytes)) = call_conduit(
        "/_matrix/client/v3/createRoom",
        Method::POST,
        Some(headers),
        Some(Bytes::from(
            serde_json::to_vec(&create_payload).unwrap_or_default(),
        )),
    )
    .await
    {
        if status.is_success() {
            let data: Value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
            if let Some(room_id) = data.get("room_id").and_then(|v| v.as_str()) {
                let mut cache = official_room_id_map()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                cache.insert(alias.to_string(), room_id.to_string());
                return Ok(room_id.to_string());
            }
        }
    }

    // 3. Fallback directory query
    if let Ok((status, _, bytes)) = call_conduit(
        &format!(
            "/_matrix/client/v3/directory/room/{}",
            url::form_urlencoded::byte_serialize(full_alias.as_bytes()).collect::<String>()
        ),
        Method::GET,
        None,
        None,
    )
    .await
    {
        if status.is_success() {
            let data: Value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
            if let Some(room_id) = data.get("room_id").and_then(|v| v.as_str()) {
                let mut cache = official_room_id_map()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                cache.insert(alias.to_string(), room_id.to_string());
                return Ok(room_id.to_string());
            }
        }
    }

    Err(format!("Could not ensure official room {alias}"))
}

pub async fn ensure_official_general_room(secret: &[u8]) -> Result<String, String> {
    ensure_official_room(
        secret,
        "general",
        "General",
        "Welcome to Mitch.pro Official Matrix Chat!",
    )
    .await
}

pub async fn sync_matrix_user_to_official_rooms(
    secret: &[u8],
    user_id: &str,
    user_token: &str,
    target_power_level: i64,
) {
    let space_alias = url::form_urlencoded::byte_serialize(b"#mitch.pro:mitch.pro")
        .collect::<String>();
    let space_id = if let Ok((status, _, bytes)) = call_conduit(
        &format!("/_matrix/client/v3/directory/room/{space_alias}"),
        Method::GET,
        None,
        None,
    )
    .await
    {
        if status.is_success() {
            serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|data| data["room_id"].as_str().map(str::to_owned))
        } else {
            None
        }
    } else {
        None
    };
    let admin_token = get_system_admin_matrix_token(secret).await.ok();
    if let Some(space_id) = &space_id {
        let encoded_space = url::form_urlencoded::byte_serialize(space_id.as_bytes())
            .collect::<String>();
        for token in [Some(user_token), admin_token.as_deref()].into_iter().flatten() {
            if token.is_empty() {
                continue;
            }
            let mut headers = HeaderMap::new();
            headers.insert("Content-Type", HeaderValue::from_static("application/json"));
            if let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}")) {
                headers.insert("Authorization", value);
            }
            let _ = call_conduit(
                &format!("/_matrix/client/v3/join/{encoded_space}"),
                Method::POST,
                Some(headers),
                Some(Bytes::from_static(b"{}")),
            )
            .await;
        }
    }

    for (alias, name, topic) in OFFICIAL_ROOMS {
        let Ok(room_id) = ensure_official_room(secret, alias, name, topic).await else {
            continue;
        };

        if let (Some(space_id), Some(admin_token)) = (&space_id, &admin_token) {
            let encoded_space = url::form_urlencoded::byte_serialize(space_id.as_bytes())
                .collect::<String>();
            let encoded_room = url::form_urlencoded::byte_serialize(room_id.as_bytes())
                .collect::<String>();
            let mut headers = HeaderMap::new();
            headers.insert("Content-Type", HeaderValue::from_static("application/json"));
            if let Ok(value) = HeaderValue::from_str(&format!("Bearer {admin_token}")) {
                headers.insert("Authorization", value);
            }
            let child_path = format!(
                "/_matrix/client/v3/rooms/{encoded_space}/state/m.space.child/{encoded_room}"
            );
            let child_exists = matches!(
                call_conduit(&child_path, Method::GET, Some(headers.clone()), None).await,
                Ok((status, _, _)) if status.is_success()
            );
            if !child_exists {
                let _ = call_conduit(
                    &child_path,
                    Method::PUT,
                    Some(headers),
                    Some(Bytes::from_static(b"{\"via\":[\"mitch.pro\"]}")),
                )
                .await;
            }
        }

        // 1. Join user to official room
        if !user_token.is_empty() {
            let encoded_room =
                url::form_urlencoded::byte_serialize(room_id.as_bytes()).collect::<String>();
            let mut join_headers = HeaderMap::new();
            join_headers.insert("Content-Type", HeaderValue::from_static("application/json"));
            if let Ok(hv) = HeaderValue::from_str(&format!("Bearer {user_token}")) {
                join_headers.insert("Authorization", hv);
            }
            let _ = call_conduit(
                &format!("/_matrix/client/v3/join/{encoded_room}"),
                Method::POST,
                Some(join_headers),
                Some(Bytes::from_static(b"{}")),
            )
            .await;
        }

        // 2. Fetch current power levels
        let Some(admin_token) = &admin_token else {
            continue;
        };
        let encoded_room =
            url::form_urlencoded::byte_serialize(room_id.as_bytes()).collect::<String>();
        let mut pl_headers = HeaderMap::new();
        if let Ok(hv) = HeaderValue::from_str(&format!("Bearer {admin_token}")) {
            pl_headers.insert("Authorization", hv);
        }
        if let Ok((status, _, bytes)) = call_conduit(
            &format!("/_matrix/client/v3/rooms/{encoded_room}/state/m.room.power_levels"),
            Method::GET,
            Some(pl_headers.clone()),
            None,
        )
        .await
        {
            if status.is_success() {
                let mut pl_data: Value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
                if !pl_data.is_object() {
                    pl_data = json!({});
                }
                if pl_data.get("users").and_then(|v| v.as_object()).is_none() {
                    pl_data["users"] = json!({});
                }
                if pl_data.get("events").and_then(|v| v.as_object()).is_none() {
                    pl_data["events"] = json!({});
                }
                let mut pl_changed = false;
                for call_ev in &[
                    "org.matrix.msc3401.call.member",
                    "org.matrix.msc3401.call",
                    "org.matrix.msc4143.rtc.member",
                ] {
                    if pl_data["events"].get(call_ev).and_then(|v| v.as_i64()) != Some(0) {
                        pl_data["events"][call_ev] = json!(0);
                        pl_changed = true;
                    }
                }
                let current_pl = pl_data["users"]
                    .get(user_id)
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                if current_pl != target_power_level {
                    if target_power_level > 0 {
                        pl_data["users"][user_id] = json!(target_power_level);
                    } else if let Some(users_obj) = pl_data["users"].as_object_mut() {
                        users_obj.remove(user_id);
                    }
                    pl_changed = true;
                }
                if pl_changed {
                    let mut put_headers = HeaderMap::new();
                    put_headers
                        .insert("Content-Type", HeaderValue::from_static("application/json"));
                    if let Ok(hv) = HeaderValue::from_str(&format!("Bearer {admin_token}")) {
                        put_headers.insert("Authorization", hv);
                    }
                    let _ = call_conduit(
                        &format!(
                            "/_matrix/client/v3/rooms/{encoded_room}/state/m.room.power_levels"
                        ),
                        Method::PUT,
                        Some(put_headers),
                        Some(Bytes::from(
                            serde_json::to_vec(&pl_data).unwrap_or_default(),
                        )),
                    )
                    .await;
                }
            }
        }
    }
}

pub fn resolve_matrix_user_id_from_store(
    store: &mitch_lib::data::DataStore,
    data_dir: &std::path::Path,
    secret: &[u8],
    norm_email: &str,
) -> String {
    let norm = mitch_lib::auth::normalize_email(norm_email);
    let profiles_file = data_dir.join("profiles.json");
    let profiles = store.read_document(&profiles_file, json!({}));
    if let Some(prof) = profiles.get(&norm) {
        if let Some(uname) = prof.get("username").and_then(|v| v.as_str()) {
            if !uname.is_empty() {
                return format!("@{uname}:mitch.pro");
            }
        }
    }
    let matrix_users_file = data_dir.join("matrix_users.json");
    let matrix_users = store.read_document(&matrix_users_file, json!({}));
    let names_file = store.base_dir.join("data/names.json");
    let names = store.read_document(&names_file, json!({}));
    if let Some(map) = matrix_users.as_object() {
        for (uid, uname_val) in map {
            let matched_email = mitch_lib::auth::email_from_sid(store, secret, uid)
                .or_else(|| names.get(uid).and_then(|v| v.as_str()).map(str::to_string));
            if let Some(email) = matched_email {
                if mitch_lib::auth::normalize_email(&email) == norm {
                    if let Some(uname) = uname_val.as_str() {
                        return format!("@{uname}:mitch.pro");
                    }
                }
            }
        }
    }
    let local = norm.split('@').next().unwrap_or("user");
    format!("@{local}:mitch.pro")
}

pub fn resolve_matrix_user_id_for_email(state: &AppState, norm_email: &str) -> String {
    resolve_matrix_user_id_from_store(&state.store, &state.data_dir(), &state.id_secret, norm_email)
}

fn push_matrix_user_id(results: &mut Vec<String>, id: &str) {
    let clean = id.trim();
    if clean.is_empty() {
        return;
    }
    let full_id = if clean.starts_with('@') && clean.contains(':') {
        clean.to_string()
    } else {
        let u = clean.trim_start_matches('@').split(':').next().unwrap_or(clean);
        format!("@{u}:mitch.pro")
    };
    let normalized: String = full_id.chars().filter(|c| !c.is_whitespace()).collect();
    if !results.contains(&normalized) {
        results.push(normalized);
    }
}

fn push_email(results: &mut Vec<String>, email: &str) {
    let clean = email.trim();
    if clean.is_empty() || !clean.contains('@') {
        return;
    }
    let norm = mitch_lib::auth::normalize_email(clean);
    if !results.contains(&norm) {
        results.push(norm);
    }
    let lower = clean.to_lowercase();
    if !results.contains(&lower) {
        results.push(lower);
    }
}

pub fn resolve_all_emails_for_target(
    store: &mitch_lib::data::DataStore,
    data_dir: &std::path::Path,
    secret: &[u8],
    input: &str,
) -> Vec<String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Vec::new();
    }

    let mut results = Vec::new();

    if raw.contains('@') && !raw.starts_with('@') {
        push_email(&mut results, raw);
    }

    let stripped = raw.trim_start_matches('@');
    let local_raw = stripped.split(':').next().unwrap_or(stripped);
    let clean_cmp: String = local_raw.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();

    // 1. Profiles
    let profiles_file = data_dir.join("profiles.json");
    let profiles = store.read_document(&profiles_file, json!({}));
    if let Some(map) = profiles.as_object() {
        for (email_key, prof) in map {
            let p_email = mitch_lib::auth::normalize_email(email_key);
            let raw_email = prof.get("email").and_then(|v| v.as_str()).unwrap_or(email_key);
            let p_uname = prof.get("username").and_then(|v| v.as_str()).unwrap_or("");
            let p_display = prof.get("displayName").or_else(|| prof.get("nickname")).and_then(|v| v.as_str()).unwrap_or("");

            let p_uname_clean: String = p_uname.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
            let p_display_clean: String = p_display.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
            let p_email_local = p_email.split('@').next().unwrap_or("");
            let p_email_clean: String = p_email_local.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();

            let matched = (!clean_cmp.is_empty() && (
                clean_cmp == p_uname_clean
                || clean_cmp == p_display_clean
                || clean_cmp == p_email_clean
                || p_display.eq_ignore_ascii_case(local_raw)
                || p_uname.eq_ignore_ascii_case(local_raw)
            )) || raw.eq_ignore_ascii_case(&p_email)
               || raw.eq_ignore_ascii_case(email_key)
               || raw.eq_ignore_ascii_case(raw_email);

            if matched {
                push_email(&mut results, &p_email);
                push_email(&mut results, email_key);
                push_email(&mut results, raw_email);
            }
        }
    }

    // 2. matrix_users.json and names.json
    let matrix_users_file = data_dir.join("matrix_users.json");
    let matrix_users = store.read_document(&matrix_users_file, json!({}));
    if let Some(map) = matrix_users.as_object() {
        let names_file = store.base_dir.join("data/names.json");
        let names = store.read_document(&names_file, json!({}));

        for (uid_or_key, uname_val) in map {
            let uname = uname_val.as_str().unwrap_or("");
            let uname_clean: String = uname.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();

            let matched = !uname.is_empty() && (!clean_cmp.is_empty() && uname_clean == clean_cmp || uname.eq_ignore_ascii_case(local_raw));
            if matched {
                if let Some(email) = mitch_lib::auth::email_from_sid(store, secret, uid_or_key)
                    .or_else(|| names.get(uid_or_key).and_then(|v| v.as_str()).map(str::to_string)) {
                    push_email(&mut results, &email);
                }
            }
        }
    }

    // 3. Blacklist files
    for bl_path in [data_dir.join("blacklist.json"), store.base_dir.join("data/blacklist.json")] {
        let bl = store.read_document(&bl_path, json!({}));
        if let Some(map) = bl.as_object() {
            for (email_key, _) in map {
                let local = email_key.split('@').next().unwrap_or("");
                let local_clean: String = local.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
                if !clean_cmp.is_empty() && (local_clean == clean_cmp || local.eq_ignore_ascii_case(local_raw)) {
                    push_email(&mut results, email_key);
                }
            }
        }
    }

    // 4. Shadow bans
    let sb_file = store.base_dir.join("data/shadow_bans.json");
    let sb = store.read_document(&sb_file, json!([]));
    if let Some(arr) = sb.as_array() {
        for v in arr {
            if let Some(s) = v.as_str() {
                let local = s.split('@').next().unwrap_or("");
                let local_clean: String = local.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
                if !clean_cmp.is_empty() && (local_clean == clean_cmp || local.eq_ignore_ascii_case(local_raw)) {
                    push_email(&mut results, s);
                }
            }
        }
    }

    results
}

pub fn resolve_all_matrix_user_ids_from_store(
    store: &mitch_lib::data::DataStore,
    data_dir: &std::path::Path,
    secret: &[u8],
    input: &str,
) -> Vec<String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Vec::new();
    }

    let mut results = Vec::new();

    if raw.starts_with('@') && !raw.contains(' ') {
        push_matrix_user_id(&mut results, raw);
    }

    let stripped = raw.trim_start_matches('@');
    let local_raw = stripped.split(':').next().unwrap_or(stripped);

    let email_candidate = if raw.contains('@') && !raw.starts_with('@') {
        Some(mitch_lib::auth::normalize_email(raw))
    } else if local_raw.contains('@') {
        Some(mitch_lib::auth::normalize_email(local_raw))
    } else {
        None
    };

    // 1. Check profiles.json for exact or fuzzy matches (username, displayName, nickname, email)
    let profiles_file = data_dir.join("profiles.json");
    let profiles = store.read_document(&profiles_file, json!({}));
    if let Some(map) = profiles.as_object() {
        let clean_cmp: String = local_raw.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
        for (email_key, prof) in map {
            let p_email = mitch_lib::auth::normalize_email(email_key);
            let p_uname = prof.get("username").and_then(|v| v.as_str()).unwrap_or("");
            let p_display = prof.get("displayName").or_else(|| prof.get("nickname")).and_then(|v| v.as_str()).unwrap_or("");

            let p_uname_clean: String = p_uname.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
            let p_display_clean: String = p_display.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
            let p_email_local = p_email.split('@').next().unwrap_or("");
            let p_email_clean: String = p_email_local.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();

            let matched = (!clean_cmp.is_empty() && (
                clean_cmp == p_uname_clean
                || clean_cmp == p_display_clean
                || clean_cmp == p_email_clean
                || p_display.eq_ignore_ascii_case(local_raw)
                || p_uname.eq_ignore_ascii_case(local_raw)
            )) || email_candidate.as_deref() == Some(&p_email);

            if matched {
                if !p_uname.is_empty() {
                    push_matrix_user_id(&mut results, p_uname);
                }
                if !p_email_local.is_empty() {
                    push_matrix_user_id(&mut results, p_email_local);
                }
            }
        }
    }

    // 2. Check matrix_users.json
    let matrix_users_file = data_dir.join("matrix_users.json");
    let matrix_users = store.read_document(&matrix_users_file, json!({}));
    if let Some(map) = matrix_users.as_object() {
        let names_file = store.base_dir.join("data/names.json");
        let names = store.read_document(&names_file, json!({}));
        let clean_cmp: String = local_raw.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();

        for (uid_or_key, uname_val) in map {
            let uname = uname_val.as_str().unwrap_or("");
            let uname_clean: String = uname.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();

            if !uname.is_empty() && (!clean_cmp.is_empty() && uname_clean == clean_cmp || uname.eq_ignore_ascii_case(local_raw)) {
                push_matrix_user_id(&mut results, uname);
            }

            let associated_email = mitch_lib::auth::email_from_sid(store, secret, uid_or_key)
                .or_else(|| names.get(uid_or_key).and_then(|v| v.as_str()).map(str::to_string));
            if let Some(email) = associated_email {
                let norm_assoc = mitch_lib::auth::normalize_email(&email);
                let assoc_local = norm_assoc.split('@').next().unwrap_or("");
                let assoc_clean: String = assoc_local.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
                if email_candidate.as_deref() == Some(&norm_assoc) || (!clean_cmp.is_empty() && clean_cmp == assoc_clean) {
                    if !uname.is_empty() {
                        push_matrix_user_id(&mut results, uname);
                    }
                    push_matrix_user_id(&mut results, assoc_local);
                }
            }
        }
    }

    // 3. Generate standard formatting variations (collapsed, dotted, dashed, underscored)
    let words: Vec<&str> = local_raw.split(|c: char| c.is_whitespace() || c == '.' || c == '-' || c == '_')
        .filter(|w| !w.is_empty())
        .collect();
    if !words.is_empty() {
        let collapsed = words.join("").to_lowercase();
        let dotted = words.join(".").to_lowercase();
        let dashed = words.join("-").to_lowercase();
        let underscored = words.join("_").to_lowercase();

        push_matrix_user_id(&mut results, &collapsed);
        push_matrix_user_id(&mut results, &dotted);
        push_matrix_user_id(&mut results, &dashed);
        push_matrix_user_id(&mut results, &underscored);
    }

    // 4. Check associated emails
    let emails = resolve_all_emails_for_target(store, data_dir, secret, input);
    for em in emails {
        let local = em.split('@').next().unwrap_or("");
        if !local.is_empty() {
            push_matrix_user_id(&mut results, local);
        }
    }

    // 5. Check matrix_room_settings.json for existing bannedUsers or mutedUsers
    let settings_file = data_dir.join("matrix_room_settings.json");
    let room_settings = store.read_document(&settings_file, json!({}));
    if let Some(rooms) = room_settings.as_object() {
        let clean_cmp: String = local_raw.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
        for (_, room) in rooms {
            for list_key in ["bannedUsers", "mutedUsers"] {
                if let Some(users_map) = room.get(list_key).and_then(|v| v.as_object()) {
                    for (k, val) in users_map {
                        let uid = val.get("userId").and_then(|v| v.as_str()).unwrap_or(k);
                        let stripped_u = uid.trim_start_matches('@').split(':').next().unwrap_or(uid);
                        let u_clean: String = stripped_u.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
                        if !clean_cmp.is_empty() && (u_clean == clean_cmp || stripped_u.eq_ignore_ascii_case(local_raw)) {
                            push_matrix_user_id(&mut results, uid);
                        }
                    }
                }
            }
        }
    }

    if results.is_empty() {
        let clean: String = local_raw.chars().filter(|c| !c.is_whitespace()).collect();
        push_matrix_user_id(&mut results, &clean);
    }

    results
}

pub fn resolve_all_matrix_user_ids_for_target(state: &AppState, input: &str) -> Vec<String> {
    resolve_all_matrix_user_ids_from_store(&state.store, &state.data_dir(), &state.id_secret, input)
}

pub async fn sync_staff_power_levels_to_all_official_rooms(
    secret: &[u8],
    store: &mitch_lib::data::DataStore,
    data_dir: &std::path::Path,
) {
    let admin_tok = match get_system_admin_matrix_token(secret).await {
        Ok(t) => t,
        Err(_) => return,
    };

    let mut staff_power_levels: std::collections::HashMap<String, i64> =
        std::collections::HashMap::new();

    // Admins, owners, co-owners (PL 100)
    for admin_email in mitch_lib::auth::site_admin_emails(store) {
        let norm = mitch_lib::auth::normalize_email(&admin_email);
        let user_id = resolve_matrix_user_id_from_store(store, data_dir, secret, &norm);
        staff_power_levels.insert(user_id, 100);
    }

    // Moderators (PL 50)
    for mod_email in mitch_lib::auth::moderator_emails(store) {
        let norm = mitch_lib::auth::normalize_email(&mod_email);
        let user_id = resolve_matrix_user_id_from_store(store, data_dir, secret, &norm);
        staff_power_levels.entry(user_id).or_insert(50);
    }

    staff_power_levels.insert("@admin:mitch.pro".to_string(), 100);

    for (alias, name, topic) in OFFICIAL_ROOMS {
        let Ok(room_id) = ensure_official_room(secret, alias, name, topic).await else {
            continue;
        };

        let encoded_room =
            url::form_urlencoded::byte_serialize(room_id.as_bytes()).collect::<String>();
        let mut pl_headers = HeaderMap::new();
        if let Ok(hv) = HeaderValue::from_str(&format!("Bearer {admin_tok}")) {
            pl_headers.insert("Authorization", hv);
        }

        if let Ok((status, _, bytes)) = call_conduit(
            &format!("/_matrix/client/v3/rooms/{encoded_room}/state/m.room.power_levels"),
            Method::GET,
            Some(pl_headers.clone()),
            None,
        )
        .await
        {
            if status.is_success() {
                let mut pl_data: Value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
                if !pl_data.is_object() {
                    pl_data = json!({});
                }
                if pl_data.get("users").and_then(|v| v.as_object()).is_none() {
                    pl_data["users"] = json!({});
                }
                let mut changed = false;
                if let Some(users_map) = pl_data.get_mut("users").and_then(|v| v.as_object_mut()) {
                    for (staff_user, &target_pl) in &staff_power_levels {
                        let cur = users_map.get(staff_user).and_then(|v| v.as_i64()).unwrap_or(0);
                        if cur != target_pl {
                            users_map.insert(staff_user.clone(), json!(target_pl));
                            changed = true;
                        }
                    }
                }

                if changed {
                    let mut put_headers = HeaderMap::new();
                    put_headers
                        .insert("Content-Type", HeaderValue::from_static("application/json"));
                    if let Ok(hv) = HeaderValue::from_str(&format!("Bearer {admin_tok}")) {
                        put_headers.insert("Authorization", hv);
                    }
                    let _ = call_conduit(
                        &format!(
                            "/_matrix/client/v3/rooms/{encoded_room}/state/m.room.power_levels"
                        ),
                        Method::PUT,
                        Some(put_headers),
                        Some(Bytes::from(
                            serde_json::to_vec(&pl_data).unwrap_or_default(),
                        )),
                    )
                    .await;
                }
            }
        }
    }
}

pub async fn sync_user_mention_only_push_rules(user_token: &str) {
    if user_token.is_empty() {
        return;
    }
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_static("application/json"));
    if let Ok(hv) = HeaderValue::from_str(&format!("Bearer {user_token}")) {
        headers.insert("Authorization", hv);
    }

    let disable_payload = Bytes::from_static(b"{\"enabled\":false}");
    let _ = call_conduit(
        "/_matrix/client/v3/pushrules/global/underride/.m.rule.message/enabled",
        Method::PUT,
        Some(headers.clone()),
        Some(disable_payload.clone()),
    )
    .await;
    let _ = call_conduit(
        "/_matrix/client/v3/pushrules/global/underride/.m.rule.roommessage/enabled",
        Method::PUT,
        Some(headers.clone()),
        Some(disable_payload),
    )
    .await;

    let enable_payload = Bytes::from_static(b"{\"enabled\":true}");
    let _ = call_conduit(
        "/_matrix/client/v3/pushrules/global/content/.m.rule.contains_user_name/enabled",
        Method::PUT,
        Some(headers.clone()),
        Some(enable_payload.clone()),
    )
    .await;
    let _ = call_conduit(
        "/_matrix/client/v3/pushrules/global/override/.m.rule.contains_display_name/enabled",
        Method::PUT,
        Some(headers),
        Some(enable_payload),
    )
    .await;
}


pub async fn unban_matrix_user_from_room(
    secret: &[u8],
    store: &mitch_lib::data::DataStore,
    data_dir: &std::path::Path,
    room_id: &str,
    target_user_id: &str,
) {
    // 1. Clean up bannedUsers and mutedUsers in room settings
    let settings_file = data_dir.join("matrix_room_settings.json");
    let mut all = store.read_document(&settings_file, json!({}));
    if let Some(rooms_map) = all.as_object_mut() {
        let uname = target_user_id
            .trim_start_matches('@')
            .split(':')
            .next()
            .unwrap_or(target_user_id);
        let uname_clean: String = uname.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
        let target_clean: String = target_user_id.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
        for (r_id, room) in rooms_map {
            if !room_id.is_empty() && room_id != "all" && room_id != "*" && r_id != room_id {
                continue;
            }
            if let Some(b_map) = room.get_mut("bannedUsers").and_then(|v| v.as_object_mut()) {
                let to_remove: Vec<String> = b_map.keys().filter(|k| {
                    let k_local = k.trim_start_matches('@').split(':').next().unwrap_or(k);
                    let k_clean: String = k_local.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
                    let k_raw_clean: String = k.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
                    k.eq_ignore_ascii_case(target_user_id)
                        || k.eq_ignore_ascii_case(uname)
                        || (!uname_clean.is_empty() && (k_clean == uname_clean || k_raw_clean == uname_clean))
                        || (!target_clean.is_empty() && (k_clean == target_clean || k_raw_clean == target_clean))
                }).cloned().collect();
                for k in to_remove {
                    b_map.remove(&k);
                }
            }
            if let Some(m_map) = room.get_mut("mutedUsers").and_then(|v| v.as_object_mut()) {
                let to_remove: Vec<String> = m_map.keys().filter(|k| {
                    let k_local = k.trim_start_matches('@').split(':').next().unwrap_or(k);
                    let k_clean: String = k_local.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
                    let k_raw_clean: String = k.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
                    k.eq_ignore_ascii_case(target_user_id)
                        || k.eq_ignore_ascii_case(uname)
                        || (!uname_clean.is_empty() && (k_clean == uname_clean || k_raw_clean == uname_clean))
                        || (!target_clean.is_empty() && (k_clean == target_clean || k_raw_clean == target_clean))
                }).cloned().collect();
                for k in to_remove {
                    m_map.remove(&k);
                }
            }
        }
        let _ = store.write_document(&settings_file, &all);
    }

    // 2. Call Conduit API to unban and reset power levels if Conduit admin token is available
    let admin_tok = match get_system_admin_matrix_token(secret).await {
        Ok(t) => t,
        Err(_) => return,
    };
    let mut req_headers = HeaderMap::new();
    req_headers.insert("Content-Type", HeaderValue::from_static("application/json"));
    if let Ok(hv) = HeaderValue::from_str(&format!("Bearer {admin_tok}")) {
        req_headers.insert("Authorization", hv);
    }
    let unban_payload = json!({ "user_id": target_user_id });
    let unban_res = call_conduit(
        &format!(
            "/_matrix/client/v3/rooms/{}/unban",
            url::form_urlencoded::byte_serialize(room_id.as_bytes()).collect::<String>()
        ),
        Method::POST,
        Some(req_headers.clone()),
        Some(Bytes::from(
            serde_json::to_vec(&unban_payload).unwrap_or_default(),
        )),
    )
    .await;
    if let Err(ref e) = unban_res {
        eprintln!("[matrix_unban] Direct unban call error for {target_user_id} in {room_id}: {e}");
    }

    let uname = target_user_id
        .trim_start_matches('@')
        .split(':')
        .next()
        .unwrap_or(target_user_id);
    let uname_clean: String = uname.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
    let target_clean: String = target_user_id.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();

    // Also inspect room banned members from Conduit: unban any member whose state_key matches target_user_id
    if let Ok((status, _, bytes)) = call_conduit(
        &format!(
            "/_matrix/client/v3/rooms/{}/members?membership=ban",
            url::form_urlencoded::byte_serialize(room_id.as_bytes()).collect::<String>()
        ),
        Method::GET,
        Some(req_headers.clone()),
        None,
    )
    .await
    {
        if status.is_success() {
            if let Ok(members_data) = serde_json::from_slice::<Value>(&bytes) {
                if let Some(chunk) = members_data.get("chunk").and_then(|v| v.as_array()) {
                    for ev in chunk {
                        let uid = ev.get("state_key").and_then(|v| v.as_str()).unwrap_or("");
                        let membership = ev.get("content").and_then(|c| c.get("membership")).and_then(|m| m.as_str()).unwrap_or("");
                        if membership == "ban" && !uid.is_empty() {
                            let uid_local = uid.trim_start_matches('@').split(':').next().unwrap_or(uid);
                            let uid_clean: String = uid_local.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
                            let uid_raw_clean: String = uid.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
                            if uid.eq_ignore_ascii_case(target_user_id)
                                || uid.eq_ignore_ascii_case(uname)
                                || (!uname_clean.is_empty() && (uid_clean == uname_clean || uid_raw_clean == uname_clean))
                                || (!target_clean.is_empty() && (uid_clean == target_clean || uid_raw_clean == target_clean))
                            {
                                let payload = json!({ "user_id": uid });
                                let _ = call_conduit(
                                    &format!(
                                        "/_matrix/client/v3/rooms/{}/unban",
                                        url::form_urlencoded::byte_serialize(room_id.as_bytes()).collect::<String>()
                                    ),
                                    Method::POST,
                                    Some(req_headers.clone()),
                                    Some(Bytes::from(serde_json::to_vec(&payload).unwrap_or_default())),
                                )
                                .await;
                            }
                        }
                    }
                }
            }
        }
    }

    // Fallback: also inspect /state for any m.room.member events with membership == "ban"
    if let Ok((status, _, bytes)) = call_conduit(
        &format!(
            "/_matrix/client/v3/rooms/{}/state",
            url::form_urlencoded::byte_serialize(room_id.as_bytes()).collect::<String>()
        ),
        Method::GET,
        Some(req_headers.clone()),
        None,
    )
    .await
    {
        if status.is_success() {
            if let Ok(state_events) = serde_json::from_slice::<Value>(&bytes) {
                if let Some(arr) = state_events.as_array() {
                    for ev in arr {
                        if ev.get("type").and_then(|v| v.as_str()) == Some("m.room.member") {
                            let uid = ev.get("state_key").and_then(|v| v.as_str()).unwrap_or("");
                            let membership = ev.get("content").and_then(|c| c.get("membership")).and_then(|m| m.as_str()).unwrap_or("");
                            if membership == "ban" && !uid.is_empty() {
                                let uid_local = uid.trim_start_matches('@').split(':').next().unwrap_or(uid);
                                let uid_clean: String = uid_local.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
                                let uid_raw_clean: String = uid.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
                                if uid.eq_ignore_ascii_case(target_user_id)
                                    || uid.eq_ignore_ascii_case(uname)
                                    || (!uname_clean.is_empty() && (uid_clean == uname_clean || uid_raw_clean == uname_clean))
                                    || (!target_clean.is_empty() && (uid_clean == target_clean || uid_raw_clean == target_clean))
                                {
                                    let payload = json!({ "user_id": uid });
                                    let _ = call_conduit(
                                        &format!(
                                            "/_matrix/client/v3/rooms/{}/unban",
                                            url::form_urlencoded::byte_serialize(room_id.as_bytes()).collect::<String>()
                                        ),
                                        Method::POST,
                                        Some(req_headers.clone()),
                                        Some(Bytes::from(serde_json::to_vec(&payload).unwrap_or_default())),
                                    )
                                    .await;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Reset PL < 0 in Conduit
    if let Ok((status, _, bytes)) = call_conduit(
        &format!(
            "/_matrix/client/v3/rooms/{}/state/m.room.power_levels",
            url::form_urlencoded::byte_serialize(room_id.as_bytes()).collect::<String>()
        ),
        Method::GET,
        Some(req_headers.clone()),
        None,
    )
    .await
    {
        if status.is_success() {
            if let Ok(mut pl_data) = serde_json::from_slice::<Value>(&bytes) {
                if let Some(users) = pl_data.get_mut("users").and_then(|v| v.as_object_mut()) {
                    let to_remove: Vec<String> = users.iter().filter_map(|(u, pl)| {
                        let val = pl.as_i64().unwrap_or(0);
                        let u_local = u.trim_start_matches('@').split(':').next().unwrap_or(u);
                        let u_clean: String = u_local.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
                        let u_raw_clean: String = u.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
                        if val < 0 && (
                            u.eq_ignore_ascii_case(target_user_id)
                                || u.eq_ignore_ascii_case(uname)
                                || (!uname_clean.is_empty() && (u_clean == uname_clean || u_raw_clean == uname_clean))
                                || (!target_clean.is_empty() && (u_clean == target_clean || u_raw_clean == target_clean))
                        ) {
                            Some(u.clone())
                        } else {
                            None
                        }
                    }).collect();
                    if !to_remove.is_empty() {
                        for u in to_remove {
                            users.remove(&u);
                        }
                        let _ = call_conduit(
                            &format!(
                                "/_matrix/client/v3/rooms/{}/state/m.room.power_levels",
                                url::form_urlencoded::byte_serialize(room_id.as_bytes())
                                    .collect::<String>()
                            ),
                            Method::PUT,
                            Some(req_headers),
                            Some(Bytes::from(
                                serde_json::to_vec(&pl_data).unwrap_or_default(),
                            )),
                        )
                        .await;
                    }
                }
            }
        }
    }
}

/// Re-invites a user to a room using the system admin's Matrix token, so the
/// room reappears in their client without them having to search for its
/// alias. Fire-and-forget — invite failures (already joined, no such room,
/// insufficient power) are not actionable here and are silently ignored.
pub async fn invite_matrix_user_to_room(secret: &[u8], room_id: &str, target_user_id: &str) {
    let Ok(admin_tok) = get_system_admin_matrix_token(secret).await else {
        return;
    };
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_static("application/json"));
    if let Ok(hv) = HeaderValue::from_str(&format!("Bearer {admin_tok}")) {
        headers.insert("Authorization", hv);
    }
    let payload = json!({ "user_id": target_user_id });
    let _ = call_conduit(
        &format!(
            "/_matrix/client/v3/rooms/{}/invite",
            url::form_urlencoded::byte_serialize(room_id.as_bytes()).collect::<String>()
        ),
        Method::POST,
        Some(headers),
        Some(Bytes::from(serde_json::to_vec(&payload).unwrap_or_default())),
    )
    .await;
}

pub async fn unban_matrix_user_all_rooms(
    secret: &[u8],
    store: &mitch_lib::data::DataStore,
    data_dir: &std::path::Path,
    target_identifier: &str,
) {
    let resolved = resolve_all_matrix_user_ids_from_store(store, data_dir, secret, target_identifier);
    let targets = if resolved.is_empty() {
        vec![if target_identifier.starts_with('@') {
            target_identifier.to_string()
        } else {
            format!("@{target_identifier}:mitch.pro")
        }]
    } else {
        resolved
    };

    for target_user_id in &targets {
        // Default/official channels: unban, then re-invite so the room
        // reappears in Cinny/Element without the user having to rediscover
        // or search for its alias.
        for (alias, name, topic) in OFFICIAL_ROOMS {
            if let Ok(room_id) = ensure_official_room(secret, alias, name, topic).await {
                unban_matrix_user_from_room(secret, store, data_dir, &room_id, target_user_id).await;
                invite_matrix_user_to_room(secret, &room_id, target_user_id).await;
            }
        }
        let settings_file = data_dir.join("matrix_room_settings.json");
        let all = store.read_document(&settings_file, json!({}));
        if let Some(map) = all.as_object() {
            for (room_id, _) in map {
                unban_matrix_user_from_room(secret, store, data_dir, room_id, target_user_id).await;
            }
        }
    }
}

pub fn check_matrix_slowmode(room_id: &str, sender_key: &str, slowmode_seconds: i64) -> i64 {
    if slowmode_seconds <= 0 {
        return 0;
    }
    let key = format!("{room_id}:{sender_key}");
    let map = room_last_message_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let last = map.get(&key).copied().unwrap_or(0);
    let elapsed = (now_millis() - last) / 1000;
    if elapsed < slowmode_seconds {
        std::cmp::max(1, slowmode_seconds - elapsed)
    } else {
        0
    }
}

pub fn record_matrix_message_sent(room_id: &str, sender_key: &str) {
    if room_id.is_empty() || sender_key.is_empty() {
        return;
    }
    let key = format!("{room_id}:{sender_key}");
    let mut map = room_last_message_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    map.insert(key, now_millis());
    if map.len() > 20000 {
        if let Some(oldest) = map.keys().next().cloned() {
            map.remove(&oldest);
        }
    }
}

pub fn load_matrix_room_settings(state: &AppState, room_id: &str) -> Value {
    let settings_file = state.data_dir().join("matrix_room_settings.json");
    let all = state.store.read_document(&settings_file, json!({}));
    all.get(room_id).cloned().unwrap_or_else(|| {
        json!({
            "slowmodeSeconds": 0,
            "roomMuted": false,
            "mutedUsers": {}
        })
    })
}

pub fn is_user_muted_in_matrix_room(
    state: &AppState,
    room_id: &str,
    user_identifiers: &[String],
) -> Option<Value> {
    let settings_file = state.data_dir().join("matrix_room_settings.json");
    let mut all = state.store.read_document(&settings_file, json!({}));
    let room = all.get(room_id).cloned().unwrap_or(json!({}));
    let muted_users = room.get("mutedUsers").and_then(|v| v.as_object())?;

    let now = now_millis();
    let mut changed = false;

    for raw_id in user_identifiers {
        if raw_id.is_empty() {
            continue;
        }
        let lower = raw_id.to_lowercase().trim().to_string();
        let clean_user = if lower.starts_with('@') {
            lower.clone()
        } else {
            format!("@{lower}:mitch.pro")
        };

        let hit = muted_users
            .get(&clean_user)
            .or_else(|| muted_users.get(&lower));
        if let Some(entry) = hit {
            if let Some(exp) = entry.get("expiresAt").and_then(|v| v.as_i64()) {
                if exp <= now {
                    if let Some(map) = all
                        .get_mut(room_id)
                        .and_then(|r| r.get_mut("mutedUsers"))
                        .and_then(|m| m.as_object_mut())
                    {
                        map.remove(&clean_user);
                        map.remove(&lower);
                        changed = true;
                    }
                    continue;
                }
            }
            if changed {
                let _ = state.store.write_document(&settings_file, &all);
            }
            return Some(entry.clone());
        }
    }

    if changed {
        let _ = state.store.write_document(&settings_file, &all);
    }
    None
}

pub fn urlencoding_decode(val: &str) -> String {
    url::form_urlencoded::parse(format!("v={val}").as_bytes())
        .find(|(k, _)| k == "v")
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| val.to_string())
}

pub fn urlencoding_encode(val: &str) -> String {
    url::form_urlencoded::byte_serialize(val.as_bytes()).collect()
}

pub fn find_profile_email_by_matrix_user_id(
    state: &AppState,
    matrix_user_id: &str,
) -> Option<String> {
    if matrix_user_id.is_empty() {
        return None;
    }
    let clean = if let Some(stripped) = matrix_user_id.strip_prefix('@') {
        stripped.split(':').next().unwrap_or(stripped)
    } else {
        matrix_user_id
    };

    let matrix_users_file = state.data_dir().join("matrix_users.json");
    let matrix_users = state.store.read_document(&matrix_users_file, json!({}));
    if let Some(map) = matrix_users.as_object() {
        for (uid, uname_val) in map {
            if let Some(uname) = uname_val.as_str() {
                if uname.eq_ignore_ascii_case(clean)
                    || format!("@{uname}:mitch.pro").eq_ignore_ascii_case(matrix_user_id)
                {
                    if let Some(email) =
                        mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, uid)
                    {
                        return Some(mitch_lib::auth::normalize_email(&email));
                    }
                }
            }
        }
    }

    let profiles_file = state.data_dir().join("profiles.json");
    let profiles = state.store.read_document(&profiles_file, json!({}));
    if let Some(map) = profiles.as_object() {
        for (norm_email, p) in map {
            if let Some(uname) = p.get("username").and_then(|v| v.as_str()) {
                if uname.eq_ignore_ascii_case(clean) {
                    return Some(norm_email.clone());
                }
            }
        }
    }

    crate::routes::auth::resolve_login_identifier(state, clean)
}

pub async fn sync_profile_to_matrix(
    state: &AppState,
    uid: &str,
    display_name: Option<&str>,
    pfp: Option<&str>,
    bio: Option<&str>,
    provided_token: Option<&str>,
) {
    let matrix_users_file = state.data_dir().join("matrix_users.json");
    let matrix_users = state.store.read_document(&matrix_users_file, json!({}));
    let Some(matrix_username) = matrix_users.get(uid).and_then(|v| v.as_str()) else {
        return; // user has never logged into Matrix
    };

    let mut token = provided_token.map(str::to_string);
    let mut user_id = format!("@{matrix_username}:mitch.pro");

    if token.is_none() {
        let password = get_matrix_password_for_uid(uid, &state.id_secret);
        let mut req_headers = HeaderMap::new();
        req_headers.insert("Content-Type", HeaderValue::from_static("application/json"));
        let login_body = json!({
            "type": "m.login.password",
            "identifier": { "type": "m.id.user", "user": matrix_username },
            "password": password,
            "initial_device_display_name": "Mitch.pro Profile Sync"
        });
        if let Ok((status, _, bytes)) = call_conduit(
            "/_matrix/client/v3/login",
            Method::POST,
            Some(req_headers),
            Some(Bytes::from(
                serde_json::to_vec(&login_body).unwrap_or_default(),
            )),
        )
        .await
        {
            if status.is_success() {
                if let Ok(data) = serde_json::from_slice::<Value>(&bytes) {
                    if let Some(tok) = data.get("access_token").and_then(|v| v.as_str()) {
                        token = Some(tok.to_string());
                    }
                    if let Some(uid_res) = data.get("user_id").and_then(|v| v.as_str()) {
                        user_id = uid_res.to_string();
                    }
                }
            }
        }
    }

    let Some(token) = token else {
        return;
    };

    let encoded_user_id =
        url::form_urlencoded::byte_serialize(user_id.as_bytes()).collect::<String>();
    let mut auth_header = HeaderMap::new();
    if let Ok(val) = HeaderValue::from_str(&format!("Bearer {token}")) {
        auth_header.insert("Authorization", val);
    }
    auth_header.insert("Content-Type", HeaderValue::from_static("application/json"));

    // 1. Sync display name
    if let Some(dn) = display_name {
        let name_val = if dn.is_empty() { matrix_username } else { dn };
        let name_val = &name_val[..name_val.len().min(40)];
        let body = json!({ "displayname": name_val });
        let _ = call_conduit(
            &format!("/_matrix/client/v3/profile/{encoded_user_id}/displayname"),
            Method::PUT,
            Some(auth_header.clone()),
            Some(Bytes::from(serde_json::to_vec(&body).unwrap_or_default())),
        )
        .await;
    }

    // 2. Sync avatar (pfp -> mxc:// URI)
    if let Some(pfp_val) = pfp {
        let raw_pfp = pfp_val.trim();
        if raw_pfp.is_empty() {
            let body = json!({ "avatar_url": Value::Null });
            let _ = call_conduit(
                &format!("/_matrix/client/v3/profile/{encoded_user_id}/avatar_url"),
                Method::PUT,
                Some(auth_header.clone()),
                Some(Bytes::from(serde_json::to_vec(&body).unwrap_or_default())),
            )
            .await;
        } else if raw_pfp.starts_with("mxc://") {
            let body = json!({ "avatar_url": raw_pfp });
            let _ = call_conduit(
                &format!("/_matrix/client/v3/profile/{encoded_user_id}/avatar_url"),
                Method::PUT,
                Some(auth_header.clone()),
                Some(Bytes::from(serde_json::to_vec(&body).unwrap_or_default())),
            )
            .await;
        } else if raw_pfp.contains("/_matrix/media/") {
            static MEDIA_RE: OnceLock<regex::Regex> = OnceLock::new();
            let media_re = MEDIA_RE.get_or_init(|| {
                regex::Regex::new(r"/_matrix/media/(?:v3|r0)/download/([^/]+)/([^/?#]+)")
                    .expect("static regex")
            });
            if let Some(caps) = media_re.captures(raw_pfp) {
                let server = &caps[1];
                let media_id = &caps[2];
                let body = json!({ "avatar_url": format!("mxc://{server}/{media_id}") });
                let _ = call_conduit(
                    &format!("/_matrix/client/v3/profile/{encoded_user_id}/avatar_url"),
                    Method::PUT,
                    Some(auth_header.clone()),
                    Some(Bytes::from(serde_json::to_vec(&body).unwrap_or_default())),
                )
                .await;
            }
        } else {
            let avatar_bytes_opt: Option<(String, Vec<u8>)> = if raw_pfp.starts_with("data:image/") {
                if let Some((meta, data)) = raw_pfp.split_once(',') {
                    let mime = meta
                        .trim_start_matches("data:")
                        .split(';')
                        .next()
                        .unwrap_or("image/png");
                    if meta.contains(";base64") {
                        use base64::Engine;
                        base64::engine::general_purpose::STANDARD
                            .decode(data.trim())
                            .ok()
                            .map(|b| (mime.to_string(), b))
                    } else {
                        Some((mime.to_string(), data.as_bytes().to_vec()))
                    }
                } else {
                    None
                }
            } else if raw_pfp.starts_with('/') {
                let clean = raw_pfp.trim_start_matches('/');
                let p1 = state.store.base_dir.join("webserver").join(clean);
                let p2 = state.data_dir().join(clean);
                let target = if p1.exists() {
                    Some(p1)
                } else if p2.exists() {
                    Some(p2)
                } else {
                    None
                };
                if let Some(path) = target {
                    if let Ok(bytes) = tokio::fs::read(&path).await {
                        let mime = if clean.ends_with(".png") {
                            "image/png"
                        } else if clean.ends_with(".jpg") || clean.ends_with(".jpeg") {
                            "image/jpeg"
                        } else if clean.ends_with(".webp") {
                            "image/webp"
                        } else if clean.ends_with(".gif") {
                            "image/gif"
                        } else if clean.ends_with(".svg") {
                            "image/svg+xml"
                        } else {
                            "image/png"
                        };
                        Some((mime.to_string(), bytes))
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else if raw_pfp.starts_with("http://") || raw_pfp.starts_with("https://") {
                if let Ok(resp) = conduit_client()
                    .get(raw_pfp)
                    .timeout(std::time::Duration::from_secs(5))
                    .send()
                    .await
                {
                    if resp.status().is_success() {
                        let mime = resp
                            .headers()
                            .get("content-type")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("image/png")
                            .to_string();
                        if let Ok(bytes) = resp.bytes().await {
                            Some((mime, bytes.to_vec()))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            if let Some((mime, bytes)) = avatar_bytes_opt {
                let mut upload_headers = HeaderMap::new();
                if let Ok(val) = HeaderValue::from_str(&format!("Bearer {token}")) {
                    upload_headers.insert("Authorization", val);
                }
                if let Ok(val) = HeaderValue::from_str(&mime) {
                    upload_headers.insert("Content-Type", val);
                }
                if let Ok((status, _, upload_resp)) = call_conduit(
                    "/_matrix/media/v3/upload?filename=avatar",
                    Method::POST,
                    Some(upload_headers),
                    Some(Bytes::from(bytes)),
                )
                .await
                {
                    if status.is_success() {
                        if let Ok(resp_json) = serde_json::from_slice::<Value>(&upload_resp) {
                            if let Some(mxc_uri) = resp_json.get("content_uri").and_then(|v| v.as_str()) {
                                let body = json!({ "avatar_url": mxc_uri });
                                let _ = call_conduit(
                                    &format!("/_matrix/client/v3/profile/{encoded_user_id}/avatar_url"),
                                    Method::PUT,
                                    Some(auth_header.clone()),
                                    Some(Bytes::from(serde_json::to_vec(&body).unwrap_or_default())),
                                )
                                .await;
                            }
                        }
                    }
                }
            }
        }
    }

    // 3. Sync bio (as presence status_msg and MSC1769 custom profile fields)
    if let Some(bio_val) = bio {
        let bio_clean = &bio_val.trim()[..bio_val.trim().len().min(300)];
        let status_body = json!({
            "presence": "online",
            "status_msg": bio_clean
        });
        let _ = call_conduit(
            &format!("/_matrix/client/v3/presence/{encoded_user_id}/status"),
            Method::PUT,
            Some(auth_header.clone()),
            Some(Bytes::from(
                serde_json::to_vec(&status_body).unwrap_or_default(),
            )),
        )
        .await;

        let msc_body = json!({ "bio": bio_clean });
        let _ = call_conduit(
            &format!("/_matrix/client/v3/user/{encoded_user_id}/account_data/org.matrix.msc1769.custom_profile_fields"),
            Method::PUT,
            Some(auth_header.clone()),
            Some(Bytes::from(serde_json::to_vec(&msc_body).unwrap_or_default())),
        )
        .await;
    }
}

pub fn is_matrix_staff_member(
    state: &AppState,
    headers: &HeaderMap,
    account: Option<&MatrixAccount>,
) -> bool {
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("");
    if !sid.is_empty()
        && mitch_lib::auth::is_any_admin_id(&state.store, &state.id_secret, sid, false)
    {
        return true;
    }
    if let Some(acc) = account {
        if !acc.uid.is_empty()
            && mitch_lib::auth::is_any_admin_id(&state.store, &state.id_secret, &acc.uid, false)
        {
            return true;
        }
        if !acc.norm_email.is_empty()
            && (mitch_lib::auth::is_admin_email(&state.store, &acc.norm_email)
                || mitch_lib::auth::is_moderator_email(&state.store, &acc.norm_email))
        {
            return true;
        }
    }
    false
}

pub async fn resolve_matrix_account(
    state: &AppState,
    headers: &HeaderMap,
    parsed_body: Option<&Value>,
) -> Option<MatrixAccount> {
    let mut user_candidates = Vec::new();
    if let Some(body) = parsed_body {
        if let Some(auth) = body.get("auth") {
            if let Some(id) = auth.get("identifier") {
                if let Some(u) = id.get("user").and_then(|v| v.as_str()) {
                    user_candidates.push(u.to_string());
                }
                if let Some(a) = id.get("address").and_then(|v| v.as_str()) {
                    user_candidates.push(a.to_string());
                }
            }
            if let Some(u) = auth.get("user").and_then(|v| v.as_str()) {
                user_candidates.push(u.to_string());
            }
        }
        if let Some(id) = body.get("identifier") {
            if let Some(u) = id.get("user").and_then(|v| v.as_str()) {
                user_candidates.push(u.to_string());
            }
            if let Some(a) = id.get("address").and_then(|v| v.as_str()) {
                user_candidates.push(a.to_string());
            }
        }
        if let Some(u) = body.get("user").and_then(|v| v.as_str()) {
            user_candidates.push(u.to_string());
        }
    }

    let matrix_users_file = state.data_dir().join("matrix_users.json");
    let matrix_users = state.store.read_document(&matrix_users_file, json!({}));

    for cand in user_candidates {
        let raw = cand.trim();
        if raw.is_empty() {
            continue;
        }
        let local_part = if raw.starts_with('@') {
            raw.trim_start_matches('@').split(':').next().unwrap_or(raw)
        } else {
            raw
        };

        if let Some(norm) = crate::routes::auth::resolve_login_identifier(state, local_part)
            .or_else(|| crate::routes::auth::resolve_login_identifier(state, raw))
        {
            if let Some(uid) =
                mitch_lib::profile::get_uid_for_email(&state.store, &state.id_secret, &norm)
            {
                let assigned = matrix_users.get(&uid).and_then(|v| v.as_str());
                let user_id = match assigned {
                    Some(a) => format!("@{a}:mitch.pro"),
                    None => format!("@{local_part}:mitch.pro"),
                };
                return Some(MatrixAccount {
                    uid,
                    norm_email: norm,
                    user_id,
                });
            }
        }

        if let Some(obj) = matrix_users.as_object() {
            for (u, name) in obj.iter() {
                let name_str = name.as_str().unwrap_or("");
                if name_str.eq_ignore_ascii_case(local_part)
                    || format!("@{name_str}:mitch.pro").eq_ignore_ascii_case(raw)
                {
                    if let Some(email) =
                        mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, u)
                    {
                        return Some(MatrixAccount {
                            uid: u.clone(),
                            norm_email: mitch_lib::auth::normalize_email(&email),
                            user_id: format!("@{name_str}:mitch.pro"),
                        });
                    }
                }
            }
        }
    }

    // Check Authorization: Bearer <token>
    if let Some(auth_hdr) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if auth_hdr.to_ascii_lowercase().starts_with("bearer ") {
            let tok = auth_hdr[7..].trim();
            let cached = {
                let map = token_to_account_map()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                map.get(tok).cloned()
            };
            if let Some(acc) = cached {
                return Some(acc);
            }

            let mut who_headers = HeaderMap::new();
            if let Ok(hv) = HeaderValue::from_str(&format!("Bearer {tok}")) {
                who_headers.insert("Authorization", hv);
            }
            if let Ok((status, _, bytes)) = call_conduit(
                "/_matrix/client/v3/account/whoami",
                Method::GET,
                Some(who_headers),
                None,
            )
            .await
            {
                if status.is_success() {
                    let who_data: Value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
                    if let Some(matrix_user_id) = who_data.get("user_id").and_then(|v| v.as_str()) {
                        let uname = matrix_user_id
                            .trim_start_matches('@')
                            .split(':')
                            .next()
                            .unwrap_or("");
                        let mut matched_uid = String::new();
                        if let Some(obj) = matrix_users.as_object() {
                            for (u, name) in obj.iter() {
                                let name_str = name.as_str().unwrap_or("");
                                if name_str.eq_ignore_ascii_case(uname)
                                    || format!("@{name_str}:mitch.pro")
                                        .eq_ignore_ascii_case(matrix_user_id)
                                {
                                    matched_uid = u.clone();
                                    break;
                                }
                            }
                        }
                        let mut norm = String::new();
                        if !matched_uid.is_empty() {
                            if let Some(em) = mitch_lib::auth::email_from_sid(
                                &state.store,
                                &state.id_secret,
                                &matched_uid,
                            ) {
                                norm = mitch_lib::auth::normalize_email(&em);
                            }
                        }
                        if norm.is_empty() && !uname.is_empty() {
                            if let Some(res) =
                                crate::routes::auth::resolve_login_identifier(state, uname)
                            {
                                norm = res;
                                if matched_uid.is_empty() {
                                    if let Some(u) = mitch_lib::profile::get_uid_for_email(
                                        &state.store,
                                        &state.id_secret,
                                        &norm,
                                    ) {
                                        matched_uid = u;
                                    }
                                }
                            }
                        }
                        let account = MatrixAccount {
                            uid: matched_uid,
                            norm_email: norm,
                            user_id: matrix_user_id.to_string(),
                        };
                        let mut map = token_to_account_map()
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        map.insert(tok.to_string(), account.clone());
                        if map.len() > 5000 {
                            if let Some(first) = map.keys().next().cloned() {
                                map.remove(&first);
                            }
                        }
                        return Some(account);
                    }
                }
            }
        }
    }

    // Session cookie
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("");
    if !sid.is_empty() && mitch_lib::auth::valid_id(sid, &state.id_secret) {
        if let Some(em) = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, sid) {
            let assigned = matrix_users.get(sid).and_then(|v| v.as_str());
            return Some(MatrixAccount {
                uid: sid.to_string(),
                norm_email: mitch_lib::auth::normalize_email(&em),
                user_id: assigned
                    .map(|a| format!("@{a}:mitch.pro"))
                    .unwrap_or_default(),
            });
        }
    }

    None
}

/// Dynamic Cinny configuration for Mitch.pro.
pub fn handle_cinny_config(headers: &HeaderMap) -> Response {
    let host = request_host(headers);
    let proto = if host.starts_with("localhost") || host.starts_with("127.0.0.1") {
        "http://"
    } else {
        "https://"
    };
    cors_json_response(
        200,
        json!({
            "defaultHomeserver": 0,
            "homeserverList": [host, "mitchdog.com", "mitch.pro"],
            "allowCustomHomeservers": false,
            "default_server_config": {
                "m.homeserver": {
                    "base_url": format!("{proto}{host}"),
                    "server_name": "mitch.pro"
                },
                "org.matrix.msc4143.rtc_foci": [
                    {
                        "type": "livekit",
                        "livekit_service_url": format!("{proto}{host}/livekit")
                    }
                ]
            },
            "disable_custom_urls": true,
            "disable_guests": false,
            "brand": "Mitch.pro Matrix",
            "default_theme": "dark",
            "setting_defaults": {
                "theme": "dark",
                "breadcrumbs": true
            },
            "element_call": {
                "brand": "Element Call",
                "url": "/matrix/public/element-call/",
                "use_exclusively": true
            },
            "elementCall": {
                "url": "/matrix/public/element-call",
                "useInternalInstance": true
            },
            "features": {
                "feature_element_call_video_rooms": true,
                "feature_group_calls": true
            },
            "room_directory": {
                "servers": ["mitch.pro", "mitchdog.com"]
            },
            "featuredCommunities": {
                "openAsDefault": true,
                "servers": ["mitch.pro"],
                "rooms": OFFICIAL_ROOMS
                    .iter()
                    .map(|(alias, _, _)| format!("#{alias}:mitch.pro"))
                    .collect::<Vec<_>>(),
                "spaces": ["#mitch.pro:mitch.pro"]
            },
            "hashRouter": {
                "enabled": false,
                "basename": "/matrix"
            }
        }),
    )
}

/// Dynamic Element Call configuration matching current request host and protocol.
pub fn handle_element_call_config(headers: &HeaderMap) -> Response {
    let host = request_host(headers);
    let proto = if host.starts_with("localhost") || host.starts_with("127.0.0.1") {
        "http://"
    } else {
        "https://"
    };
    cors_json_response(
        200,
        json!({
            "default_server_config": {
                "m.homeserver": {
                    "base_url": format!("{proto}{host}"),
                    "server_name": "mitch.pro"
                },
                "org.matrix.msc4143.rtc_foci": [
                    {
                        "type": "livekit",
                        "livekit_service_url": format!("{proto}{host}/livekit")
                    }
                ]
            },
            "livekit": {
                "livekit_service_url": format!("{proto}{host}/livekit")
            },
            "features": {
                "feature_use_device_session_member_events": true
            },
            "matrix_rtc_session": {
                "wait_for_key_rotation_ms": 5000,
                "delayed_leave_event_restart_ms": 4000,
                "delayed_leave_event_delay_ms": 18000
            }
        }),
    )
}

/// Matrix discovery endpoints (`/.well-known/matrix/client` and `server`).
pub fn handle_well_known(method: &Method, path: &str, headers: &HeaderMap) -> Option<Response> {
    if method == Method::OPTIONS {
        return Some(cors_response(StatusCode::NO_CONTENT, Bytes::new(), None));
    }
    if path == "/.well-known/matrix/client" && method == Method::GET {
        let host = request_host(headers);
        let proto = if host.starts_with("localhost") || host.starts_with("127.0.0.1") {
            "http://"
        } else {
            "https://"
        };
        return Some(cors_json_response(
            200,
            json!({
                "m.homeserver": {
                    "base_url": format!("{proto}{host}")
                },
                "org.matrix.msc4143.rtc_foci": [
                    {
                        "type": "livekit",
                        "livekit_service_url": format!("{proto}{host}/livekit")
                    }
                ]
            }),
        ));
    }
    if path == "/.well-known/matrix/server" && method == Method::GET {
        let mut resp = cors_json_response(200, json!({ "m.server": "mitch.pro:443" }));
        resp.headers_mut().insert(
            "Cache-Control",
            HeaderValue::from_static("public, max-age=300"),
        );
        return Some(resp);
    }
    None
}

/// Matrix API handler (`/api/matrix/*`).
pub async fn handle_api(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    search: &str,
    body_bytes: &[u8],
) -> Option<Response> {
    if method == Method::OPTIONS {
        return Some(cors_response(StatusCode::NO_CONTENT, Bytes::new(), None));
    }

    if path == "/api/matrix/sso-status" && method == Method::GET {
        return Some(api_sso_status(state, headers));
    }
    if path == "/api/matrix/sso-login" && method == Method::POST {
        return Some(api_sso_login(state, headers, body_bytes).await);
    }
    if path == "/api/matrix/notifications/read" && method == Method::POST {
        return Some(api_notifications_read(state, headers, body_bytes));
    }
    if path == "/api/matrix/report-room" && method == Method::POST {
        return Some(api_report_room(state, headers, body_bytes));
    }

    // Moderation endpoints
    if path == "/api/matrix/moderation/overview" && method == Method::GET {
        return Some(mod_overview(state, headers).await);
    }
    if path == "/api/matrix/moderation/set-role" && method == Method::POST {
        return Some(mod_set_role(state, headers, body_bytes).await);
    }
    if path == "/api/matrix/moderation/kick" && method == Method::POST {
        return Some(mod_kick(state, headers, body_bytes).await);
    }
    if path == "/api/matrix/moderation/ban" && method == Method::POST {
        return Some(mod_ban(state, headers, body_bytes).await);
    }
    if path == "/api/matrix/moderation/unban" && method == Method::POST {
        return Some(mod_unban(state, headers, body_bytes).await);
    }
    if path == "/api/matrix/moderation/redact" && method == Method::POST {
        return Some(mod_redact(state, headers, body_bytes).await);
    }
    if (path == "/api/matrix/moderation/slowmode" || path == "/api/matrix/moderation/user-slowmode")
        && method == Method::POST
    {
        return Some(mod_slowmode(state, headers, body_bytes).await);
    }
    if path == "/api/matrix/moderation/mute-user" && method == Method::POST {
        return Some(mod_mute_user(state, headers, body_bytes).await);
    }
    if path == "/api/matrix/moderation/unmute-user" && method == Method::POST {
        return Some(mod_unmute_user(state, headers, body_bytes).await);
    }
    if path == "/api/matrix/moderation/mute-room" && method == Method::POST {
        return Some(mod_mute_room(state, headers, body_bytes).await);
    }
    if path == "/api/matrix/devices/prune-stale" && method == Method::POST {
        return Some(mod_prune_stale(state, headers, body_bytes).await);
    }
    if path == "/api/matrix/gifs/trending" && method == Method::GET {
        return Some(api_gifs_trending(state, headers, search).await);
    }
    if path == "/api/matrix/gifs/search" && method == Method::GET {
        return Some(api_gifs_search(state, headers, search).await);
    }
    if path == "/api/matrix/gifs/proxy" && (method == Method::GET || method == Method::OPTIONS) {
        return Some(api_gifs_proxy(state, headers, search).await);
    }
    if path == "/api/matrix/gifs/send" && method == Method::POST {
        return Some(api_gifs_send(state, headers, body_bytes).await);
    }
    if path == "/api/matrix/gifs/favorites" && method == Method::GET {
        return Some(api_gifs_favorites_get(state, headers).await);
    }
    if path == "/api/matrix/gifs/favorites" && method == Method::POST {
        return Some(api_gifs_favorites_post(state, headers, body_bytes).await);
    }
    if path == "/api/matrix/stickers/packs" && method == Method::GET {
        return Some(api_stickers_packs(state, headers).await);
    }

    let _ = search;
    None
}

fn api_sso_status(state: &AppState, headers: &HeaderMap) -> Response {
    let cookies = crate::routes::me::cookies_of(state, headers);
    let uid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("");
    if uid.is_empty()
        || !mitch_lib::auth::valid_id(uid, &state.id_secret)
        || mitch_lib::auth::banned_info_for_sid(&state.store, &state.id_secret, uid).is_some()
    {
        return cors_json_response(200, json!({ "authenticated": false }));
    }

    let email =
        mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, uid).unwrap_or_default();
    let norm = mitch_lib::auth::normalize_email(&email);
    let profiles_file = state.data_dir().join("profiles.json");
    let profiles = state.store.read_document(&profiles_file, json!({}));
    let prof = profiles.get(&norm).cloned().unwrap_or(json!({}));
    let username = prof
        .get("username")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| {
            if !email.is_empty() {
                email.split('@').next().unwrap_or("user")
            } else {
                "user"
            }
        });

    let matrix_users_file = state.data_dir().join("matrix_users.json");
    let matrix_users = state.store.read_document(&matrix_users_file, json!({}));
    let assigned_username = matrix_users
        .get(uid)
        .and_then(|v| v.as_str())
        .unwrap_or(username);
    let display_name = prof
        .get("displayName")
        .or_else(|| prof.get("nickname"))
        .and_then(|v| v.as_str())
        .unwrap_or(username);
    let raw_pfp = prof.get("pfp").and_then(|v| v.as_str()).unwrap_or("");
    let pfp_val = if let Some(stripped) = raw_pfp.strip_prefix("mxc://") {
        format!("/_matrix/media/v3/download/{stripped}")
    } else {
        raw_pfp.to_string()
    };

    cors_json_response(
        200,
        json!({
            "authenticated": true,
            "username": username,
            "user_id": format!("@{assigned_username}:mitch.pro"),
            "displayName": display_name,
            "pfp": pfp_val,
            "bio": prof.get("bio").and_then(|v| v.as_str()).unwrap_or(""),
        }),
    )
}

async fn api_sso_login(state: &AppState, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let cookies = crate::routes::me::cookies_of(state, headers);
    let uid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("");
    if uid.is_empty() || !mitch_lib::auth::valid_id(uid, &state.id_secret) {
        return cors_json_response(
            401,
            json!({ "ok": false, "error": "Not authenticated on Mitch.pro" }),
        );
    }
    if mitch_lib::auth::banned_info_for_sid(&state.store, &state.id_secret, uid).is_some() {
        return cors_json_response(
            403,
            json!({ "ok": false, "error": "Account is banned", "banned": true }),
        );
    }

    let email =
        mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, uid).unwrap_or_default();
    let norm = mitch_lib::auth::normalize_email(&email);
    let profiles_file = state.data_dir().join("profiles.json");
    let profiles = state.store.read_document(&profiles_file, json!({}));
    let prof = profiles.get(&norm).cloned().unwrap_or(json!({}));
    let username = prof
        .get("username")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| {
            if !email.is_empty() {
                email.split('@').next().unwrap_or("user")
            } else {
                "user"
            }
        });
    let display_name = prof
        .get("displayName")
        .or_else(|| prof.get("nickname"))
        .and_then(|v| v.as_str())
        .unwrap_or(username);

    let body_json: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
    let requested_device_id = sanitize_matrix_device_id(
        body_json
            .get("device_id")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    );

    let matrix_users_file = state.data_dir().join("matrix_users.json");
    let mut matrix_users = state.store.read_document(&matrix_users_file, json!({}));
    let assigned_user = matrix_users
        .get(uid)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let u = mitch_lib::profile::normalize_username(username);
            if u.len() < 2 {
                format!("user_{}", &uid[..std::cmp::min(6, uid.len())])
            } else {
                u
            }
        });

    let password = get_matrix_password_for_uid(uid, &state.id_secret);

    // 1. Login attempt
    let mut login_payload = json!({
        "type": "m.login.password",
        "identifier": { "type": "m.id.user", "user": assigned_user },
        "password": password,
        "initial_device_display_name": "Mitch.pro Web"
    });
    if !requested_device_id.is_empty() {
        login_payload["device_id"] = json!(requested_device_id);
    }

    let mut req_headers = HeaderMap::new();
    req_headers.insert("Content-Type", HeaderValue::from_static("application/json"));

    let login_res = call_conduit(
        "/_matrix/client/v3/login",
        Method::POST,
        Some(req_headers.clone()),
        Some(Bytes::from(
            serde_json::to_vec(&login_payload).unwrap_or_default(),
        )),
    )
    .await;

    let mut auth_result: Option<Value> = match login_res {
        Ok((status, _, bytes)) if status.is_success() => serde_json::from_slice(&bytes).ok(),
        _ => None,
    };

    let mut final_user = assigned_user.clone();

    // 2. Register attempt if login fails
    if auth_result.is_none() {
        let mut candidate_name = assigned_user.clone();
        for attempt in 0..5 {
            let mut reg_payload = json!({
                "username": candidate_name,
                "password": password,
                "auth": { "type": "m.login.dummy" }
            });
            if !requested_device_id.is_empty() {
                reg_payload["device_id"] = json!(requested_device_id);
            }
            let reg_res = call_conduit(
                "/_matrix/client/v3/register",
                Method::POST,
                Some(req_headers.clone()),
                Some(Bytes::from(
                    serde_json::to_vec(&reg_payload).unwrap_or_default(),
                )),
            )
            .await;

            if let Ok((status, _, bytes)) = reg_res {
                let data: Value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
                if status.is_success() && data.get("access_token").is_some() {
                    auth_result = Some(data);
                    final_user = candidate_name;
                    break;
                } else if data.get("errcode").and_then(|v| v.as_str()) == Some("M_USER_IN_USE") {
                    candidate_name = format!("{assigned_user}-{}", attempt + 2);
                } else {
                    break;
                }
            }
        }
    }

    let Some(auth_data) = auth_result else {
        return cors_json_response(
            500,
            json!({ "ok": false, "error": "Matrix SSO authentication failed" }),
        );
    };

    if matrix_users.get(uid).and_then(|v| v.as_str()) != Some(&final_user) {
        if let Some(map) = matrix_users.as_object_mut() {
            map.insert(uid.to_string(), json!(final_user));
            let _ = state
                .store
                .write_document(&matrix_users_file, &matrix_users);
        }
    }

    let user_id = auth_data
        .get("user_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let access_token = auth_data
        .get("access_token")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let device_id = auth_data
        .get("device_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if !access_token.is_empty() {
        let mut map = token_to_account_map()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        map.insert(
            access_token.to_string(),
            MatrixAccount {
                uid: uid.to_string(),
                norm_email: norm.clone(),
                user_id: user_id.to_string(),
            },
        );
    }

    let target_power_level = get_matrix_power_level_for_sid(state, uid);
    let role = if target_power_level >= 100 {
        "admin"
    } else if target_power_level >= 50 {
        "moderator"
    } else {
        "member"
    };

    sync_matrix_user_to_official_rooms(&state.id_secret, user_id, access_token, target_power_level)
        .await;

    let push_tok = access_token.to_string();
    tokio::spawn(async move {
        sync_user_mention_only_push_rules(&push_tok).await;
    });

    let raw_pfp = prof.get("pfp").and_then(|v| v.as_str()).unwrap_or("");
    let bio_val = prof.get("bio").and_then(|v| v.as_str()).unwrap_or("");

    sync_profile_to_matrix(
        state,
        uid,
        Some(display_name),
        Some(raw_pfp),
        Some(bio_val),
        Some(access_token),
    )
    .await;

    let pfp_val = if let Some(stripped) = raw_pfp.strip_prefix("mxc://") {
        format!("/_matrix/media/v3/download/{stripped}")
    } else {
        raw_pfp.to_string()
    };

    cors_json_response(
        200,
        json!({
            "ok": true,
            "user_id": user_id,
            "access_token": access_token,
            "device_id": device_id,
            "home_server": "mitch.pro",
            "base_url": "https://mitchdog.com",
            "username": final_user,
            "displayName": display_name,
            "pfp": pfp_val,
            "bio": bio_val,
            "role": role,
            "powerLevel": target_power_level,
            "officialRoom": "#general:mitch.pro",
            "officialRooms": OFFICIAL_ROOMS.iter().map(|(alias, _, _)| format!("#{alias}:mitch.pro")).collect::<Vec<_>>()
        }),
    )
}

fn api_notifications_read(state: &AppState, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("");
    let mut norm = String::new();
    if !sid.is_empty() && mitch_lib::auth::valid_id(sid, &state.id_secret) {
        if let Some(em) = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, sid) {
            norm = mitch_lib::auth::normalize_email(&em);
        }
    }
    if norm.is_empty() {
        if let Some(auth_hdr) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
            let tok = auth_hdr.trim_start_matches("Bearer ").trim();
            let map = token_to_account_map()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(acc) = map.get(tok) {
                norm = acc.norm_email.clone();
            }
        }
    }
    if norm.is_empty() {
        return cors_json_response(401, json!({ "error": "unauthorized" }));
    }

    let body_json: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
    let room_id = body_json
        .get("roomId")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let notif_file = state.data_dir().join("matrix_notifications.json");
    let mut all = state.store.read_document(&notif_file, json!({}));
    let mut changed = false;

    if let Some(list) = all.get_mut(&norm).and_then(|v| v.as_array_mut()) {
        for n in list.iter_mut() {
            let n_room = n.get("roomId").and_then(|v| v.as_str()).unwrap_or("");
            if (room_id.is_empty() || n_room == room_id) && n.get("read") != Some(&json!(true)) {
                changed = true;
                n["read"] = json!(true);
            }
        }
    }

    if changed {
        let _ = state.store.write_document(&notif_file, &all);
        crate::ws::trigger_notification_refresh(state);
    }

    cors_json_response(200, json!({ "ok": true }))
}

fn api_report_room(state: &AppState, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let body_json: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
    let room_id = body_json
        .get("roomId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if room_id.is_empty() {
        return cors_json_response(400, json!({ "error": "Room ID required" }));
    }

    let reason = body_json
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("Reported chat without entering");
    let room_name = body_json
        .get("roomName")
        .and_then(|v| v.as_str())
        .unwrap_or(room_id);

    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("");
    let mut reporter = if !sid.is_empty() {
        mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, sid)
            .unwrap_or_else(|| sid.to_string())
    } else {
        String::new()
    };

    if reporter.is_empty() {
        if let Some(auth_hdr) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
            let tok = auth_hdr.trim_start_matches("Bearer ").trim();
            let map = token_to_account_map()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(acc) = map.get(tok) {
                reporter = acc.norm_email.clone();
            }
        }
    }
    if reporter.is_empty() {
        reporter = body_json
            .get("reporter")
            .and_then(|v| v.as_str())
            .unwrap_or("matrix-user")
            .to_string();
    }

    let clean_id = format!("room-{}-{}", now_millis(), rand::random::<u16>());
    let report_entry = json!({
        "id": clean_id,
        "reason": format!("[Matrix Room {room_name} ({room_id})] {reason}"),
        "reportedBy": reporter,
        "ts": now_millis(),
        "status": "Needs review",
        "matrixRoomId": room_id,
        "matrixRoomName": room_name,
        "reportedWithoutEntering": true,
        "context": [
            {
                "from": "system",
                "to": room_id,
                "text": format!("Chat reported without opening: {reason} (Room: {room_name})"),
                "ts": now_millis(),
                "reported": true
            }
        ]
    });

    let reports_file = state.data_dir().join("chat_reports.json");
    let mut reports = state.store.read_document(&reports_file, json!([]));
    if let Some(arr) = reports.as_array_mut() {
        arr.push(report_entry);
        if arr.len() > 5000 {
            let excess = arr.len() - 5000;
            arr.drain(0..excess);
        }
        let _ = state.store.write_document(&reports_file, &reports);
    }

    cors_json_response(
        200,
        json!({ "success": true, "message": "Chat reported successfully" }),
    )
}

fn extract_matrix_query_param(query: &str, key: &str) -> Option<String> {
    form_urlencoded::parse(query.trim_start_matches('?').as_bytes())
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

const GIF_PAGE_SIZE: i64 = 25;

/// Giphy trending/search, one page.
async fn fetch_giphy_gifs(query: Option<&str>, offset: i64) -> Vec<Value> {
    let mut giphy_key = std::env::var("GIPHY_API_KEY").unwrap_or_default().trim().to_string();
    if giphy_key.is_empty() {
        giphy_key = "sXpGFDGZs0Dv1mmNFvYaGUvYwKX0PWIh".to_string();
    }
    let url = match query {
        Some(q) => {
            let enc_q = form_urlencoded::byte_serialize(q.as_bytes()).collect::<String>();
            format!(
                "https://api.giphy.com/v1/gifs/search?api_key={giphy_key}&q={enc_q}&limit={GIF_PAGE_SIZE}&offset={offset}&rating=g"
            )
        }
        None => format!(
            "https://api.giphy.com/v1/gifs/trending?api_key={giphy_key}&limit={GIF_PAGE_SIZE}&offset={offset}&rating=g"
        ),
    };
    let Ok(client) = reqwest::Client::builder().timeout(std::time::Duration::from_secs(6)).build() else {
        return Vec::new();
    };
    let Ok(resp) = client.get(&url).send().await else {
        return Vec::new();
    };
    if !resp.status().is_success() {
        return Vec::new();
    }
    let Ok(data) = resp.json::<Value>().await else {
        return Vec::new();
    };
    let Some(results) = data.get("data").and_then(|r| r.as_array()) else {
        return Vec::new();
    };
    results
        .iter()
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?;
            let title = item.get("title").and_then(|c| c.as_str()).unwrap_or("");
            let images = item.get("images")?.as_object()?;
            let orig = images.get("original")?.as_object()?;
            let gif_url = orig.get("url")?.as_str()?;
            let preview = images
                .get("fixed_width_small")
                .or_else(|| images.get("fixed_width"))
                .or_else(|| images.get("downsized"))
                .and_then(|tg| tg.get("url"))
                .and_then(|u| u.as_str())
                .unwrap_or(gif_url);
            Some(json!({
                "id": id,
                "title": title,
                "url": gif_url,
                "preview": preview,
                "width": orig.get("width").and_then(|w| w.as_str()).and_then(|s| s.parse::<i64>().ok()).unwrap_or(320),
                "height": orig.get("height").and_then(|h| h.as_str()).and_then(|s| s.parse::<i64>().ok()).unwrap_or(240),
            }))
        })
        .collect()
}

/// Fetches one Giphy page and shapes it for the client's infinite scroll:
/// `giphyOffset` is the offset to request next, `hasMore` tells the client
/// whether to bother. Falls back to the small curated list only on a first
/// page that comes back empty (key missing/rate-limited/offline) — a later
/// empty page just means "no more," not a fallback trigger.
async fn fetch_gif_page(query: Option<&str>, giphy_offset: i64) -> Response {
    let items = fetch_giphy_gifs(query, giphy_offset).await;

    if items.is_empty() && giphy_offset == 0 {
        return cors_json_response(
            200,
            json!({
                "results": curated_gifs(query),
                "giphyOffset": 0,
                "hasMore": false,
            }),
        );
    }

    let count = items.len() as i64;
    cors_json_response(
        200,
        json!({
            "results": items,
            "giphyOffset": giphy_offset + count,
            "hasMore": count >= GIF_PAGE_SIZE,
        }),
    )
}

async fn api_gifs_trending(_state: &AppState, _headers: &HeaderMap, search: &str) -> Response {
    let giphy_offset = extract_matrix_query_param(search, "giphyOffset")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);
    fetch_gif_page(None, giphy_offset).await
}

async fn api_gifs_search(_state: &AppState, _headers: &HeaderMap, search: &str) -> Response {
    let q = extract_matrix_query_param(search, "q").unwrap_or_default();
    if q.is_empty() {
        return api_gifs_trending(_state, _headers, search).await;
    }
    let giphy_offset = extract_matrix_query_param(search, "giphyOffset")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);
    fetch_gif_page(Some(&q), giphy_offset).await
}

async fn api_gifs_proxy(_state: &AppState, _headers: &HeaderMap, search: &str) -> Response {
    let url_str = extract_matrix_query_param(search, "url").unwrap_or_default();
    if url_str.is_empty() {
        return cors_json_response(400, json!({ "error": "url parameter is required" }));
    }
    let Ok(parsed_url) = url::Url::parse(&url_str) else {
        return cors_json_response(400, json!({ "error": "invalid url" }));
    };
    if parsed_url.scheme() != "http" && parsed_url.scheme() != "https" {
        return cors_json_response(400, json!({ "error": "invalid scheme" }));
    }
    let client = match reqwest::Client::builder().timeout(std::time::Duration::from_secs(10)).build() {
        Ok(c) => c,
        Err(_) => return cors_json_response(500, json!({ "error": "client build failed" })),
    };
    // Tenor/Giphy's CDNs hotlink-block or throttle requests that don't look
    // like a browser; reqwest's default UA ("reqwest/x.y.z") gets a 403/429
    // from them often enough that GIF sends silently failed.
    let resp = match client
        .get(parsed_url.as_str())
        .header(reqwest::header::USER_AGENT, GIF_FETCH_USER_AGENT)
        .header(reqwest::header::ACCEPT, "image/*,*/*;q=0.8")
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return cors_json_response(502, json!({ "error": "failed to fetch media" })),
    };
    if !resp.status().is_success() {
        return cors_json_response(resp.status().as_u16(), json!({ "error": "upstream error" }));
    }
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/gif")
        .to_string();
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(_) => return cors_json_response(502, json!({ "error": "failed to read media body" })),
    };
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(hyper::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(hyper::header::ACCESS_CONTROL_ALLOW_METHODS, "GET, OPTIONS")
        .header(hyper::header::CACHE_CONTROL, "public, max-age=86400");
    if let Ok(ct) = HeaderValue::from_str(&content_type) {
        builder = builder.header(hyper::header::CONTENT_TYPE, ct);
    }
    builder.body(axum::body::Body::from(bytes)).unwrap_or_else(|_| cors_json_response(500, json!({ "error": "response builder failed" })))
}

async fn api_gifs_send(_state: &AppState, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let token = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .unwrap_or("");
    if token.is_empty() {
        return cors_json_response(401, json!({ "error": "Authorization Bearer token required" }));
    }

    let payload: Value = match serde_json::from_slice(body_bytes) {
        Ok(v) => v,
        Err(_) => return cors_json_response(400, json!({ "error": "Invalid JSON body" })),
    };

    let room_id = payload.get("roomId").and_then(|v| v.as_str()).unwrap_or("").trim();
    let gif_url = payload.get("url").and_then(|v| v.as_str()).unwrap_or("").trim();
    let title = payload.get("title").and_then(|v| v.as_str()).unwrap_or("GIF");

    if room_id.is_empty() || gif_url.is_empty() {
        return cors_json_response(400, json!({ "error": "roomId and url are required" }));
    }

    let encoded_room = form_urlencoded::byte_serialize(room_id.as_bytes()).collect::<String>();

    // 1. Check if room is encrypted
    let mut auth_headers = HeaderMap::new();
    if let Ok(hv) = HeaderValue::from_str(&format!("Bearer {token}")) {
        auth_headers.insert("Authorization", hv);
    }
    if let Ok((status, _, _)) = call_conduit(
        &format!("/_matrix/client/v3/rooms/{encoded_room}/state/m.room.encryption"),
        Method::GET,
        Some(auth_headers.clone()),
        None,
    ).await {
        if status.is_success() {
            // Room is encrypted, tell client to use local crypto staging
            return cors_json_response(200, json!({ "encrypted": true }));
        }
    }

    // 2. Fetch the GIF bytes from upstream (Tenor/Giphy)
    let Ok(parsed_url) = url::Url::parse(gif_url) else {
        return cors_json_response(400, json!({ "error": "Invalid gif url" }));
    };
    if parsed_url.scheme() != "http" && parsed_url.scheme() != "https" {
        return cors_json_response(400, json!({ "error": "Invalid scheme" }));
    }

    let Ok(client) = reqwest::Client::builder().timeout(std::time::Duration::from_secs(12)).build() else {
        return cors_json_response(500, json!({ "error": "Client build failed" }));
    };

    // Same hotlink-protection issue as api_gifs_proxy below — without a
    // browser-like User-Agent, Tenor/Giphy intermittently 403/429 this
    // fetch, which made the "instant" send path fail and silently fall
    // back to the client-side paste flow (or just show as a failed send).
    let Ok(resp) = client
        .get(parsed_url.as_str())
        .header(reqwest::header::USER_AGENT, GIF_FETCH_USER_AGENT)
        .header(reqwest::header::ACCEPT, "image/*,*/*;q=0.8")
        .send()
        .await
    else {
        return cors_json_response(502, json!({ "error": "Failed to fetch gif" }));
    };

    if !resp.status().is_success() {
        return cors_json_response(502, json!({ "error": "Upstream gif provider returned error" }));
    }

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/gif")
        .to_string();

    let Ok(bytes) = resp.bytes().await else {
        return cors_json_response(502, json!({ "error": "Failed to read gif body" }));
    };

    // 3. Upload to Conduit on localhost
    let ext = if content_type.contains("png") {
        "png"
    } else if content_type.contains("webp") {
        "webp"
    } else {
        "gif"
    };
    let filename = format!("{}.{ext}", title.chars().filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-').collect::<String>());

    let mut upload_headers = auth_headers.clone();
    if let Ok(ct) = HeaderValue::from_str(&content_type) {
        upload_headers.insert("Content-Type", ct);
    }

    let upload_res = call_conduit(
        &format!("/_matrix/media/v3/upload?filename={filename}"),
        Method::POST,
        Some(upload_headers),
        Some(bytes.clone()),
    ).await;

    let content_uri = match upload_res {
        Ok((status, _, upload_bytes)) if status.is_success() => {
            let data: Value = serde_json::from_slice(&upload_bytes).unwrap_or(json!({}));
            let uri = data.get("content_uri").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if uri.is_empty() {
                return cors_json_response(502, json!({ "error": "No content_uri returned from media upload" }));
            }
            uri
        }
        _ => return cors_json_response(502, json!({ "error": "Media upload failed" })),
    };

    // 4. Send m.room.message
    let txn_id = format!("mitch_gif_{}", now_millis());
    let mut send_headers = auth_headers;
    send_headers.insert("Content-Type", HeaderValue::from_static("application/json"));

    let send_payload = json!({
        "msgtype": "m.image",
        "body": title,
        "url": content_uri,
        "info": {
            "mimetype": content_type,
            "size": bytes.len()
        }
    });

    let send_res = call_conduit(
        &format!("/_matrix/client/v3/rooms/{encoded_room}/send/m.room.message/{txn_id}"),
        Method::PUT,
        Some(send_headers),
        Some(Bytes::from(serde_json::to_vec(&send_payload).unwrap_or_default())),
    ).await;

    match send_res {
        Ok((status, _, send_bytes)) if status.is_success() => {
            let data: Value = serde_json::from_slice(&send_bytes).unwrap_or(json!({}));
            cors_json_response(200, json!({ "ok": true, "eventId": data.get("event_id") }))
        }
        Ok((status, _, send_bytes)) => {
            let data: Value = serde_json::from_slice(&send_bytes).unwrap_or(json!({}));
            cors_json_response(status.as_u16(), data)
        }
        Err(e) => cors_json_response(502, json!({ "error": "Send failed", "details": e })),
    }
}

const GIF_FAVORITES_FILE: &str = "gif_favorites.json";
const MAX_GIF_FAVORITES: usize = 200;

/// Applies one add/remove action to a user's favorites list in place.
/// Dedupes by id first (so re-adding an already-favorited GIF moves it to
/// the front instead of duplicating it), caps the list at
/// `MAX_GIF_FAVORITES`, newest first. Pure and synchronous — no network or
/// store access — so it's unit-testable without a running Conduit.
fn apply_gif_favorite_action(
    list: &mut Vec<Value>,
    gif_id: &str,
    action: &str,
    gif: Option<&Value>,
) -> Result<(), &'static str> {
    list.retain(|g| g.get("id").and_then(|v| v.as_str()) != Some(gif_id));
    if action == "add" {
        let g = gif.ok_or("gif object is required to add a favorite")?;
        let url = g.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if url.is_empty() {
            return Err("gif.url is required to favorite a GIF");
        }
        list.insert(
            0,
            json!({
                "id": gif_id,
                "title": g.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                "url": url,
                "preview": g.get("preview").and_then(|v| v.as_str()).unwrap_or(""),
                "width": g.get("width").and_then(Value::as_i64).unwrap_or(320),
                "height": g.get("height").and_then(Value::as_i64).unwrap_or(240),
            }),
        );
        list.truncate(MAX_GIF_FAVORITES);
    }
    Ok(())
}

/// `GET /api/matrix/gifs/favorites` — the signed-in user's saved GIFs, in
/// the same `{id,title,url,preview,width,height}` shape trending/search
/// return, so the client renders them with the exact same `renderGifs()`.
async fn api_gifs_favorites_get(state: &AppState, headers: &HeaderMap) -> Response {
    let Some(account) = resolve_matrix_account(state, headers, None).await else {
        return cors_json_response(401, json!({ "error": "Sign in required" }));
    };
    let all = state
        .store
        .read_document(&state.data_dir().join(GIF_FAVORITES_FILE), json!({}));
    let results = all.get(&account.norm_email).cloned().unwrap_or(json!([]));
    cors_json_response(200, json!({ "results": results }))
}

/// `POST /api/matrix/gifs/favorites` — `{action: "add"|"remove", gif: {...}}`.
/// Favorites are keyed by the account's normalized email (same identity key
/// used everywhere else in this codebase), capped at `MAX_GIF_FAVORITES` per
/// user, newest first.
async fn api_gifs_favorites_post(
    state: &AppState,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Response {
    let Some(account) = resolve_matrix_account(state, headers, None).await else {
        return cors_json_response(401, json!({ "error": "Sign in required" }));
    };
    let payload: Value = match serde_json::from_slice(body_bytes) {
        Ok(v) => v,
        Err(_) => return cors_json_response(400, json!({ "error": "Invalid JSON body" })),
    };
    let action = payload.get("action").and_then(|v| v.as_str()).unwrap_or("");
    let gif = payload.get("gif");
    let gif_id = gif
        .and_then(|g| g.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if gif_id.is_empty() || (action != "add" && action != "remove") {
        return cors_json_response(
            400,
            json!({ "error": "gif.id and a valid action ('add' or 'remove') are required" }),
        );
    }

    let file = state.data_dir().join(GIF_FAVORITES_FILE);
    let mut all = state.store.read_document(&file, json!({}));
    let mut list: Vec<Value> = all
        .get(&account.norm_email)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if let Err(msg) = apply_gif_favorite_action(&mut list, &gif_id, action, gif) {
        return cors_json_response(400, json!({ "error": msg }));
    }

    if let Some(obj) = all.as_object_mut() {
        obj.insert(account.norm_email.clone(), json!(list));
    } else {
        all = json!({ account.norm_email.clone(): list });
    }
    let _ = state.store.write_document(&file, &all);

    cors_json_response(200, json!({ "ok": true, "results": list }))
}

async fn api_stickers_packs(_state: &AppState, _headers: &HeaderMap) -> Response {
    cors_json_response(200, json!({
        "packs": curated_sticker_packs()
    }))
}

fn curated_gifs(query: Option<&str>) -> Vec<Value> {
    let all = vec![
        json!({
            "id": "cat-vibe",
            "title": "Cat Vibing",
            "url": "https://media.tenor.com/7wA2VwZt6xIAAAAC/cat-vibe.gif",
            "preview": "https://media.tenor.com/7wA2VwZt6xIAAAAC/cat-vibe.gif",
            "tags": "cat vibe dancing groove music lol",
            "width": 320,
            "height": 240
        }),
        json!({
            "id": "pop-cat",
            "title": "Pop Cat",
            "url": "https://media.tenor.com/2lF8i8l1g3QAAAAC/pop-cat.gif",
            "preview": "https://media.tenor.com/2lF8i8l1g3QAAAAC/pop-cat.gif",
            "tags": "pop cat mouth meme sound funny",
            "width": 320,
            "height": 240
        }),
        json!({
            "id": "this-is-fine",
            "title": "This Is Fine",
            "url": "https://media.tenor.com/E13yZ29v8-MAAAAC/this-is-fine-dog.gif",
            "preview": "https://media.tenor.com/E13yZ29v8-MAAAAC/this-is-fine-dog.gif",
            "tags": "this is fine dog fire burning ok chill calm",
            "width": 320,
            "height": 200
        }),
        json!({
            "id": "bongo-cat",
            "title": "Bongo Cat",
            "url": "https://media.tenor.com/pM4bVpE0nHYAAAAC/bongo-cat.gif",
            "preview": "https://media.tenor.com/pM4bVpE0nHYAAAAC/bongo-cat.gif",
            "tags": "bongo cat cute drum music play",
            "width": 320,
            "height": 240
        }),
        json!({
            "id": "gigachad",
            "title": "Gigachad",
            "url": "https://media.tenor.com/4Nn90E_r3iYAAAAC/gigachad-chad.gif",
            "preview": "https://media.tenor.com/4Nn90E_r3iYAAAAC/gigachad-chad.gif",
            "tags": "gigachad chad sigma based muscle handsome smile",
            "width": 320,
            "height": 320
        }),
        json!({
            "id": "leo-cheers",
            "title": "Leonardo Cheers",
            "url": "https://media.tenor.com/6cE975J15sYAAAAC/leonardo-dicaprio-cheers.gif",
            "preview": "https://media.tenor.com/6cE975J15sYAAAAC/leonardo-dicaprio-cheers.gif",
            "tags": "cheers toast celebrate gatsby wine glass leonardo",
            "width": 320,
            "height": 180
        }),
        json!({
            "id": "confused-travolta",
            "title": "Confused Travolta",
            "url": "https://media.tenor.com/gK9p9sA633QAAAAC/confused-travolta.gif",
            "preview": "https://media.tenor.com/gK9p9sA633QAAAAC/confused-travolta.gif",
            "tags": "confused where what lost john travolta pulp fiction",
            "width": 320,
            "height": 180
        }),
        json!({
            "id": "kermit-excited",
            "title": "Kermit Excited",
            "url": "https://media.tenor.com/2s4f-tV26yIAAAAC/kermit-excited.gif",
            "preview": "https://media.tenor.com/2s4f-tV26yIAAAAC/kermit-excited.gif",
            "tags": "kermit yay excited scream happy hands frog",
            "width": 320,
            "height": 240
        }),
        json!({
            "id": "mind-blown",
            "title": "Mind Blown",
            "url": "https://media.tenor.com/15Gf1f2h3YIAAAAC/mind-blown-explosion.gif",
            "preview": "https://media.tenor.com/15Gf1f2h3YIAAAAC/mind-blown-explosion.gif",
            "tags": "mind blown explosion galaxy wow shock insane",
            "width": 320,
            "height": 240
        }),
        json!({
            "id": "frog-dance",
            "title": "Dancing Frog",
            "url": "https://media.tenor.com/hG9A9U00vYIAAAAC/frog-dance.gif",
            "preview": "https://media.tenor.com/hG9A9U00vYIAAAAC/frog-dance.gif",
            "tags": "dance frog groove rhythm vibe",
            "width": 320,
            "height": 240
        }),
        json!({
            "id": "lmao-laughing",
            "title": "Laughing LMAO",
            "url": "https://media.tenor.com/41M5sNlB1GIAAAAC/lmao-laughing.gif",
            "preview": "https://media.tenor.com/41M5sNlB1GIAAAAC/lmao-laughing.gif",
            "tags": "laugh lmao lol haha funny crying laugh rolling",
            "width": 320,
            "height": 240
        }),
        json!({
            "id": "facepalm",
            "title": "Facepalm Picard",
            "url": "https://media.tenor.com/u8w2vA515jAAAAAC/facepalm-picard.gif",
            "preview": "https://media.tenor.com/u8w2vA515jAAAAAC/facepalm-picard.gif",
            "tags": "facepalm fail star trek picard smh sigh",
            "width": 320,
            "height": 240
        }),
        json!({
            "id": "spiderman-pointing",
            "title": "Spider-Man Pointing",
            "url": "https://media.tenor.com/qLqC_bXF7aUAAAAC/spiderman-pointing.gif",
            "preview": "https://media.tenor.com/qLqC_bXF7aUAAAAC/spiderman-pointing.gif",
            "tags": "spiderman pointing duplicate copy same you identical",
            "width": 320,
            "height": 240
        }),
        json!({
            "id": "thumbs-up-seal",
            "title": "Seal Thumbs Up",
            "url": "https://media.tenor.com/K33d2k2LgJIAAAAC/thumbs-up-seal.gif",
            "preview": "https://media.tenor.com/K33d2k2LgJIAAAAC/thumbs-up-seal.gif",
            "tags": "seal thumbs up approve yes good great ok nice",
            "width": 320,
            "height": 240
        }),
        json!({
            "id": "anya-heh",
            "title": "Anya Smug Heh",
            "url": "https://media.tenor.com/y2bC7_j231MAAAAC/anya-spy-x-family.gif",
            "preview": "https://media.tenor.com/y2bC7_j231MAAAAC/anya-spy-x-family.gif",
            "tags": "anya heh smug anime spy x family grin cute",
            "width": 320,
            "height": 240
        }),
        json!({
            "id": "doge-nod",
            "title": "Doge Nod",
            "url": "https://media.tenor.com/86oY1c5e-0UAAAAC/doge-nod.gif",
            "preview": "https://media.tenor.com/86oY1c5e-0UAAAAC/doge-nod.gif",
            "tags": "doge nod yes agree dog shiba approve",
            "width": 320,
            "height": 240
        }),
        json!({
            "id": "capybara-bath",
            "title": "Capybara Chill",
            "url": "https://media.tenor.com/B70uF848w4wAAAAC/capybara-bath.gif",
            "preview": "https://media.tenor.com/B70uF848w4wAAAAC/capybara-bath.gif",
            "tags": "capybara chill bath relax orange calm peaceful zen",
            "width": 320,
            "height": 240
        }),
        json!({
            "id": "rickroll",
            "title": "Rickroll Dance",
            "url": "https://media.tenor.com/x8v1oNUOmg4AAAAd/rickroll-roll.gif",
            "preview": "https://media.tenor.com/x8v1oNUOmg4AAAAd/rickroll-roll.gif",
            "tags": "rickroll rick astley dance music meme troll never gonna give you up",
            "width": 320,
            "height": 240
        }),
    ];

    if let Some(q) = query {
        let q_lower = q.to_lowercase();
        let terms: Vec<&str> = q_lower.split_whitespace().collect();
        let filtered: Vec<Value> = all
            .into_iter()
            .filter(|item| {
                let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
                let tags = item.get("tags").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
                terms.iter().all(|t| title.contains(t) || tags.contains(t))
            })
            .collect();
        filtered
    } else {
        all
    }
}

fn curated_sticker_packs() -> Vec<Value> {
    vec![
        json!({
            "id": "pepe",
            "name": "Pepe & Memes",
            "icon": "https://cdn.jsdelivr.net/gh/twitter/twemoji@14.0.2/assets/72x72/1f438.png",
            "stickers": [
                { "id": "pepe-happy", "name": "Happy", "url": "https://media.tenor.com/z0w2yXN5m4EAAAAi/pepe-happy.gif" },
                { "id": "pepe-dance", "name": "Dance", "url": "https://media.tenor.com/r_z_2U-L_YAAAAAi/pepe-dance.gif" },
                { "id": "pepe-clap", "name": "Clap", "url": "https://media.tenor.com/o7sPq04sK04AAAAi/pepe-clap.gif" },
                { "id": "pepe-hacker", "name": "Hacker", "url": "https://media.tenor.com/C_iJ5-oM1Z4AAAAi/pepe-typing.gif" },
                { "id": "pepe-coffee", "name": "Coffee", "url": "https://media.tenor.com/Qh0b_O9u34sAAAAi/pepe-coffee.gif" },
                { "id": "pepe-rain", "name": "Sad Rain", "url": "https://media.tenor.com/o1nK2K4uE5IAAAAi/pepe-sad-rain.gif" },
                { "id": "pepe-cheer", "name": "Cheer", "url": "https://media.tenor.com/3Z6wTf2E9-AAAAAi/pepe-cheers.gif" },
                { "id": "pepe-smug", "name": "Smug", "url": "https://media.tenor.com/aC3QZ9vjJ9AAAAAi/pepe-smug.gif" }
            ]
        }),
        json!({
            "id": "cats",
            "name": "Cat Vibing",
            "icon": "https://cdn.jsdelivr.net/gh/twitter/twemoji@14.0.2/assets/72x72/1f431.png",
            "stickers": [
                { "id": "pop-cat", "name": "Pop Cat", "url": "https://media.tenor.com/2lF8i8l1g3QAAAAC/pop-cat.gif" },
                { "id": "vibing-cat", "name": "Cat Vibe", "url": "https://media.tenor.com/7wA2VwZt6xIAAAAC/cat-vibe.gif" },
                { "id": "bongo-cat", "name": "Bongo Cat", "url": "https://media.tenor.com/pM4bVpE0nHYAAAAC/bongo-cat.gif" },
                { "id": "cat-jam", "name": "Cat Jam", "url": "https://media.tenor.com/j4uQ9_4y_rIAAAAi/cat-jam.gif" },
                { "id": "cat-spin", "name": "Cat Spin", "url": "https://media.tenor.com/mOcf5uTf23IAAAAi/spinning-cat.gif" },
                { "id": "cat-pat", "name": "Pat Pat", "url": "https://media.tenor.com/5lV5sP8iXnEAAAAi/cat-pat.gif" }
            ]
        }),
        json!({
            "id": "reactions",
            "name": "Reactions & Emotes",
            "icon": "https://cdn.jsdelivr.net/gh/twitter/twemoji@14.0.2/assets/72x72/1f525.png",
            "stickers": [
                { "id": "fire", "name": "Fire", "url": "https://cdn.jsdelivr.net/gh/twitter/twemoji@14.0.2/assets/72x72/1f525.png" },
                { "id": "skull", "name": "Dead", "url": "https://cdn.jsdelivr.net/gh/twitter/twemoji@14.0.2/assets/72x72/1f480.png" },
                { "id": "100", "name": "100", "url": "https://cdn.jsdelivr.net/gh/twitter/twemoji@14.0.2/assets/72x72/1f4af.png" },
                { "id": "party", "name": "Party", "url": "https://cdn.jsdelivr.net/gh/twitter/twemoji@14.0.2/assets/72x72/1f389.png" },
                { "id": "eyes", "name": "Eyes", "url": "https://cdn.jsdelivr.net/gh/twitter/twemoji@14.0.2/assets/72x72/1f440.png" },
                { "id": "sparkles", "name": "Sparkles", "url": "https://cdn.jsdelivr.net/gh/twitter/twemoji@14.0.2/assets/72x72/2728.png" },
                { "id": "clown", "name": "Clown", "url": "https://cdn.jsdelivr.net/gh/twitter/twemoji@14.0.2/assets/72x72/1f921.png" },
                { "id": "salute", "name": "Salute", "url": "https://cdn.jsdelivr.net/gh/twitter/twemoji@14.0.2/assets/72x72/1fae1.png" }
            ]
        })
    ]
}

// ── Moderation endpoints ──

async fn mod_overview(state: &AppState, headers: &HeaderMap) -> Response {
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("");
    if !mitch_lib::auth::is_any_admin_id(&state.store, &state.id_secret, sid, false) {
        return cors_json_response(403, json!({ "error": "forbidden" }));
    }

    let room_id = match ensure_official_general_room(&state.id_secret).await {
        Ok(id) => id,
        Err(e) => return cors_json_response(500, json!({ "ok": false, "error": e })),
    };

    let admin_tok = match get_system_admin_matrix_token(&state.id_secret).await {
        Ok(t) => t,
        Err(e) => return cors_json_response(500, json!({ "ok": false, "error": e })),
    };

    let mut req_headers = HeaderMap::new();
    if let Ok(hv) = HeaderValue::from_str(&format!("Bearer {admin_tok}")) {
        req_headers.insert("Authorization", hv);
    }

    let pl_res = call_conduit(
        &format!(
            "/_matrix/client/v3/rooms/{}/state/m.room.power_levels",
            url::form_urlencoded::byte_serialize(room_id.as_bytes()).collect::<String>()
        ),
        Method::GET,
        Some(req_headers.clone()),
        None,
    )
    .await;

    let pl_data: Value = match pl_res {
        Ok((status, _, bytes)) if status.is_success() => {
            serde_json::from_slice(&bytes).unwrap_or(json!({}))
        }
        _ => json!({ "users": {} }),
    };

    let mut staff = Vec::new();
    if let Some(users) = pl_data.get("users").and_then(|v| v.as_object()) {
        for (m_user_id, pl_val) in users.iter() {
            let pl = pl_val.as_i64().unwrap_or(0);
            if pl >= 50 {
                staff.push(json!({
                    "userId": m_user_id,
                    "powerLevel": pl,
                    "role": if pl >= 100 { "Admin" } else { "Moderator" }
                }));
            }
        }
    }
    staff.sort_by(|a, b| {
        let b_pl = b.get("powerLevel").and_then(|v| v.as_i64()).unwrap_or(0);
        let a_pl = a.get("powerLevel").and_then(|v| v.as_i64()).unwrap_or(0);
        b_pl.cmp(&a_pl)
    });

    let reports_file = state.data_dir().join("chat_reports.json");
    let all_reports = state.store.read_document(&reports_file, json!([]));
    let mut matrix_reports = Vec::new();
    if let Some(arr) = all_reports.as_array() {
        for r in arr.iter().rev() {
            let is_matrix = r.get("matrixRoomId").is_some()
                || r.get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.starts_with("matrix-"))
                    .unwrap_or(false);
            if is_matrix {
                matrix_reports.push(r.clone());
                if matrix_reports.len() >= 50 {
                    break;
                }
            }
        }
    }

    let room_settings = load_matrix_room_settings(state, &room_id);
    let slowmode_seconds = room_settings
        .get("slowmodeSeconds")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let room_muted = room_settings
        .get("roomMuted")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut active_muted_users = Vec::new();
    if let Some(obj) = room_settings.get("mutedUsers").and_then(|v| v.as_object()) {
        let now = now_millis();
        for (m_id, entry) in obj.iter() {
            let exp = entry.get("expiresAt").and_then(|v| v.as_i64());
            if exp.is_none() || exp > Some(now) {
                active_muted_users.push(json!({
                    "userId": entry.get("userId").and_then(|v| v.as_str()).unwrap_or(m_id),
                    "reason": entry.get("reason").and_then(|v| v.as_str()).unwrap_or(""),
                    "mutedBy": entry.get("mutedBy").and_then(|v| v.as_str()).unwrap_or(""),
                    "mutedAt": entry.get("mutedAt").and_then(|v| v.as_i64()).unwrap_or(0),
                    "expiresAt": entry.get("expiresAt").and_then(|v| v.as_i64())
                }));
            }
        }
    }

    let mut banned_users = Vec::new();
    let mut seen_banned = std::collections::HashSet::new();

    if let Some(obj) = room_settings.get("bannedUsers").and_then(|v| v.as_object()) {
        for (m_id, entry) in obj.iter() {
            let uid = entry.get("userId").and_then(|v| v.as_str()).unwrap_or(m_id);
            if !uid.is_empty() && seen_banned.insert(uid.to_string()) {
                banned_users.push(json!({
                    "userId": uid,
                    "reason": entry.get("reason").and_then(|v| v.as_str()).unwrap_or(""),
                    "bannedBy": entry.get("bannedBy").and_then(|v| v.as_str()).unwrap_or(""),
                    "bannedAt": entry.get("bannedAt").and_then(|v| v.as_i64()).unwrap_or(0),
                }));
            }
        }
    }

    if let Ok((status, _, bytes)) = call_conduit(
        &format!(
            "/_matrix/client/v3/rooms/{}/members?membership=ban",
            url::form_urlencoded::byte_serialize(room_id.as_bytes()).collect::<String>()
        ),
        Method::GET,
        Some(req_headers.clone()),
        None,
    )
    .await
    {
        if status.is_success() {
            if let Ok(members_data) = serde_json::from_slice::<Value>(&bytes) {
                if let Some(chunk) = members_data.get("chunk").and_then(|v| v.as_array()) {
                    for ev in chunk {
                        let uid = ev.get("state_key").and_then(|v| v.as_str()).unwrap_or("");
                        let membership = ev
                            .get("content")
                            .and_then(|c| c.get("membership"))
                            .and_then(|m| m.as_str())
                            .unwrap_or("");
                        if membership == "ban"
                            && !uid.is_empty()
                            && seen_banned.insert(uid.to_string())
                        {
                            let reason = ev
                                .get("content")
                                .and_then(|c| c.get("reason"))
                                .and_then(|r| r.as_str())
                                .unwrap_or("");
                            let sender = ev.get("sender").and_then(|s| s.as_str()).unwrap_or("");
                            let ts = ev.get("origin_server_ts").and_then(|t| t.as_i64()).unwrap_or(0);
                            banned_users.push(json!({
                                "userId": uid,
                                "reason": reason,
                                "bannedBy": sender,
                                "bannedAt": ts,
                            }));
                        }
                    }
                }
            }
        }
    }

    let mut user_slowmodes = Vec::new();
    if let Some(obj) = room_settings.get("userSlowmode").and_then(|v| v.as_object()) {
        for (u_id, sec_val) in obj.iter() {
            let sec = sec_val.as_i64().unwrap_or(0);
            if sec > 0 {
                user_slowmodes.push(json!({
                    "userId": u_id,
                    "slowmodeSeconds": sec
                }));
            }
        }
    }

    cors_json_response(
        200,
        json!({
            "ok": true,
            "officialRoom": "#general:mitch.pro",
            "roomId": room_id,
            "staff": staff,
            "slowmodeSeconds": slowmode_seconds,
            "roomMuted": room_muted,
            "mutedUsers": active_muted_users,
            "bannedUsers": banned_users,
            "userSlowmodes": user_slowmodes,
            "recentReports": matrix_reports
        }),
    )
}

async fn mod_set_role(state: &AppState, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("");
    if !mitch_lib::auth::is_admin_id(&state.store, &state.id_secret, sid, false) {
        return cors_json_response(403, json!({ "error": "Admin access required" }));
    }

    let body_json: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
    let raw_input = body_json
        .get("userId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if raw_input.is_empty() {
        return cors_json_response(400, json!({ "error": "userId is required" }));
    }
    let target_user_id = resolve_all_matrix_user_ids_for_target(state, raw_input)
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            let clean: String = raw_input.chars().filter(|c| !c.is_whitespace()).collect();
            if clean.starts_with('@') && clean.contains(':') {
                clean
            } else {
                let u = clean.trim_start_matches('@').split(':').next().unwrap_or(&clean);
                format!("@{u}:mitch.pro")
            }
        });
    let target_pl = body_json
        .get("powerLevel")
        .and_then(|v| v.as_i64())
        .unwrap_or(-1);
    if !(0..=100).contains(&target_pl) {
        return cors_json_response(
            400,
            json!({ "error": "powerLevel must be between 0 and 100" }),
        );
    }

    let room_id = match body_json.get("roomId").and_then(|v| v.as_str()) {
        Some(r) if !r.is_empty() => r.to_string(),
        _ => match ensure_official_general_room(&state.id_secret).await {
            Ok(id) => id,
            Err(e) => return cors_json_response(500, json!({ "ok": false, "error": e })),
        },
    };

    let admin_tok = match get_system_admin_matrix_token(&state.id_secret).await {
        Ok(t) => t,
        Err(e) => return cors_json_response(500, json!({ "ok": false, "error": e })),
    };

    let mut req_headers = HeaderMap::new();
    if let Ok(hv) = HeaderValue::from_str(&format!("Bearer {admin_tok}")) {
        req_headers.insert("Authorization", hv);
    }

    let pl_res = call_conduit(
        &format!(
            "/_matrix/client/v3/rooms/{}/state/m.room.power_levels",
            url::form_urlencoded::byte_serialize(room_id.as_bytes()).collect::<String>()
        ),
        Method::GET,
        Some(req_headers.clone()),
        None,
    )
    .await;

    let mut pl_data: Value = match pl_res {
        Ok((status, _, bytes)) if status.is_success() => {
            serde_json::from_slice(&bytes).unwrap_or(json!({}))
        }
        _ => {
            return cors_json_response(
                500,
                json!({ "ok": false, "error": "Failed to fetch room power levels" }),
            )
        }
    };

    if let Some(users) = pl_data.get_mut("users").and_then(|v| v.as_object_mut()) {
        if target_pl > 0 {
            users.insert(target_user_id.clone(), json!(target_pl));
        } else {
            users.remove(&target_user_id);
        }
    }

    req_headers.insert("Content-Type", HeaderValue::from_static("application/json"));
    let put_res = call_conduit(
        &format!(
            "/_matrix/client/v3/rooms/{}/state/m.room.power_levels",
            url::form_urlencoded::byte_serialize(room_id.as_bytes()).collect::<String>()
        ),
        Method::PUT,
        Some(req_headers),
        Some(Bytes::from(
            serde_json::to_vec(&pl_data).unwrap_or_default(),
        )),
    )
    .await;

    match put_res {
        Ok((status, _, _)) if status.is_success() => {
            let admin_actor = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, sid)
                .unwrap_or_else(|| "admin".to_string());
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                &admin_actor,
                "matrix_set_role",
                json!({
                    "userId": target_user_id,
                    "powerLevel": target_pl,
                    "roomId": room_id
                }),
            );
            cors_json_response(
                200,
                json!({ "ok": true, "userId": target_user_id, "powerLevel": target_pl }),
            )
        }
        _ => cors_json_response(
            500,
            json!({ "ok": false, "error": "Failed to update power level state" }),
        ),
    }
}

async fn mod_kick(state: &AppState, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("");
    if !mitch_lib::auth::is_any_admin_id(&state.store, &state.id_secret, sid, false) {
        return cors_json_response(403, json!({ "error": "Staff access required" }));
    }

    let body_json: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
    let raw_input = body_json
        .get("userId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if raw_input.is_empty() {
        return cors_json_response(400, json!({ "error": "userId is required" }));
    }
    let target_user_id = resolve_all_matrix_user_ids_for_target(state, raw_input)
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            let clean: String = raw_input.chars().filter(|c| !c.is_whitespace()).collect();
            if clean.starts_with('@') && clean.contains(':') {
                clean
            } else {
                let u = clean.trim_start_matches('@').split(':').next().unwrap_or(&clean);
                format!("@{u}:mitch.pro")
            }
        });
    let reason = body_json
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("Kicked by Mitch.pro staff");

    let room_id = match body_json.get("roomId").and_then(|v| v.as_str()) {
        Some(r) if !r.is_empty() => r.to_string(),
        _ => match ensure_official_general_room(&state.id_secret).await {
            Ok(id) => id,
            Err(e) => return cors_json_response(500, json!({ "ok": false, "error": e })),
        },
    };

    let admin_tok = match get_system_admin_matrix_token(&state.id_secret).await {
        Ok(t) => t,
        Err(e) => return cors_json_response(500, json!({ "ok": false, "error": e })),
    };

    let mut req_headers = HeaderMap::new();
    req_headers.insert("Content-Type", HeaderValue::from_static("application/json"));
    if let Ok(hv) = HeaderValue::from_str(&format!("Bearer {admin_tok}")) {
        req_headers.insert("Authorization", hv);
    }

    let kick_payload = json!({ "user_id": target_user_id, "reason": reason });
    let res = call_conduit(
        &format!(
            "/_matrix/client/v3/rooms/{}/kick",
            url::form_urlencoded::byte_serialize(room_id.as_bytes()).collect::<String>()
        ),
        Method::POST,
        Some(req_headers),
        Some(Bytes::from(
            serde_json::to_vec(&kick_payload).unwrap_or_default(),
        )),
    )
    .await;

    match res {
        Ok((status, _, _)) if status.is_success() => {
            let admin_actor = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, sid)
                .unwrap_or_else(|| "admin".to_string());
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                &admin_actor,
                "matrix_kick_user",
                json!({
                    "userId": target_user_id,
                    "roomId": room_id,
                    "reason": reason
                }),
            );
            cors_json_response(
                200,
                json!({ "ok": true, "userId": target_user_id, "kicked": true }),
            )
        }
        _ => cors_json_response(500, json!({ "ok": false, "error": "Failed to kick user" })),
    }
}

async fn mod_ban(state: &AppState, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("");
    if !mitch_lib::auth::is_any_admin_id(&state.store, &state.id_secret, sid, false) {
        return cors_json_response(403, json!({ "error": "Staff access required" }));
    }

    let body_json: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
    let raw_input = body_json
        .get("userId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if raw_input.is_empty() {
        return cors_json_response(400, json!({ "error": "userId is required" }));
    }
    let target_user_id = resolve_all_matrix_user_ids_for_target(state, raw_input)
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            let clean: String = raw_input.chars().filter(|c| !c.is_whitespace()).collect();
            if clean.starts_with('@') && clean.contains(':') {
                clean
            } else {
                let u = clean.trim_start_matches('@').split(':').next().unwrap_or(&clean);
                format!("@{u}:mitch.pro")
            }
        });
    let reason = body_json
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("Banned by Mitch.pro staff");

    let room_id = match body_json.get("roomId").and_then(|v| v.as_str()) {
        Some(r) if !r.is_empty() => r.to_string(),
        _ => match ensure_official_general_room(&state.id_secret).await {
            Ok(id) => id,
            Err(e) => return cors_json_response(500, json!({ "ok": false, "error": e })),
        },
    };

    let admin_tok = match get_system_admin_matrix_token(&state.id_secret).await {
        Ok(t) => t,
        Err(e) => return cors_json_response(500, json!({ "ok": false, "error": e })),
    };

    let mut req_headers = HeaderMap::new();
    req_headers.insert("Content-Type", HeaderValue::from_static("application/json"));
    if let Ok(hv) = HeaderValue::from_str(&format!("Bearer {admin_tok}")) {
        req_headers.insert("Authorization", hv);
    }

    let ban_payload = json!({ "user_id": target_user_id, "reason": reason });
    let res = call_conduit(
        &format!(
            "/_matrix/client/v3/rooms/{}/ban",
            url::form_urlencoded::byte_serialize(room_id.as_bytes()).collect::<String>()
        ),
        Method::POST,
        Some(req_headers),
        Some(Bytes::from(
            serde_json::to_vec(&ban_payload).unwrap_or_default(),
        )),
    )
    .await;

    match res {
        Ok((status, _, _)) if status.is_success() => {
            let admin_actor = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, sid)
                .unwrap_or_else(|| "admin".to_string());
            let settings_file = state.data_dir().join("matrix_room_settings.json");
            let mut all = state.store.read_document(&settings_file, json!({}));
            if let Some(map) = all.as_object_mut() {
                let room = map.entry(room_id.clone()).or_insert_with(|| {
                    json!({
                        "slowmodeSeconds": 0,
                        "roomMuted": false,
                        "mutedUsers": {},
                        "bannedUsers": {},
                        "userSlowmode": {}
                    })
                });
                if room.get("bannedUsers").is_none() {
                    room["bannedUsers"] = json!({});
                }
                if let Some(b_map) = room.get_mut("bannedUsers").and_then(|v| v.as_object_mut()) {
                    b_map.insert(
                        target_user_id.clone(),
                        json!({
                            "userId": target_user_id,
                            "reason": reason,
                            "bannedBy": admin_actor,
                            "bannedAt": now_millis()
                        }),
                    );
                }
                let _ = state.store.write_document(&settings_file, &all);
            }

            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                &admin_actor,
                "matrix_ban_user",
                json!({
                    "userId": target_user_id,
                    "roomId": room_id,
                    "reason": reason
                }),
            );
            cors_json_response(
                200,
                json!({ "ok": true, "userId": target_user_id, "banned": true }),
            )
        }
        _ => cors_json_response(500, json!({ "ok": false, "error": "Failed to ban user" })),
    }
}

async fn mod_unban(state: &AppState, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("");
    if !mitch_lib::auth::is_any_admin_id(&state.store, &state.id_secret, sid, false) {
        return cors_json_response(403, json!({ "error": "Staff access required" }));
    }

    let body_json: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
    let raw_input = body_json
        .get("userId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if raw_input.is_empty() {
        return cors_json_response(400, json!({ "error": "userId is required" }));
    }

    let mut candidates = resolve_all_matrix_user_ids_for_target(state, raw_input);
    if candidates.is_empty() {
        let clean: String = raw_input.chars().filter(|c| !c.is_whitespace()).collect();
        let fallback = if clean.starts_with('@') && clean.contains(':') {
            clean
        } else {
            let u = clean.trim_start_matches('@').split(':').next().unwrap_or(&clean);
            format!("@{u}:mitch.pro")
        };
        candidates.push(fallback);
    }

    let explicit_room = body_json
        .get("roomId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();

    for candidate in &candidates {
        if explicit_room.is_empty() || explicit_room == "all" || explicit_room == "*" {
            unban_matrix_user_all_rooms(
                &state.id_secret,
                &state.store,
                &state.data_dir(),
                candidate,
            )
            .await;
        } else {
            unban_matrix_user_from_room(
                &state.id_secret,
                &state.store,
                &state.data_dir(),
                explicit_room,
                candidate,
            )
            .await;
        }
    }

    // Also unban at the account/SSO level if blacklisted or shadow-banned
    let associated_emails = resolve_all_emails_for_target(&state.store, &state.data_dir(), &state.id_secret, raw_input);
    for email in &associated_emails {
        for bl_path in [state.cfg.data_dir.join("blacklist.json"), state.store.base_dir.join("data/blacklist.json")] {
            let mut bl = state.store.read_document(&bl_path, json!({}));
            if let Some(map) = bl.as_object_mut() {
                let removed1 = map.remove(email);
                let removed2 = map.remove(email.to_lowercase().as_str());
                if removed1.is_some() || removed2.is_some() {
                    let _ = state.store.write_document(&bl_path, &bl);
                }
            }
        }

        {
            let mut bans = state.shadow_bans.write().unwrap_or_else(|e| e.into_inner());
            let removed1 = bans.remove(email);
            let removed2 = bans.remove(email.to_lowercase().as_str());
            if removed1 || removed2 {
                let arr: Vec<Value> = bans.iter().map(|b| json!(b)).collect();
                let _ = state.store.write_document(
                    &state.cfg.base_dir.join("data/shadow_bans.json"),
                    &json!(arr),
                );
            }
        }

        let last_known = state.store.read_document(&state.cfg.data_dir.join("last_known_ips.json"), json!({}));
        if let Some(target_ip) = last_known.get(email.as_str()).and_then(|v| v.as_str()).map(str::to_string) {
            if !mitch_lib::auth::WHITELISTED_IPS.contains(&target_ip.as_str()) {
                let banned_ips_file = state.cfg.data_dir.join("banned_ips.json");
                let mut banned = state.store.read_document(&banned_ips_file, json!({}));
                if let Some(map) = banned.as_object_mut() {
                    if map.remove(&target_ip).is_some() {
                        let _ = state.store.write_document(&banned_ips_file, &banned);
                    }
                }
            }
        }
    }

    let admin_actor = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, sid)
        .unwrap_or_else(|| "admin".to_string());
    mitch_lib::admin::log_admin_action(
        &state.store,
        &state.cfg.data_dir,
        &admin_actor,
        "matrix_unban_user",
        json!({
            "userId": raw_input,
            "resolvedCandidates": candidates,
            "associatedEmails": associated_emails,
            "roomId": if explicit_room.is_empty() { "all" } else { explicit_room }
        }),
    );

    cors_json_response(
        200,
        json!({
            "ok": true,
            "userId": raw_input,
            "unbannedUserIds": candidates,
            "unbannedEmails": associated_emails,
            "unbanned": true
        }),
    )
}

async fn mod_redact(state: &AppState, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("");
    if !mitch_lib::auth::is_any_admin_id(&state.store, &state.id_secret, sid, false) {
        return cors_json_response(403, json!({ "error": "Staff access required" }));
    }

    let body_json: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
    let event_id = body_json
        .get("eventId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if event_id.is_empty() {
        return cors_json_response(400, json!({ "error": "eventId is required" }));
    }
    let reason = body_json
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("Redacted by staff");

    let room_id = match body_json.get("roomId").and_then(|v| v.as_str()) {
        Some(r) if !r.is_empty() => r.to_string(),
        _ => match ensure_official_general_room(&state.id_secret).await {
            Ok(id) => id,
            Err(e) => return cors_json_response(500, json!({ "ok": false, "error": e })),
        },
    };

    let admin_tok = match get_system_admin_matrix_token(&state.id_secret).await {
        Ok(t) => t,
        Err(e) => return cors_json_response(500, json!({ "ok": false, "error": e })),
    };

    let mut req_headers = HeaderMap::new();
    req_headers.insert("Content-Type", HeaderValue::from_static("application/json"));
    if let Ok(hv) = HeaderValue::from_str(&format!("Bearer {admin_tok}")) {
        req_headers.insert("Authorization", hv);
    }

    let txn_id = format!("mitch_redact_{}", now_millis());
    let redact_payload = json!({ "reason": reason });
    let res = call_conduit(
        &format!(
            "/_matrix/client/v3/rooms/{}/redact/{}/{}",
            url::form_urlencoded::byte_serialize(room_id.as_bytes()).collect::<String>(),
            url::form_urlencoded::byte_serialize(event_id.as_bytes()).collect::<String>(),
            url::form_urlencoded::byte_serialize(txn_id.as_bytes()).collect::<String>()
        ),
        Method::PUT,
        Some(req_headers),
        Some(Bytes::from(
            serde_json::to_vec(&redact_payload).unwrap_or_default(),
        )),
    )
    .await;

    match res {
        Ok((status, _, _)) if status.is_success() => {
            let admin_actor = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, sid)
                .unwrap_or_else(|| "admin".to_string());
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                &admin_actor,
                "matrix_redact_message",
                json!({
                    "eventId": event_id,
                    "roomId": room_id,
                    "reason": reason
                }),
            );
            cors_json_response(
                200,
                json!({ "ok": true, "eventId": event_id, "redacted": true }),
            )
        }
        _ => cors_json_response(
            500,
            json!({ "ok": false, "error": "Failed to redact message" }),
        ),
    }
}

async fn mod_slowmode(state: &AppState, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("");
    if !mitch_lib::auth::is_any_admin_id(&state.store, &state.id_secret, sid, false) {
        return cors_json_response(403, json!({ "error": "Staff access required" }));
    }

    let body_json: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
    let seconds = std::cmp::max(
        0,
        body_json
            .get("seconds")
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
    );
    let user_id_opt = body_json
        .get("userId")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());

    let room_id = match body_json.get("roomId").and_then(|v| v.as_str()) {
        Some(r) if !r.is_empty() => r.to_string(),
        _ => match ensure_official_general_room(&state.id_secret).await {
            Ok(id) => id,
            Err(e) => return cors_json_response(500, json!({ "ok": false, "error": e })),
        },
    };

    let admin_actor = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, sid)
        .unwrap_or_else(|| "admin".to_string());

    let settings_file = state.data_dir().join("matrix_room_settings.json");
    let mut all = state.store.read_document(&settings_file, json!({}));

    if let Some(target_uid_raw) = user_id_opt {
        let target_user_id = resolve_all_matrix_user_ids_for_target(state, target_uid_raw)
            .into_iter()
            .next()
            .unwrap_or_else(|| {
                let clean: String = target_uid_raw.chars().filter(|c| !c.is_whitespace()).collect();
                if clean.starts_with('@') && clean.contains(':') {
                    clean
                } else {
                    let u = clean.trim_start_matches('@').split(':').next().unwrap_or(&clean);
                    format!("@{u}:mitch.pro")
                }
            });
        let uname = target_user_id
            .trim_start_matches('@')
            .split(':')
            .next()
            .unwrap_or("")
            .to_string();

        if let Some(map) = all.as_object_mut() {
            let room = map.entry(room_id.clone()).or_insert_with(|| {
                json!({
                    "slowmodeSeconds": 0,
                    "roomMuted": false,
                    "mutedUsers": {},
                    "bannedUsers": {},
                    "userSlowmode": {}
                })
            });
            if room.get("userSlowmode").is_none() {
                room["userSlowmode"] = json!({});
            }
            if let Some(us_map) = room.get_mut("userSlowmode").and_then(|v| v.as_object_mut()) {
                if seconds > 0 {
                    us_map.insert(target_user_id.clone(), json!(seconds));
                } else {
                    us_map.remove(&target_user_id);
                    us_map.remove(&uname);
                }
            }
            let _ = state.store.write_document(&settings_file, &all);
        }

        mitch_lib::admin::log_admin_action(
            &state.store,
            &state.cfg.data_dir,
            &admin_actor,
            "matrix_set_user_slowmode",
            json!({
                "roomId": room_id,
                "userId": target_user_id,
                "slowmodeSeconds": seconds
            }),
        );

        return cors_json_response(
            200,
            json!({ "ok": true, "roomId": room_id, "userId": target_user_id, "slowmodeSeconds": seconds }),
        );
    }

    if let Some(map) = all.as_object_mut() {
        let room = map.entry(room_id.clone()).or_insert_with(|| {
            json!({
                "slowmodeSeconds": 0,
                "roomMuted": false,
                "mutedUsers": {},
                "bannedUsers": {},
                "userSlowmode": {}
            })
        });
        room["slowmodeSeconds"] = json!(seconds);
        let _ = state.store.write_document(&settings_file, &all);
    }

    mitch_lib::admin::log_admin_action(
        &state.store,
        &state.cfg.data_dir,
        &admin_actor,
        "matrix_set_slowmode",
        json!({
            "roomId": room_id,
            "slowmodeSeconds": seconds
        }),
    );

    cors_json_response(
        200,
        json!({ "ok": true, "roomId": room_id, "slowmodeSeconds": seconds }),
    )
}

async fn mod_mute_user(state: &AppState, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("");
    if !mitch_lib::auth::is_admin_id(&state.store, &state.id_secret, sid, false) {
        return cors_json_response(403, json!({ "error": "Admin access required" }));
    }

    let body_json: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
    let raw_input = body_json
        .get("userId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if raw_input.is_empty() {
        return cors_json_response(400, json!({ "error": "userId is required" }));
    }
    let target_user_id = resolve_all_matrix_user_ids_for_target(state, raw_input)
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            let clean: String = raw_input.chars().filter(|c| !c.is_whitespace()).collect();
            if clean.starts_with('@') && clean.contains(':') {
                clean
            } else {
                let u = clean.trim_start_matches('@').split(':').next().unwrap_or(&clean);
                format!("@{u}:mitch.pro")
            }
        });

    let duration_seconds = std::cmp::max(
        0,
        body_json
            .get("durationSeconds")
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
    );
    let reason = body_json
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("Muted by administrator");
    let room_id = match body_json.get("roomId").and_then(|v| v.as_str()) {
        Some(r) if !r.is_empty() => r.to_string(),
        _ => match ensure_official_general_room(&state.id_secret).await {
            Ok(id) => id,
            Err(e) => return cors_json_response(500, json!({ "ok": false, "error": e })),
        },
    };

    let expires_at = if duration_seconds > 0 {
        Some(now_millis() + duration_seconds * 1000)
    } else {
        None
    };

    let admin_actor = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, sid)
        .unwrap_or_else(|| "admin".to_string());

    let settings_file = state.data_dir().join("matrix_room_settings.json");
    let mut all = state.store.read_document(&settings_file, json!({}));
    if let Some(map) = all.as_object_mut() {
        let room = map.entry(room_id.clone()).or_insert_with(|| {
            json!({
                "slowmodeSeconds": 0,
                "roomMuted": false,
                "mutedUsers": {}
            })
        });
        if room.get("mutedUsers").is_none() {
            room["mutedUsers"] = json!({});
        }
        if let Some(m_map) = room.get_mut("mutedUsers").and_then(|v| v.as_object_mut()) {
            m_map.insert(
                target_user_id.clone(),
                json!({
                    "userId": target_user_id,
                    "reason": reason,
                    "mutedBy": admin_actor,
                    "mutedAt": now_millis(),
                    "expiresAt": expires_at
                }),
            );
        }
        let _ = state.store.write_document(&settings_file, &all);
    }

    // Attempt power level -1 in Conduit
    if let Ok(admin_tok) = get_system_admin_matrix_token(&state.id_secret).await {
        let mut req_headers = HeaderMap::new();
        if let Ok(hv) = HeaderValue::from_str(&format!("Bearer {admin_tok}")) {
            req_headers.insert("Authorization", hv);
        }
        if let Ok((status, _, bytes)) = call_conduit(
            &format!(
                "/_matrix/client/v3/rooms/{}/state/m.room.power_levels",
                url::form_urlencoded::byte_serialize(room_id.as_bytes()).collect::<String>()
            ),
            Method::GET,
            Some(req_headers.clone()),
            None,
        )
        .await
        {
            if status.is_success() {
                if let Ok(mut pl_data) = serde_json::from_slice::<Value>(&bytes) {
                    if let Some(users) = pl_data.get_mut("users").and_then(|v| v.as_object_mut()) {
                        users.insert(target_user_id.clone(), json!(-1));
                        req_headers
                            .insert("Content-Type", HeaderValue::from_static("application/json"));
                        let _ = call_conduit(
                            &format!(
                                "/_matrix/client/v3/rooms/{}/state/m.room.power_levels",
                                url::form_urlencoded::byte_serialize(room_id.as_bytes())
                                    .collect::<String>()
                            ),
                            Method::PUT,
                            Some(req_headers),
                            Some(Bytes::from(
                                serde_json::to_vec(&pl_data).unwrap_or_default(),
                            )),
                        )
                        .await;
                    }
                }
            }
        }
    }

    mitch_lib::admin::log_admin_action(
        &state.store,
        &state.cfg.data_dir,
        &admin_actor,
        "matrix_mute_user",
        json!({
            "roomId": room_id,
            "userId": target_user_id,
            "durationSeconds": duration_seconds,
            "reason": reason,
            "expiresAt": expires_at
        }),
    );

    cors_json_response(
        200,
        json!({ "ok": true, "roomId": room_id, "userId": target_user_id, "muted": true, "expiresAt": expires_at }),
    )
}

async fn mod_unmute_user(state: &AppState, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("");
    if !mitch_lib::auth::is_admin_id(&state.store, &state.id_secret, sid, false) {
        return cors_json_response(403, json!({ "error": "Admin access required" }));
    }

    let body_json: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
    let mut target_user_id = body_json
        .get("userId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if target_user_id.is_empty() {
        return cors_json_response(400, json!({ "error": "userId is required" }));
    }
    if !target_user_id.starts_with('@') {
        target_user_id = format!("@{target_user_id}:mitch.pro");
    }

    let room_id = match body_json.get("roomId").and_then(|v| v.as_str()) {
        Some(r) if !r.is_empty() => r.to_string(),
        _ => match ensure_official_general_room(&state.id_secret).await {
            Ok(id) => id,
            Err(e) => return cors_json_response(500, json!({ "ok": false, "error": e })),
        },
    };

    let settings_file = state.data_dir().join("matrix_room_settings.json");
    let mut all = state.store.read_document(&settings_file, json!({}));
    if let Some(room) = all.get_mut(&room_id) {
        if let Some(m_map) = room.get_mut("mutedUsers").and_then(|v| v.as_object_mut()) {
            m_map.remove(&target_user_id);
            let uname = target_user_id
                .trim_start_matches('@')
                .split(':')
                .next()
                .unwrap_or("");
            m_map.remove(uname);
            let _ = state.store.write_document(&settings_file, &all);
        }
    }

    // Reset PL -1 in Conduit
    if let Ok(admin_tok) = get_system_admin_matrix_token(&state.id_secret).await {
        let mut req_headers = HeaderMap::new();
        if let Ok(hv) = HeaderValue::from_str(&format!("Bearer {admin_tok}")) {
            req_headers.insert("Authorization", hv);
        }
        if let Ok((status, _, bytes)) = call_conduit(
            &format!(
                "/_matrix/client/v3/rooms/{}/state/m.room.power_levels",
                url::form_urlencoded::byte_serialize(room_id.as_bytes()).collect::<String>()
            ),
            Method::GET,
            Some(req_headers.clone()),
            None,
        )
        .await
        {
            if status.is_success() {
                if let Ok(mut pl_data) = serde_json::from_slice::<Value>(&bytes) {
                    if let Some(users) = pl_data.get_mut("users").and_then(|v| v.as_object_mut()) {
                        if users.get(&target_user_id).and_then(|v| v.as_i64()) == Some(-1) {
                            users.remove(&target_user_id);
                            req_headers.insert(
                                "Content-Type",
                                HeaderValue::from_static("application/json"),
                            );
                            let _ = call_conduit(
                                &format!(
                                    "/_matrix/client/v3/rooms/{}/state/m.room.power_levels",
                                    url::form_urlencoded::byte_serialize(room_id.as_bytes())
                                        .collect::<String>()
                                ),
                                Method::PUT,
                                Some(req_headers),
                                Some(Bytes::from(
                                    serde_json::to_vec(&pl_data).unwrap_or_default(),
                                )),
                            )
                            .await;
                        }
                    }
                }
            }
        }
    }

    let admin_actor = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, sid)
        .unwrap_or_else(|| "admin".to_string());
    mitch_lib::admin::log_admin_action(
        &state.store,
        &state.cfg.data_dir,
        &admin_actor,
        "matrix_unmute_user",
        json!({
            "roomId": room_id,
            "userId": target_user_id
        }),
    );

    cors_json_response(
        200,
        json!({ "ok": true, "roomId": room_id, "userId": target_user_id, "muted": false }),
    )
}

async fn mod_mute_room(state: &AppState, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("");
    if !mitch_lib::auth::is_admin_id(&state.store, &state.id_secret, sid, false) {
        return cors_json_response(403, json!({ "error": "Admin access required" }));
    }

    let body_json: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
    let room_muted = body_json
        .get("muted")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let room_id = match body_json.get("roomId").and_then(|v| v.as_str()) {
        Some(r) if !r.is_empty() => r.to_string(),
        _ => match ensure_official_general_room(&state.id_secret).await {
            Ok(id) => id,
            Err(e) => return cors_json_response(500, json!({ "ok": false, "error": e })),
        },
    };

    let settings_file = state.data_dir().join("matrix_room_settings.json");
    let mut all = state.store.read_document(&settings_file, json!({}));
    if let Some(map) = all.as_object_mut() {
        let room = map.entry(room_id.clone()).or_insert_with(|| {
            json!({
                "slowmodeSeconds": 0,
                "roomMuted": false,
                "mutedUsers": {}
            })
        });
        room["roomMuted"] = json!(room_muted);
        let _ = state.store.write_document(&settings_file, &all);
    }

    // Set events_default in Conduit (50 if muted, 0 if normal)
    if let Ok(admin_tok) = get_system_admin_matrix_token(&state.id_secret).await {
        let mut req_headers = HeaderMap::new();
        if let Ok(hv) = HeaderValue::from_str(&format!("Bearer {admin_tok}")) {
            req_headers.insert("Authorization", hv);
        }
        if let Ok((status, _, bytes)) = call_conduit(
            &format!(
                "/_matrix/client/v3/rooms/{}/state/m.room.power_levels",
                url::form_urlencoded::byte_serialize(room_id.as_bytes()).collect::<String>()
            ),
            Method::GET,
            Some(req_headers.clone()),
            None,
        )
        .await
        {
            if status.is_success() {
                if let Ok(mut pl_data) = serde_json::from_slice::<Value>(&bytes) {
                    pl_data["events_default"] = json!(if room_muted { 50 } else { 0 });
                    req_headers
                        .insert("Content-Type", HeaderValue::from_static("application/json"));
                    let _ = call_conduit(
                        &format!(
                            "/_matrix/client/v3/rooms/{}/state/m.room.power_levels",
                            url::form_urlencoded::byte_serialize(room_id.as_bytes())
                                .collect::<String>()
                        ),
                        Method::PUT,
                        Some(req_headers),
                        Some(Bytes::from(
                            serde_json::to_vec(&pl_data).unwrap_or_default(),
                        )),
                    )
                    .await;
                }
            }
        }
    }

    let admin_actor = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, sid)
        .unwrap_or_else(|| "admin".to_string());
    mitch_lib::admin::log_admin_action(
        &state.store,
        &state.cfg.data_dir,
        &admin_actor,
        "matrix_mute_room",
        json!({
            "roomId": room_id,
            "roomMuted": room_muted
        }),
    );

    cors_json_response(
        200,
        json!({ "ok": true, "roomId": room_id, "roomMuted": room_muted }),
    )
}

async fn mod_prune_stale(state: &AppState, headers: &HeaderMap, body_bytes: &[u8]) -> Response {
    let cookies = crate::routes::me::cookies_of(state, headers);
    let sid = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .or_else(|| cookies.get("id"))
        .unwrap_or("");
    let body_json: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));

    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let user_token = auth_header.trim_start_matches("Bearer ").trim().to_string();

    let mut current_device_id = body_json
        .get("currentDeviceId")
        .and_then(|v| v.as_str())
        .or_else(|| {
            headers
                .get("x-matrix-device-id")
                .and_then(|v| v.to_str().ok())
        })
        .unwrap_or("")
        .to_string();

    let max_age_days = body_json
        .get("maxAgeDays")
        .and_then(|v| v.as_f64())
        .unwrap_or(7.0);
    let prune_all_except_current = body_json
        .get("pruneAllExceptCurrent")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let target_uid = if !sid.is_empty() && mitch_lib::auth::valid_id(sid, &state.id_secret) {
        sid.to_string()
    } else {
        String::new()
    };
    let mut target_username = String::new();

    if !target_uid.is_empty() {
        let matrix_users_file = state.data_dir().join("matrix_users.json");
        let matrix_users = state.store.read_document(&matrix_users_file, json!({}));
        if let Some(name) = matrix_users.get(&target_uid).and_then(|v| v.as_str()) {
            target_username = name.to_string();
        }
    }

    if user_token.is_empty() {
        return cors_json_response(
            401,
            json!({ "error": "Authentication required to prune devices" }),
        );
    }

    let mut dev_headers = HeaderMap::new();
    if let Ok(hv) = HeaderValue::from_str(&format!("Bearer {user_token}")) {
        dev_headers.insert("Authorization", hv);
    }

    if let Ok((status, _, bytes)) = call_conduit(
        "/_matrix/client/v3/account/whoami",
        Method::GET,
        Some(dev_headers.clone()),
        None,
    )
    .await
    {
        if status.is_success() {
            if let Ok(who) = serde_json::from_slice::<Value>(&bytes) {
                if let Some(did) = who.get("device_id").and_then(|v| v.as_str()) {
                    if current_device_id.is_empty() {
                        current_device_id = did.to_string();
                    }
                }
                if let Some(uid) = who.get("user_id").and_then(|v| v.as_str()) {
                    if target_username.is_empty() {
                        let local = uid
                            .strip_prefix('@')
                            .unwrap_or(uid)
                            .split(':')
                            .next()
                            .unwrap_or("");
                        target_username = local.to_string();
                    }
                }
            }
        }
    }

    if let Ok((status, _, bytes)) = call_conduit(
        "/_matrix/client/v3/devices",
        Method::GET,
        Some(dev_headers.clone()),
        None,
    )
    .await
    {
        if status.is_success() {
            let dev_data: Value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
            let devices = dev_data
                .get("devices")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let now = now_millis();
            let max_age_ms = (max_age_days.max(1.0) * 86_400_000.0) as i64;
            let mut to_delete = Vec::new();

            for dev in &devices {
                let d_id = dev.get("device_id").and_then(|v| v.as_str()).unwrap_or("");
                if d_id.is_empty() {
                    continue;
                }
                if !current_device_id.is_empty() && d_id == current_device_id {
                    continue;
                }
                if prune_all_except_current {
                    to_delete.push(d_id.to_string());
                    continue;
                }
                let last_seen = dev
                    .get("last_seen_ts")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                if last_seen == 0 || (now - last_seen) > max_age_ms {
                    to_delete.push(d_id.to_string());
                }
            }

            if !to_delete.is_empty() {
                let del_body =
                    serde_json::to_vec(&json!({ "devices": to_delete })).unwrap_or_default();
                let del_res = call_conduit(
                    "/_matrix/client/v3/delete_devices",
                    Method::POST,
                    Some(dev_headers.clone()),
                    Some(Bytes::from(del_body)),
                )
                .await;

                if let Ok((del_status, _, _)) = del_res {
                    if del_status == StatusCode::UNAUTHORIZED && !target_uid.is_empty() {
                        let conduit_pass =
                            get_matrix_password_for_uid(&target_uid, &state.id_secret);
                        let uia_body = serde_json::to_vec(&json!({
                            "devices": to_delete,
                            "auth": {
                                "type": "m.login.password",
                                "identifier": { "type": "m.id.user", "user": target_username },
                                "password": conduit_pass
                            }
                        }))
                        .unwrap_or_default();
                        let _ = call_conduit(
                            "/_matrix/client/v3/delete_devices",
                            Method::POST,
                            Some(dev_headers.clone()),
                            Some(Bytes::from(uia_body)),
                        )
                        .await;
                    }
                }
            }

            let pruned_count = to_delete.len();
            let remaining_count = devices.len().saturating_sub(pruned_count);
            return cors_json_response(
                200,
                json!({
                    "ok": true,
                    "prunedCount": pruned_count,
                    "prunedDevices": to_delete,
                    "remainingCount": remaining_count
                }),
            );
        }
    }

    cors_json_response(
        500,
        json!({ "ok": false, "error": "Failed to list user devices" }),
    )
}

async fn translate_matrix_password(
    state: &AppState,
    headers: &HeaderMap,
    path: &str,
    body_bytes: &[u8],
) -> (bool, Vec<u8>) {
    let Ok(mut parsed) = serde_json::from_slice::<Value>(body_bytes) else {
        return (false, body_bytes.to_vec());
    };
    if !parsed.is_object() {
        return (false, body_bytes.to_vec());
    }

    let has_auth_password = parsed
        .get("auth")
        .and_then(|a| a.get("password"))
        .and_then(|p| p.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let has_root_password = parsed
        .get("password")
        .and_then(|p| p.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    if !has_auth_password && !has_root_password {
        return (false, body_bytes.to_vec());
    }

    let Some(account) = resolve_matrix_account(state, headers, Some(&parsed)).await else {
        return (false, body_bytes.to_vec());
    };
    if account.uid.is_empty() || account.norm_email.is_empty() {
        return (false, body_bytes.to_vec());
    }

    let passwords = state
        .store
        .read_document(&state.data_dir().join("passwords.json"), json!({}));
    let stored_hash = passwords
        .get(&account.norm_email)
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let conduit_pass = get_matrix_password_for_uid(&account.uid, &state.id_secret);
    let mut changed = false;

    if has_auth_password {
        let entered = parsed["auth"]["password"]
            .as_str()
            .unwrap_or("")
            .to_string();
        if entered == conduit_pass {
            // Already internal conduit password
        } else if !stored_hash.is_empty() {
            let valid = mitch_lib::crypto::argon2_verify(stored_hash, &entered);
            if valid {
                parsed["auth"]["password"] = json!(conduit_pass);
                changed = true;
                if path.contains("/account/password") && parsed.get("new_password").is_some() {
                    if let Some(new_p) = parsed.get("new_password").and_then(|v| v.as_str()) {
                        let new_hash = mitch_lib::crypto::argon2_hash(new_p);
                        if !new_hash.is_empty() {
                            let mut pw_map = passwords.clone();
                            if let Some(obj) = pw_map.as_object_mut() {
                                obj.insert(account.norm_email.clone(), json!(new_hash));
                                let _ = state.store.write_document(
                                    &state.data_dir().join("passwords.json"),
                                    &pw_map,
                                );
                            }
                            parsed["new_password"] = json!(conduit_pass);
                        }
                    }
                }
            }
        }
    }

    if has_root_password {
        let entered = parsed["password"].as_str().unwrap_or("").to_string();
        if entered == conduit_pass {
            // Already internal conduit password
        } else if !stored_hash.is_empty() {
            let valid = mitch_lib::crypto::argon2_verify(stored_hash, &entered);
            if valid {
                parsed["password"] = json!(conduit_pass);
                changed = true;
            }
        }
    }

    if changed {
        (
            true,
            serde_json::to_vec(&parsed).unwrap_or_else(|_| body_bytes.to_vec()),
        )
    } else {
        (false, body_bytes.to_vec())
    }
}

pub fn record_matrix_email_sent(state: &AppState, member_norm: &str) {
    let norm = mitch_lib::auth::normalize_email(member_norm);
    if norm.is_empty() {
        return;
    }
    let email_sent_file = state.data_dir().join("matrix_email_sent.json");
    let mut sent_map = state.store.read_document(&email_sent_file, json!({}));
    if !sent_map.is_object() {
        sent_map = json!({});
    }
    if let Some(obj) = sent_map.as_object_mut() {
        obj.insert(norm, json!(now_millis()));
        let _ = state.store.write_document(&email_sent_file, &sent_map);
    }
}

pub fn add_matrix_notification(state: &AppState, target_norm: &str, notif: &Value) {
    if target_norm.is_empty() {
        return;
    }
    let notif_file = state.data_dir().join("matrix_notifications.json");
    let mut all = state.store.read_document(&notif_file, json!({}));
    if !all.is_object() {
        all = json!({});
    }

    let room_id = notif.get("roomId").and_then(|v| v.as_str()).unwrap_or("");
    let n_type = notif
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("matrix");
    let sender = notif
        .get("sender")
        .and_then(|v| v.as_str())
        .unwrap_or("Someone");
    let room_title = notif
        .get("roomTitle")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let is_direct = notif
        .get("isDirect")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let now = now_millis();

    let list = match all.get_mut(target_norm).and_then(|v| v.as_array_mut()) {
        Some(l) => l,
        None => {
            if let Some(obj) = all.as_object_mut() {
                obj.entry(target_norm.to_string())
                    .or_insert_with(|| json!([]));
                if let Some(l) = obj.get_mut(target_norm).and_then(|v| v.as_array_mut()) {
                    l
                } else {
                    return;
                }
            } else {
                return;
            }
        }
    };

    let existing_idx = list.iter().position(|n| {
        n.get("read") != Some(&json!(true))
            && n.get("roomId").and_then(|v| v.as_str()) == Some(room_id)
            && n.get("type").and_then(|v| v.as_str()) == Some(n_type)
    });

    if let Some(idx) = existing_idx {
        let mut existing = list.remove(idx);
        let count = existing.get("count").and_then(|v| v.as_i64()).unwrap_or(1) + 1;
        existing["count"] = json!(count);
        existing["ts"] = json!(now);
        if let Some(detail) = notif.get("detail").and_then(|v| v.as_str()) {
            if !detail.is_empty() {
                existing["detail"] = json!(detail);
            }
        }
        if n_type == "matrix_call" {
            existing["title"] = json!(format!("📞 Active call from {sender}"));
        } else if n_type == "matrix_invite" {
            if let Some(t) = notif.get("title").and_then(|v| v.as_str()) {
                existing["title"] = json!(t);
            }
        } else {
            let updated_title = if is_direct {
                format!("{count} messages from {sender}")
            } else if !room_title.is_empty() {
                format!("{count} new messages in {room_title}")
            } else {
                format!("{count} new messages in chat")
            };
            existing["title"] = json!(updated_title);
        }
        list.insert(0, existing);
    } else {
        let item_id = notif
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("matrix:{room_id}:{now}"));
        let default_title = if is_direct {
            format!("Message from {sender}")
        } else if !room_title.is_empty() {
            format!("New message from {sender} in {room_title}")
        } else {
            "New message in chat".to_string()
        };
        let title = notif
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or(&default_title);
        let body = notif
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or("Matrix Chat");
        let detail = notif.get("detail").and_then(|v| v.as_str()).unwrap_or("");
        let enc_room = urlencoding_encode(room_id);
        let default_url = format!("/matrix/#/room/{enc_room}");
        let url = notif
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or(&default_url);

        let item = json!({
            "id": item_id,
            "type": n_type,
            "roomId": room_id,
            "title": title,
            "body": body,
            "detail": detail,
            "sender": sender,
            "roomTitle": room_title,
            "isDirect": is_direct,
            "count": 1,
            "ts": now,
            "url": url,
            "read": false
        });
        list.insert(0, item);
    }

    if list.len() > 50 {
        list.truncate(50);
    }

    let _ = state.store.write_document(&notif_file, &all);
    crate::ws::trigger_notification_refresh(state);
}

#[derive(Clone)]
struct MatrixRoomCachedInfo {
    members: Vec<String>,
    name: String,
    ts: i64,
}

static ROOM_INFO_CACHE: OnceLock<Mutex<HashMap<String, MatrixRoomCachedInfo>>> = OnceLock::new();
fn room_info_cache() -> &'static Mutex<HashMap<String, MatrixRoomCachedInfo>> {
    ROOM_INFO_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn get_matrix_room_info_for_notifications(
    room_id: &str,
    token: &str,
    secret: &[u8],
) -> (Vec<String>, String) {
    let now = now_millis();
    {
        let cache = room_info_cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = cache.get(room_id) {
            if now.saturating_sub(entry.ts) < 30_000 && !entry.members.is_empty() {
                return (entry.members.clone(), entry.name.clone());
            }
        }
    }

    let mut members = Vec::new();
    let mut name = String::new();

    let mut headers = HeaderMap::new();
    let eff_token = if !token.is_empty() {
        token.to_string()
    } else {
        get_system_admin_matrix_token(secret)
            .await
            .unwrap_or_default()
    };
    if !eff_token.is_empty() {
        let auth_val = if eff_token.to_lowercase().starts_with("bearer ") {
            eff_token
        } else {
            format!("Bearer {eff_token}")
        };
        if let Ok(hv) = HeaderValue::from_str(&auth_val) {
            headers.insert("authorization", hv);
        }
    }

    let enc_room = urlencoding_encode(room_id);
    let mem_subpath = format!("/_matrix/client/v3/rooms/{enc_room}/joined_members");
    if let Ok((status, _, bytes)) =
        call_conduit(&mem_subpath, Method::GET, Some(headers.clone()), None).await
    {
        if status.is_success() {
            if let Ok(val) = serde_json::from_slice::<Value>(&bytes) {
                if let Some(joined) = val.get("joined").and_then(|v| v.as_object()) {
                    members = joined.keys().cloned().collect();
                }
            }
        }
    }

    let name_subpath = format!("/_matrix/client/v3/rooms/{enc_room}/state/m.room.name");
    if let Ok((status, _, bytes)) =
        call_conduit(&name_subpath, Method::GET, Some(headers.clone()), None).await
    {
        if status.is_success() {
            if let Ok(val) = serde_json::from_slice::<Value>(&bytes) {
                if let Some(n) = val.get("name").and_then(|v| v.as_str()) {
                    name = n.to_string();
                }
            }
        }
    }
    if name.is_empty() {
        let alias_subpath =
            format!("/_matrix/client/v3/rooms/{enc_room}/state/m.room.canonical_alias");
        if let Ok((status, _, bytes)) =
            call_conduit(&alias_subpath, Method::GET, Some(headers.clone()), None).await
        {
            if status.is_success() {
                if let Ok(val) = serde_json::from_slice::<Value>(&bytes) {
                    if let Some(a) = val.get("alias").and_then(|v| v.as_str()) {
                        name = a.to_string();
                    }
                }
            }
        }
    }

    if !members.is_empty() {
        let mut cache = room_info_cache().lock().unwrap_or_else(|e| e.into_inner());
        cache.insert(
            room_id.to_string(),
            MatrixRoomCachedInfo {
                members: members.clone(),
                name: name.clone(),
                ts: now,
            },
        );
    }

    (members, name)
}

fn resolve_member_norm(state: &AppState, member_id: &str) -> String {
    let local_part = member_id
        .strip_prefix('@')
        .unwrap_or(member_id)
        .split(':')
        .next()
        .unwrap_or("");
    if local_part.is_empty() {
        return String::new();
    }

    let matrix_users_file = state.data_dir().join("matrix_users.json");
    let matrix_users = state.store.read_document(&matrix_users_file, json!({}));
    if let Some(obj) = matrix_users.as_object() {
        for (sid, name_val) in obj {
            let name_str = name_val.as_str().unwrap_or("");
            if name_str.eq_ignore_ascii_case(local_part)
                || format!("@{name_str}:mitch.pro").eq_ignore_ascii_case(member_id)
            {
                if let Some(em) =
                    mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, sid)
                {
                    let norm = mitch_lib::auth::normalize_email(&em);
                    if !norm.is_empty() {
                        return norm;
                    }
                }
            }
        }
    }

    if let Some(em) = crate::routes::auth::resolve_login_identifier(state, local_part) {
        let norm = mitch_lib::auth::normalize_email(&em);
        if !norm.is_empty() {
            return norm;
        }
    }

    let profiles_file = state.data_dir().join("profiles.json");
    let profiles = state.store.read_document(&profiles_file, json!({}));
    if let Some(obj) = profiles.as_object() {
        for (email_key, prof_val) in obj {
            if let Some(uname) = prof_val.get("username").and_then(|v| v.as_str()) {
                if uname.eq_ignore_ascii_case(local_part) {
                    return email_key.clone();
                }
            }
        }
    }

    String::new()
}

async fn resolve_sender_info(
    state: &AppState,
    headers: &HeaderMap,
    body_val: Option<&Value>,
) -> (String, String, String) {
    let mut sender_user_id = String::new();
    let mut sender_display_name = String::from("Someone");
    let mut sender_norm_email = String::new();

    if let Some(acc) = resolve_matrix_account(state, headers, body_val).await {
        sender_user_id = acc.user_id;
        sender_norm_email = acc.norm_email;
    }

    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_start_matches("Bearer ").trim().to_string())
        .unwrap_or_default();

    if sender_user_id.is_empty() && !token.is_empty() {
        let map = token_to_account_map()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(acc) = map.get(&token) {
            sender_user_id = acc.user_id.clone();
            if sender_norm_email.is_empty() {
                sender_norm_email = acc.norm_email.clone();
            }
        }
    }

    if sender_norm_email.is_empty() && !sender_user_id.is_empty() {
        sender_norm_email = resolve_member_norm(state, &sender_user_id);
    }

    if !sender_norm_email.is_empty() {
        let profiles_file = state.data_dir().join("profiles.json");
        let profiles = state.store.read_document(&profiles_file, json!({}));
        if let Some(prof) = profiles.get(&sender_norm_email) {
            if let Some(name) = prof
                .get("displayName")
                .or_else(|| prof.get("nickname"))
                .or_else(|| prof.get("username"))
                .and_then(|v| v.as_str())
            {
                if !name.trim().is_empty() {
                    sender_display_name = name.trim().to_string();
                }
            }
        }
        if sender_display_name == "Someone" {
            if let Some(local) = sender_norm_email.split('@').next() {
                if !local.is_empty() {
                    sender_display_name = local.to_string();
                }
            }
        }
    } else if !sender_user_id.is_empty() {
        let local = sender_user_id
            .strip_prefix('@')
            .unwrap_or(&sender_user_id)
            .split(':')
            .next()
            .unwrap_or("");
        if !local.is_empty() {
            sender_display_name = local.to_string();
        }
    }

    (sender_user_id, sender_display_name, sender_norm_email)
}

fn check_user_is_mentioned(member_id: &str, member_norm: &str, parsed: Option<&Value>) -> bool {
    let Some(parsed) = parsed else {
        return false;
    };

    // 1. Matrix MSC3952 m.mentions:
    if let Some(mentions) = parsed.get("m.mentions").and_then(|v| v.as_object()) {
        if mentions.get("room").and_then(|v| v.as_bool()).unwrap_or(false) {
            return true;
        }
        if let Some(user_ids) = mentions.get("user_ids").and_then(|v| v.as_array()) {
            if user_ids
                .iter()
                .any(|u| u.as_str().map(|s| s.eq_ignore_ascii_case(member_id)).unwrap_or(false))
            {
                return true;
            }
        }
    }

    // 2. Check message body text for @username, @member_id, @room, @all, @everyone
    let body = parsed.get("body").and_then(|v| v.as_str()).unwrap_or("");
    let formatted_body = parsed
        .get("formatted_body")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let full_text = format!("{body} {formatted_body}").to_lowercase();

    if full_text.contains("@room")
        || full_text.contains("@all")
        || full_text.contains("@everyone")
    {
        return true;
    }

    let member_lower = member_id.to_lowercase();
    if full_text.contains(&member_lower) {
        return true;
    }

    let username = member_id
        .trim_start_matches('@')
        .split(':')
        .next()
        .unwrap_or("")
        .to_lowercase();
    if !username.is_empty()
        && (full_text.contains(&format!("@{username}"))
            || full_text.contains(&format!("<a href=\"https://matrix.to/#/@{username}:")))
    {
        return true;
    }

    let email_local = member_norm.split('@').next().unwrap_or("").to_lowercase();
    if !email_local.is_empty() && full_text.contains(&format!("@{email_local}")) {
        return true;
    }

    false
}

async fn dispatch_matrix_message_notifications(
    state: &Arc<AppState>,
    room_id: &str,
    event_type: &str,
    body_bytes: &[u8],
    headers: &HeaderMap,
) {
    if event_type != "m.room.message" && event_type != "m.room.encrypted" {
        return;
    }

    let parsed: Option<Value> = serde_json::from_slice(body_bytes).ok();
    let preview_text = if event_type == "m.room.encrypted" {
        "🔒 Encrypted message".to_string()
    } else if let Some(ref p) = parsed {
        match p.get("msgtype").and_then(|v| v.as_str()) {
            Some("m.image") => "📷 Sent an image".to_string(),
            Some("m.file") => "📎 Sent an attachment".to_string(),
            _ => {
                if let Some(b) = p.get("body").and_then(|v| v.as_str()) {
                    let stripped = b.replace("<", "&lt;").replace(">", "&gt;");
                    let trimmed = stripped.trim();
                    let len = trimmed.chars().count().min(120);
                    trimmed.chars().take(len).collect::<String>()
                } else {
                    "New message".to_string()
                }
            }
        }
    } else {
        "New message".to_string()
    };

    let (sender_user_id, sender_display_name, sender_norm_email) =
        resolve_sender_info(state, headers, parsed.as_ref()).await;

    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_start_matches("Bearer ").trim())
        .unwrap_or("");

    let (members, room_name) =
        get_matrix_room_info_for_notifications(room_id, token, &state.id_secret).await;
    if members.is_empty() {
        return;
    }

    let is_direct = members.len() <= 2;
    let room_title = if !room_name.is_empty() {
        room_name
    } else if is_direct {
        String::new()
    } else {
        "General".to_string()
    };

    let notif_title = if is_direct {
        format!("Message from {sender_display_name}")
    } else if !room_title.is_empty() {
        format!("New message from {sender_display_name} in {room_title}")
    } else {
        format!("Message from {sender_display_name}")
    };

    let notif_body = if is_direct {
        "Matrix Direct Message".to_string()
    } else if !room_title.is_empty() {
        format!("Matrix • {room_title}")
    } else {
        "Matrix Group Message".to_string()
    };

    let enc_room = urlencoding_encode(room_id);
    let notif_url = format!("/matrix/#/room/{enc_room}");

    let vapid_public = std::env::var("VAPID_PUBLIC_KEY").unwrap_or_default();
    let subs = if !vapid_public.is_empty() {
        state
            .store
            .read_document(&state.data_dir().join("push_subs.json"), json!({}))
    } else {
        json!({})
    };

    for member_id in &members {
        if !sender_user_id.is_empty() && member_id.eq_ignore_ascii_case(&sender_user_id) {
            continue;
        }

        let member_norm = resolve_member_norm(state, member_id);
        if member_norm.is_empty() || member_norm == sender_norm_email {
            continue;
        }

        let notif_key = if is_direct { "dm" } else { "group" };
        if !crate::routes::dm::notif_allowed(state, &member_norm, notif_key) {
            continue;
        }

        let is_mentioned = check_user_is_mentioned(member_id, &member_norm, parsed.as_ref());
        if !is_direct && !is_mentioned {
            // Notifications are by default OFF unless mentioned (@)
            continue;
        }

        let title_to_use = if is_mentioned && !is_direct {
            format!("{sender_display_name} mentioned you in {room_title}")
        } else {
            notif_title.clone()
        };

        add_matrix_notification(
            state,
            &member_norm,
            &json!({
                "roomId": room_id,
                "type": "matrix",
                "title": title_to_use,
                "body": notif_body,
                "detail": preview_text,
                "sender": sender_display_name,
                "roomTitle": room_title,
                "isDirect": is_direct,
                "url": notif_url,
            }),
        );

        if !vapid_public.is_empty() {
            if let Some(sub) = subs.get(&member_norm) {
                let push_payload = json!({
                    "title": title_to_use,
                    "body": preview_text,
                    "url": notif_url,
                    "tag": format!("matrix-{room_id}"),
                });
                let st = state.clone();
                let vp = vapid_public.clone();
                let mn = member_norm.clone();
                let sb = sub.clone();
                tokio::spawn(async move {
                    let _ =
                        crate::routes::push::send_web_push(&st, &vp, &mn, &sb, &push_payload).await;
                });
            }
        }
    }
}

async fn dispatch_matrix_invite_notifications(
    state: &Arc<AppState>,
    room_id: &str,
    body_bytes: &[u8],
    headers: &HeaderMap,
) {
    let parsed: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
    let target_user_id = parsed.get("user_id").and_then(|v| v.as_str()).unwrap_or("");
    if target_user_id.is_empty() {
        return;
    }

    let (_, sender_display_name, _) = resolve_sender_info(state, headers, Some(&parsed)).await;

    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_start_matches("Bearer ").trim())
        .unwrap_or("");

    let (_, room_name) =
        get_matrix_room_info_for_notifications(room_id, token, &state.id_secret).await;
    let room_title = if !room_name.is_empty() {
        room_name
    } else {
        "a chat room".to_string()
    };

    let member_norm = resolve_member_norm(state, target_user_id);
    if member_norm.is_empty() {
        return;
    }

    if !crate::routes::dm::notif_allowed(state, &member_norm, "dm") {
        return;
    }

    let notif_title = "Chat Room Invite".to_string();
    let notif_body = format!("{sender_display_name} invited you to join {room_title}");
    let enc_room = urlencoding_encode(room_id);
    let notif_url = format!("/matrix/#/room/{enc_room}");

    add_matrix_notification(
        state,
        &member_norm,
        &json!({
            "roomId": room_id,
            "type": "matrix_invite",
            "title": notif_title,
            "body": format!("{sender_display_name} invited you"),
            "detail": format!("Invited you to join room \"{room_title}\""),
            "sender": sender_display_name,
            "roomTitle": room_title,
            "url": notif_url,
        }),
    );

    let vapid_public = std::env::var("VAPID_PUBLIC_KEY").unwrap_or_default();
    if !vapid_public.is_empty() {
        let subs = state
            .store
            .read_document(&state.data_dir().join("push_subs.json"), json!({}));
        if let Some(sub) = subs.get(&member_norm) {
            let push_payload = json!({
                "title": notif_title,
                "body": notif_body,
                "url": notif_url,
                "tag": format!("matrix-invite-{room_id}"),
            });
            let st = state.clone();
            let vp = vapid_public.clone();
            let mn = member_norm.clone();
            let sb = sub.clone();
            tokio::spawn(async move {
                let _ = crate::routes::push::send_web_push(&st, &vp, &mn, &sb, &push_payload).await;
            });
        }
    }

    record_matrix_email_sent(state, &member_norm);
}

async fn dispatch_matrix_call_notifications(
    state: &Arc<AppState>,
    room_id: &str,
    _event_type: &str,
    body_bytes: &[u8],
    headers: &HeaderMap,
) {
    let parsed: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
    if let Some(memberships) = parsed.get("memberships").and_then(|v| v.as_array()) {
        if memberships.is_empty() {
            return;
        }
    }

    let (sender_user_id, sender_display_name, sender_norm_email) =
        resolve_sender_info(state, headers, Some(&parsed)).await;

    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim_start_matches("Bearer ").trim())
        .unwrap_or("");

    let (members, room_name) =
        get_matrix_room_info_for_notifications(room_id, token, &state.id_secret).await;
    if members.is_empty() {
        return;
    }

    let is_direct = members.len() <= 2;
    let room_title = if !room_name.is_empty() {
        room_name
    } else if is_direct {
        String::new()
    } else {
        "General".to_string()
    };

    let notif_title = if is_direct {
        format!("📞 Incoming Call from {sender_display_name}")
    } else if !room_title.is_empty() {
        format!("📞 Call in {room_title}")
    } else {
        format!("📞 Group call from {sender_display_name}")
    };

    let notif_body = if !room_title.is_empty() {
        format!("Incoming voice/video call in {room_title}. Tap to join!")
    } else {
        format!("{sender_display_name} is calling you. Tap to answer!")
    };

    let enc_room = urlencoding_encode(room_id);
    let notif_url = format!("/matrix/#/room/{enc_room}");

    let vapid_public = std::env::var("VAPID_PUBLIC_KEY").unwrap_or_default();
    let subs = if !vapid_public.is_empty() {
        state
            .store
            .read_document(&state.data_dir().join("push_subs.json"), json!({}))
    } else {
        json!({})
    };

    for member_id in &members {
        if !sender_user_id.is_empty() && member_id.eq_ignore_ascii_case(&sender_user_id) {
            continue;
        }

        let member_norm = resolve_member_norm(state, member_id);
        if member_norm.is_empty() || member_norm == sender_norm_email {
            continue;
        }

        if !crate::routes::dm::notif_allowed(
            state,
            &member_norm,
            if is_direct { "dm" } else { "group" },
        ) {
            continue;
        }

        let call_body = if is_direct {
            "Matrix Voice/Video Call".to_string()
        } else if !room_title.is_empty() {
            format!("Group Call in {room_title}")
        } else {
            "Group Call in Chat".to_string()
        };

        add_matrix_notification(
            state,
            &member_norm,
            &json!({
                "roomId": room_id,
                "type": "matrix_call",
                "title": notif_title,
                "body": call_body,
                "detail": format!("{sender_display_name} started a call. Click to join."),
                "sender": sender_display_name,
                "roomTitle": room_title,
                "isDirect": is_direct,
                "url": notif_url,
            }),
        );

        if !vapid_public.is_empty() {
            if let Some(sub) = subs.get(&member_norm) {
                let push_payload = json!({
                    "title": notif_title,
                    "body": notif_body,
                    "url": notif_url,
                    "tag": format!("matrix-call-{room_id}"),
                    "requireInteraction": true,
                    "type": "call",
                });
                let st = state.clone();
                let vp = vapid_public.clone();
                let mn = member_norm.clone();
                let sb = sub.clone();
                tokio::spawn(async move {
                    let _ =
                        crate::routes::push::send_web_push(&st, &vp, &mn, &sb, &push_payload).await;
                });
            }
        }
    }
}

/// Room summary handler for MSC3266 / Nheko Room Summary (`im.nheko.summary`).
pub async fn handle_room_summary(state: &AppState, raw_room_id_or_alias: &str) -> Response {
    let decoded = urlencoding_decode(raw_room_id_or_alias);
    let trimmed = decoded.trim();

    // Check official rooms
    for (alias, name, topic) in OFFICIAL_ROOMS {
        let full_alias = format!("#{alias}:mitch.pro");
        let bare = trimmed.trim_start_matches('#').split(':').next().unwrap_or("");
        let matches = trimmed.eq_ignore_ascii_case(&full_alias)
            || trimmed.eq_ignore_ascii_case(alias)
            || bare.eq_ignore_ascii_case(alias);

        if matches {
            let room_id = ensure_official_room(&state.id_secret, alias, name, topic)
                .await
                .unwrap_or_else(|_| format!("!{alias}:mitch.pro"));
            return cors_json_response(
                200,
                json!({
                    "room_id": room_id,
                    "name": name,
                    "topic": topic,
                    "avatar_url": null,
                    "canonical_alias": full_alias,
                    "aliases": [full_alias],
                    "world_readable": true,
                    "guest_can_join": true,
                    "num_joined_members": 1,
                    "joined_member_count": 1,
                    "membership": "leave",
                    "room_type": null,
                    "encryption": null,
                    "visibility": "public"
                }),
            );
        }
    }

    // Check official rooms by cached room_id
    {
        let cache = official_room_id_map().lock().unwrap_or_else(|e| e.into_inner());
        for (alias, name, topic) in OFFICIAL_ROOMS {
            if let Some(cached_id) = cache.get(*alias) {
                if cached_id == trimmed {
                    let full_alias = format!("#{alias}:mitch.pro");
                    return cors_json_response(
                        200,
                        json!({
                            "room_id": trimmed,
                            "name": name,
                            "topic": topic,
                            "avatar_url": null,
                            "canonical_alias": full_alias,
                            "aliases": [full_alias],
                            "world_readable": true,
                            "guest_can_join": true,
                            "num_joined_members": 1,
                            "joined_member_count": 1,
                            "membership": "leave",
                            "room_type": null,
                            "encryption": null,
                            "visibility": "public"
                        }),
                    );
                }
            }
        }
    }

    // Fall back to querying Conduit's directory if it looks like an alias
    if trimmed.starts_with('#') {
        let encoded = url::form_urlencoded::byte_serialize(trimmed.as_bytes()).collect::<String>();
        if let Ok((status, _, bytes)) = call_conduit(
            &format!("/_matrix/client/v3/directory/room/{encoded}"),
            Method::GET,
            None,
            None,
        )
        .await
        {
            if status.is_success() {
                let data: Value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
                if let Some(room_id) = data.get("room_id").and_then(|v| v.as_str()) {
                    return cors_json_response(
                        200,
                        json!({
                            "room_id": room_id,
                            "canonical_alias": trimmed,
                            "aliases": [trimmed],
                            "world_readable": true,
                            "guest_can_join": true,
                            "num_joined_members": 1,
                            "joined_member_count": 1,
                            "membership": "leave",
                            "room_type": null,
                            "encryption": null,
                            "visibility": "public"
                        }),
                    );
                }
            }
        }
    }

    cors_json_response(
        404,
        json!({
            "errcode": "M_NOT_FOUND",
            "error": "Room not found"
        }),
    )
}

/// Gateway dispatcher for Matrix paths: `/_matrix/*`, `/.well-known/matrix/*`, `/matrix/config.json`.
pub async fn handle_matrix_gateway(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    search: &str,
    body_bytes: &[u8],
) -> Option<Response> {
    if method == Method::OPTIONS {
        return Some(cors_response(StatusCode::NO_CONTENT, Bytes::new(), None));
    }

    if path == "/matrix/config.json" && method == Method::GET {
        return Some(handle_cinny_config(headers));
    }

    if (path == "/matrix/public/element-call/config.json"
        || path == "/matrix/public/element-call/config.json/")
        && method == Method::GET
    {
        return Some(handle_element_call_config(headers));
    }

    if path.starts_with("/.well-known/matrix/") {
        return handle_well_known(method, path, headers);
    }

    if !path.starts_with("/_matrix/") {
        return None;
    }

    // MSC3266 / Nheko Room Summary
    static ROOM_SUMMARY_RE: OnceLock<regex::Regex> = OnceLock::new();
    let room_summary_re = ROOM_SUMMARY_RE.get_or_init(|| {
        regex::Regex::new(
            r"^/_matrix/client/(?:unstable/im\.nheko\.summary|unstable/org\.matrix\.msc3266|v1)/(?:summary/([^/?]+)|rooms/([^/?]+)/summary)/?$",
        )
        .unwrap_or_else(|_| unreachable_regex())
    });
    if method == Method::GET {
        if let Some(caps) = room_summary_re.captures(path) {
            let room_param = caps
                .get(1)
                .or_else(|| caps.get(2))
                .map(|m| m.as_str())
                .unwrap_or("");
            return Some(handle_room_summary(state, room_param).await);
        }
    }

    // MSC2965 OIDC Discovery endpoint stub
    static MSC2965_RE: OnceLock<regex::Regex> = OnceLock::new();
    let msc2965_re = MSC2965_RE.get_or_init(|| {
        regex::Regex::new(
            r"^/_matrix/client/unstable/org\.matrix\.msc2965/(?:auth_metadata|auth_issuer)/?$",
        )
        .unwrap_or_else(|_| unreachable_regex())
    });
    if method == Method::GET && msc2965_re.is_match(path) {
        return Some(cors_json_response(
            404,
            json!({
                "errcode": "M_UNRECOGNIZED",
                "error": "OIDC is not supported"
            }),
        ));
    }

    // Room keys backup version fallback
    if method == Method::GET
        && (path == "/_matrix/client/v3/room_keys/version"
            || path == "/_matrix/client/v3/room_keys/version/")
    {
        return Some(cors_json_response(
            404,
            json!({
                "errcode": "M_NOT_FOUND",
                "error": "No current key backup version"
            }),
        ));
    }

    // VoIP STUN/TURN Discovery
    static TURN_RE: OnceLock<regex::Regex> = OnceLock::new();
    let turn_re = TURN_RE.get_or_init(|| {
        regex::Regex::new(r"^/_matrix/client/(?:v3|r0)/voip/turnServer")
            .unwrap_or_else(|_| unreachable_regex())
    });
    if method == Method::GET && turn_re.is_match(path) {
        return Some(cors_json_response(
            200,
            json!({
                "uris": [
                    "stun:stun.l.google.com:19302",
                    "stun:stun1.l.google.com:19302",
                    "stun:stun2.l.google.com:19302",
                    "stun:stun.cloudflare.com:3478",
                    "stun:stun.matrix.org:3478"
                ],
                "ttl": 86400
            }),
        ));
    }

    // Empty notifications fallback for Conduit
    static NOTIF_RE: OnceLock<regex::Regex> = OnceLock::new();
    let notif_re = NOTIF_RE.get_or_init(|| {
        regex::Regex::new(r"^/_matrix/client/(?:v3|r0)/notifications")
            .unwrap_or_else(|_| unreachable_regex())
    });
    if method == Method::GET && notif_re.is_match(path) {
        return Some(cors_json_response(200, json!({ "notifications": [] })));
    }

    // Direct bio updates via MSC1769 account_data
    static BIO_PUT_RE: OnceLock<regex::Regex> = OnceLock::new();
    let bio_put_re = BIO_PUT_RE.get_or_init(|| {
        regex::Regex::new(
            r"^/_matrix/client/(?:v3|r0)/user/([^/]+)/account_data/org\.matrix\.msc1769\.custom_profile_fields/?$",
        )
        .expect("static regex")
    });
    if method == Method::PUT {
        if let Some(caps) = bio_put_re.captures(path) {
            let target_user_id = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let decoded_user_id = urlencoding_decode(target_user_id);
            if let Ok(body) = serde_json::from_slice::<Value>(body_bytes) {
                if let Some(bio_val) = body.get("bio").and_then(|v| v.as_str()) {
                    if let Some(norm) =
                        find_profile_email_by_matrix_user_id(state, &decoded_user_id)
                    {
                        let profiles_file = state.data_dir().join("profiles.json");
                        let mut profiles = state.store.read_document(&profiles_file, json!({}));
                        if let Some(prof) = profiles.get_mut(&norm).and_then(|v| v.as_object_mut())
                        {
                            let bio_clean = &bio_val.trim()[..bio_val.trim().len().min(300)];
                            prof.insert("bio".to_string(), json!(bio_clean));
                            let now = now_millis();
                            prof.insert("updatedAt".to_string(), json!(now));
                            let uname = prof
                                .get("username")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let _ = state.store.write_document(&profiles_file, &profiles);
                            crate::ws::broadcast_profile_change(state, &uname, now);
                        }
                    }
                }
            }
            return Some(cors_json_response(200, json!({})));
        }
    }

    // Intercept readable-message policy before forwarding to homeserver
    static POLICY_SEND_RE: OnceLock<regex::Regex> = OnceLock::new();
    let policy_send_re = POLICY_SEND_RE.get_or_init(|| {
        regex::Regex::new(
            r"^/_matrix/client/(?:v3|r0|v1|unstable)/rooms/[^/]+/send/([^/]+)(?:/[^/]+)?$",
        )
        .unwrap_or_else(|_| unreachable_regex())
    });
    if (method == Method::PUT || method == Method::POST) && !body_bytes.is_empty() {
        if let Some(caps) = policy_send_re.captures(path) {
            let event_type = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            if event_type == "m.room.message" {
                if let Ok(content) = serde_json::from_slice::<Value>(body_bytes) {
                    if mitch_lib::matrix::matrix_message_blocked(&content) {
                        return Some(cors_json_response(
                            403,
                            json!({
                                "errcode": "M_FORBIDDEN",
                                "error": "Your message contains a word or phrase that is not allowed in this chat. Please edit it and try again."
                            }),
                        ));
                    }
                }
            }
        }
    }

    // Intercept client-side chat reports to feed into Mitch.pro Safety & Moderation
    static REPORT_RE: OnceLock<regex::Regex> = OnceLock::new();
    let report_re = REPORT_RE.get_or_init(|| {
        regex::Regex::new(r"^/_matrix/client/(?:v3|r0)/rooms/([^/]+)/report(?:/([^/]+))?$")
            .unwrap_or_else(|_| unreachable_regex())
    });
    if method == Method::POST {
        if let Some(caps) = report_re.captures(path) {
            let room_id = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let event_id = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let parsed: Value = serde_json::from_slice(body_bytes).unwrap_or(json!({}));
            let reason =
                parsed
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or(if event_id.is_empty() {
                        "Reported chat without entering"
                    } else {
                        "Reported message"
                    });

            let cookies = crate::routes::me::cookies_of(state, headers);
            let sid = cookies
                .get("studentId")
                .filter(|s| !s.is_empty())
                .or_else(|| cookies.get("id"))
                .unwrap_or("");
            let mut reporter = if !sid.is_empty() {
                mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, sid)
                    .unwrap_or_else(|| sid.to_string())
            } else {
                String::new()
            };
            if reporter.is_empty() {
                reporter = "matrix-user".to_string();
            }

            let clean_id = if !event_id.is_empty() {
                event_id.to_string()
            } else {
                format!("room-{}", now_millis())
            };
            let report_entry = json!({
                "id": format!("matrix-{clean_id}"),
                "reason": format!("[Matrix Room {room_id}] {reason}"),
                "reportedBy": reporter,
                "ts": now_millis(),
                "status": "Needs review",
                "matrixRoomId": room_id,
                "matrixEventId": event_id,
                "matrixSender": "unknown",
                "reportedWithoutEntering": event_id.is_empty(),
                "context": [
                    {
                        "from": "unknown",
                        "to": room_id,
                        "text": format!("Reported: {reason}"),
                        "ts": now_millis(),
                        "reported": true
                    }
                ]
            });

            let reports_file = state.data_dir().join("chat_reports.json");
            let mut reports = state.store.read_document(&reports_file, json!([]));
            if let Some(arr) = reports.as_array_mut() {
                arr.push(report_entry);
                if arr.len() > 5000 {
                    let excess = arr.len() - 5000;
                    arr.drain(0..excess);
                }
                let _ = state.store.write_document(&reports_file, &reports);
            }

            if event_id.is_empty() {
                return Some(cors_json_response(200, json!({})));
            }
        }
    }

    // Enforce slowmode, room lockdown, and user mute on send
    static SEND_RE: OnceLock<regex::Regex> = OnceLock::new();
    let send_re = SEND_RE.get_or_init(|| {
        regex::Regex::new(r"^/_matrix/client/(?:v3|r0)/rooms/([^/]+)/send/([^/]+)(?:/([^/]+))?$")
            .unwrap_or_else(|_| unreachable_regex())
    });
    let mut send_match_room_id = String::new();
    let mut send_match_sender_key = String::new();
    let mut is_chat_send_event = false;

    if method == Method::PUT || method == Method::POST {
        if let Some(caps) = send_re.captures(path) {
            let raw_room_id = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let target_room_id = urlencoding_decode(raw_room_id);
            let event_type = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            is_chat_send_event = event_type == "m.room.message"
                || event_type == "m.room.encrypted"
                || event_type == "m.reaction"
                || event_type == "m.sticker"
                || event_type.starts_with("org.matrix.msc2677.reaction");

            if is_chat_send_event {
                send_match_room_id = target_room_id.clone();
                let parsed_payload: Option<Value> = serde_json::from_slice(body_bytes).ok();
                let sender_account =
                    resolve_matrix_account(state, headers, parsed_payload.as_ref()).await;

                let is_staff = is_matrix_staff_member(state, headers, sender_account.as_ref());
                if !is_staff {
                    let room_settings = load_matrix_room_settings(state, &target_room_id);
                    if room_settings.get("roomMuted").and_then(|v| v.as_bool()) == Some(true) {
                        return Some(cors_json_response(
                            403,
                            json!({
                                "errcode": "M_FORBIDDEN",
                                "error": "This room is currently in lockdown mode. Only administrators and moderators may speak."
                            }),
                        ));
                    }

                    let mut sender_ids = Vec::new();
                    if let Some(ref acc) = sender_account {
                        if !acc.user_id.is_empty() {
                            sender_ids.push(acc.user_id.clone());
                        }
                        if !acc.norm_email.is_empty() {
                            sender_ids.push(acc.norm_email.clone());
                        }
                        if !acc.uid.is_empty() {
                            sender_ids.push(acc.uid.clone());
                        }
                    }

                    if let Some(mute) =
                        is_user_muted_in_matrix_room(state, &target_room_id, &sender_ids)
                    {
                        let reason = mute.get("reason").and_then(|v| v.as_str()).unwrap_or("");
                        let reason_part = if !reason.is_empty() {
                            format!(": {reason}")
                        } else {
                            String::new()
                        };
                        return Some(cors_json_response(
                            403,
                            json!({
                                "errcode": "M_FORBIDDEN",
                                "error": format!("You are muted in this room{reason_part}.")
                            }),
                        ));
                    }

                    let mut effective_slowmode = room_settings
                        .get("slowmodeSeconds")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);

                    if let Some(user_slowmodes) =
                        room_settings.get("userSlowmode").and_then(|v| v.as_object())
                    {
                        for sid in &sender_ids {
                            if let Some(sec) = user_slowmodes.get(sid).and_then(|v| v.as_i64()) {
                                effective_slowmode = std::cmp::max(effective_slowmode, sec);
                            }
                            let uname = sid.trim_start_matches('@').split(':').next().unwrap_or("");
                            if let Some(sec) = user_slowmodes.get(uname).and_then(|v| v.as_i64()) {
                                effective_slowmode = std::cmp::max(effective_slowmode, sec);
                            }
                        }
                    }

                    if effective_slowmode > 0 {
                        let s_key = sender_ids
                            .first()
                            .cloned()
                            .unwrap_or_else(|| "anonymous".to_string());
                        let wait_sec =
                            check_matrix_slowmode(&target_room_id, &s_key, effective_slowmode);
                        if wait_sec > 0 {
                            let mut resp = cors_json_response(
                                429,
                                json!({
                                    "errcode": "M_LIMIT_EXCEEDED",
                                    "error": format!("Slowmode is enabled ({effective_slowmode}s). Please wait {wait_sec}s before sending another message."),
                                    "retry_after_ms": wait_sec * 1000
                                }),
                            );
                            if let Ok(hv) = HeaderValue::from_str(&wait_sec.to_string()) {
                                resp.headers_mut().insert("Retry-After", hv);
                            }
                            return Some(resp);
                        }
                    }
                }

                send_match_sender_key = sender_account
                    .as_ref()
                    .map(|a| a.user_id.clone())
                    .unwrap_or_else(|| "anon".to_string());
            }
        }
    }

    // Forward upstream to Conduit
    let full_path = if search.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{search}")
    };

    let (body_changed, translated_body) = if !body_bytes.is_empty() {
        translate_matrix_password(state, headers, path, body_bytes).await
    } else {
        (false, Vec::new())
    };

    let body_opt = if body_changed {
        Some(Bytes::from(translated_body))
    } else if body_bytes.is_empty() {
        None
    } else {
        Some(Bytes::copy_from_slice(body_bytes))
    };

    let conduit_res = call_conduit_with_timeout(
        &full_path,
        method.clone(),
        Some(headers.clone()),
        body_opt,
        Some(std::time::Duration::from_secs(300)),
    )
    .await;

    match conduit_res {
        Ok((status, upstream_headers, bytes)) => {
            // Track active user in Matrix for presence
            if let Some(acc) = resolve_matrix_account(state, headers, None).await {
                if !acc.norm_email.is_empty() {
                    let mut seen = state
                        .matrix_user_last_seen
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    seen.insert(acc.norm_email.clone(), now_millis());
                    crate::ws::touch_user_presence(state, &acc.norm_email, "Chatting in Matrix");
                }
            }

            // Cache access token from successful login responses
            static LOGIN_RE: OnceLock<regex::Regex> = OnceLock::new();
            let login_re = LOGIN_RE.get_or_init(|| {
                regex::Regex::new(r"^/_matrix/client/(?:v3|r0)/login").expect("static regex")
            });
            if method == Method::POST && status.is_success() && login_re.is_match(path) {
                if let Ok(login_data) = serde_json::from_slice::<Value>(&bytes) {
                    if let Some(token) = login_data.get("access_token").and_then(|v| v.as_str()) {
                        let user_id = login_data
                            .get("user_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let username = if let Some(stripped) = user_id.strip_prefix('@') {
                            stripped.split(':').next().unwrap_or(stripped)
                        } else {
                            user_id
                        };
                        let norm_email =
                            crate::routes::auth::resolve_login_identifier(state, username)
                                .or_else(|| find_profile_email_by_matrix_user_id(state, user_id))
                                .unwrap_or_default();
                        let uid = if !norm_email.is_empty() {
                            mitch_lib::auth::make_email_id(&norm_email, 0, &state.id_secret)
                        } else {
                            String::new()
                        };
                        let mut map = token_to_account_map()
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        map.insert(
                            token.to_string(),
                            MatrixAccount {
                                uid,
                                norm_email,
                                user_id: user_id.to_string(),
                            },
                        );
                    }
                }
            }

            if status.is_success() && is_chat_send_event && !send_match_room_id.is_empty() {
                record_matrix_message_sent(&send_match_room_id, &send_match_sender_key);
            }

            if status.is_success() && (*method == Method::PUT || *method == Method::POST) {
                if let Some(caps) = send_re.captures(path) {
                    let room_id = urlencoding_decode(caps.get(1).map(|m| m.as_str()).unwrap_or(""));
                    let event_type = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                    if event_type == "m.call.invite" {
                        dispatch_matrix_call_notifications(
                            state, &room_id, event_type, body_bytes, headers,
                        )
                        .await;
                    } else if event_type == "m.room.message" || event_type == "m.room.encrypted" {
                        dispatch_matrix_message_notifications(
                            state, &room_id, event_type, body_bytes, headers,
                        )
                        .await;
                    }
                } else {
                    static INVITE_RE: OnceLock<regex::Regex> = OnceLock::new();
                    let invite_re = INVITE_RE.get_or_init(|| {
                        regex::Regex::new(r"^/_matrix/client/(?:v3|r0)/rooms/([^/]+)/invite/?$")
                            .expect("static regex")
                    });
                    if let Some(caps) = invite_re.captures(path) {
                        let room_id =
                            urlencoding_decode(caps.get(1).map(|m| m.as_str()).unwrap_or(""));
                        dispatch_matrix_invite_notifications(state, &room_id, body_bytes, headers)
                            .await;
                    } else {
                        static STATE_RE: OnceLock<regex::Regex> = OnceLock::new();
                        let state_re = STATE_RE.get_or_init(|| {
                            regex::Regex::new(
                                r"^/_matrix/client/(?:v3|r0)/rooms/([^/]+)/state/([^/]+)",
                            )
                            .expect("static regex")
                        });
                        if let Some(caps) = state_re.captures(path) {
                            let room_id =
                                urlencoding_decode(caps.get(1).map(|m| m.as_str()).unwrap_or(""));
                            let event_type = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                            if event_type == "m.call.member"
                                || event_type == "org.matrix.msc3401.call.member"
                            {
                                dispatch_matrix_call_notifications(
                                    state, &room_id, event_type, body_bytes, headers,
                                )
                                .await;
                            }
                        }
                    }
                }
            }

            // Matrix -> Mitch.pro Profile Sync on successful PUT
            if method == Method::PUT && status.is_success() && !body_bytes.is_empty() {
                static PROFILE_DISPLAY_RE: OnceLock<regex::Regex> = OnceLock::new();
                let profile_display_re = PROFILE_DISPLAY_RE.get_or_init(|| {
                    regex::Regex::new(r"^/_matrix/client/(?:v3|r0)/profile/([^/]+)/displayname/?$")
                        .expect("static regex")
                });
                static PROFILE_AVATAR_RE: OnceLock<regex::Regex> = OnceLock::new();
                let profile_avatar_re = PROFILE_AVATAR_RE.get_or_init(|| {
                    regex::Regex::new(r"^/_matrix/client/(?:v3|r0)/profile/([^/]+)/avatar_url/?$")
                        .expect("static regex")
                });
                static PRESENCE_STATUS_RE: OnceLock<regex::Regex> = OnceLock::new();
                let presence_status_re = PRESENCE_STATUS_RE.get_or_init(|| {
                    regex::Regex::new(r"^/_matrix/client/(?:v3|r0)/presence/([^/]+)/status/?$")
                        .expect("static regex")
                });

                if let Some(caps) = profile_display_re.captures(path) {
                    let target_user_id = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    let decoded_user_id = urlencoding_decode(target_user_id);
                    if let Ok(body) = serde_json::from_slice::<Value>(body_bytes) {
                        if let Some(displayname_val) =
                            body.get("displayname").and_then(|v| v.as_str())
                        {
                            if let Some(norm) =
                                find_profile_email_by_matrix_user_id(state, &decoded_user_id)
                            {
                                let profiles_file = state.data_dir().join("profiles.json");
                                let mut profiles =
                                    state.store.read_document(&profiles_file, json!({}));
                                if let Some(prof) =
                                    profiles.get_mut(&norm).and_then(|v| v.as_object_mut())
                                {
                                    let dn_clean = &displayname_val.trim()
                                        [..displayname_val.trim().len().min(40)];
                                    prof.insert("displayName".to_string(), json!(dn_clean));
                                    let now = now_millis();
                                    prof.insert("updatedAt".to_string(), json!(now));
                                    let uname = prof
                                        .get("username")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let _ = state.store.write_document(&profiles_file, &profiles);
                                    crate::ws::broadcast_profile_change(state, &uname, now);
                                }
                            }
                        }
                    }
                } else if let Some(caps) = profile_avatar_re.captures(path) {
                    let target_user_id = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    let decoded_user_id = urlencoding_decode(target_user_id);
                    if let Ok(body) = serde_json::from_slice::<Value>(body_bytes) {
                        if let Some(norm) =
                            find_profile_email_by_matrix_user_id(state, &decoded_user_id)
                        {
                            let raw_url = body
                                .get("avatar_url")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let pfp_val = if let Some(stripped) = raw_url.strip_prefix("mxc://") {
                                format!("/_matrix/media/v3/download/{stripped}")
                            } else {
                                String::new()
                            };
                            let profiles_file = state.data_dir().join("profiles.json");
                            let mut profiles = state.store.read_document(&profiles_file, json!({}));
                            if let Some(prof) =
                                profiles.get_mut(&norm).and_then(|v| v.as_object_mut())
                            {
                                prof.insert("pfp".to_string(), json!(pfp_val));
                                let now = now_millis();
                                prof.insert("updatedAt".to_string(), json!(now));
                                let uname = prof
                                    .get("username")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let _ = state.store.write_document(&profiles_file, &profiles);
                                crate::ws::broadcast_profile_change(state, &uname, now);
                            }
                        }
                    }
                } else if let Some(caps) = presence_status_re.captures(path) {
                    let target_user_id = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    let decoded_user_id = urlencoding_decode(target_user_id);
                    if let Ok(body) = serde_json::from_slice::<Value>(body_bytes) {
                        if let Some(status_msg) = body.get("status_msg").and_then(|v| v.as_str()) {
                            if let Some(norm) =
                                find_profile_email_by_matrix_user_id(state, &decoded_user_id)
                            {
                                let profiles_file = state.data_dir().join("profiles.json");
                                let mut profiles =
                                    state.store.read_document(&profiles_file, json!({}));
                                if let Some(prof) =
                                    profiles.get_mut(&norm).and_then(|v| v.as_object_mut())
                                {
                                    let bio_clean =
                                        &status_msg.trim()[..status_msg.trim().len().min(300)];
                                    prof.insert("bio".to_string(), json!(bio_clean));
                                    let now = now_millis();
                                    prof.insert("updatedAt".to_string(), json!(now));
                                    let uname = prof
                                        .get("username")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let _ = state.store.write_document(&profiles_file, &profiles);
                                    crate::ws::broadcast_profile_change(state, &uname, now);
                                }
                            }
                        }
                    }
                }
            }

            // Matrix profile GET hook: augment with bio and status_msg
            if method == Method::GET && status.is_success() {
                static PROFILE_GET_RE: OnceLock<regex::Regex> = OnceLock::new();
                let profile_get_re = PROFILE_GET_RE.get_or_init(|| {
                    regex::Regex::new(r"^/_matrix/client/(?:v3|r0)/profile/([^/]+)$")
                        .expect("static regex")
                });
                if let Some(caps) = profile_get_re.captures(path) {
                    let target_user_id = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                    let decoded_user_id = urlencoding_decode(target_user_id);
                    if let Some(norm) =
                        find_profile_email_by_matrix_user_id(state, &decoded_user_id)
                    {
                        let profiles_file = state.data_dir().join("profiles.json");
                        let profiles = state.store.read_document(&profiles_file, json!({}));
                        if let Some(prof) = profiles.get(&norm) {
                            if let Some(bio) = prof.get("bio").and_then(|v| v.as_str()) {
                                if !bio.is_empty() {
                                    if let Ok(mut profile_data) =
                                        serde_json::from_slice::<Value>(&bytes)
                                    {
                                        if let Some(obj) = profile_data.as_object_mut() {
                                            if !obj.contains_key("bio") {
                                                obj.insert("bio".to_string(), json!(bio));
                                            }
                                            if !obj.contains_key("status_msg") {
                                                obj.insert("status_msg".to_string(), json!(bio));
                                            }
                                            return Some(cors_json_response(200, profile_data));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            Some(proxy_response_with_cors(status, &upstream_headers, bytes))
        }
        Err(err) => {
            if method == Method::GET && path.starts_with("/_matrix/media/") {
                return Some(cors_response(StatusCode::NOT_FOUND, Bytes::new(), Some("text/plain")));
            }
            Some(cors_json_response(
                502,
                json!({
                    "error": "Matrix chat backend unavailable",
                    "details": err
                }),
            ))
        }
    }
}

/// Recursively sums file sizes and counts under `path` — used to report the
/// Conduit RocksDB/SQLite data directory size. Best-effort: unreadable
/// entries are skipped rather than failing the whole health check.
fn dir_size_and_count(path: &std::path::Path) -> (u64, u64) {
    let mut total = 0u64;
    let mut count = 0u64;
    let Ok(entries) = std::fs::read_dir(path) else {
        return (0, 0);
    };
    for entry in entries.flatten() {
        let entry_path = entry.path();
        if entry_path.is_dir() {
            let (sub_total, sub_count) = dir_size_and_count(&entry_path);
            total += sub_total;
            count += sub_count;
        } else if let Ok(meta) = entry.metadata() {
            total += meta.len();
            count += 1;
        }
    }
    (total, count)
}

/// `GET /api/admin/matrix/health` — a lightweight Conduit diagnostics
/// widget: reachability + round-trip latency via an unauthenticated
/// `/_matrix/client/versions` call, the on-disk size of Conduit's data
/// directory (shared with this container via the `./data/conduit` volume —
/// see docker-compose.yml), and how many rooms this app is actively
/// tracking settings for. Conduit does not expose a federation-queue or
/// live-connection-count admin API, so those are intentionally omitted
/// rather than faked.
pub async fn admin_conduit_health(state: &Arc<AppState>) -> Response {
    let started = std::time::Instant::now();
    let reachable = matches!(
        call_conduit_with_timeout(
            "/_matrix/client/versions",
            Method::GET,
            None,
            None,
            Some(std::time::Duration::from_secs(5)),
        )
        .await,
        Ok((status, _, _)) if status.is_success()
    );
    let latency_ms = started.elapsed().as_millis() as u64;

    let db_dir = state.cfg.data_dir.join("conduit");
    let (db_size_bytes, db_file_count) = dir_size_and_count(&db_dir);

    let settings_file = state.cfg.data_dir.join("matrix_room_settings.json");
    let settings_all = state.store.read_document(&settings_file, json!({}));
    let tracked_rooms = settings_all.as_object().map(|m| m.len()).unwrap_or(0);

    crate::errors::json_resp(
        200,
        json!({
            "reachable": reachable,
            "latencyMs": latency_ms,
            "dbSizeBytes": db_size_bytes,
            "dbFileCount": db_file_count,
            "trackedRooms": tracked_rooms,
            "officialRoomCount": OFFICIAL_ROOMS.len(),
            "federationAllowed": std::env::var("CONDUIT_ALLOW_FEDERATION")
                .map(|v| v == "true")
                .unwrap_or(false),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_matrix_device_id() {
        assert_eq!(
            sanitize_matrix_device_id("ABC_123.test~foo-bar"),
            "ABC_123.test~foo-bar"
        );
        assert_eq!(
            sanitize_matrix_device_id("invalid device id with spaces!"),
            ""
        );
        assert_eq!(sanitize_matrix_device_id(""), "");
    }

    #[test]
    fn test_matrix_password_generation() {
        let p1 = get_matrix_password_for_uid("test-user-123", b"secret-salt");
        let p2 = get_matrix_password_for_uid("test-user-123", b"secret-salt");
        assert_eq!(p1, p2);
        assert!(!p1.is_empty());

        let p3 = get_matrix_password_for_uid("test-user-456", b"secret-salt");
        assert_ne!(p1, p3);
    }

    #[test]
    fn test_slowmode_tracking() {
        let room = "test_room_1";
        let sender = "user_abc";
        assert_eq!(check_matrix_slowmode(room, sender, 10), 0);

        record_matrix_message_sent(room, sender);
        let wait = check_matrix_slowmode(room, sender, 10);
        assert!(wait > 0 && wait <= 10);

        let other_sender = "user_def";
        assert_eq!(check_matrix_slowmode(room, other_sender, 10), 0);
    }

    #[test]
    fn test_cinny_config() {
        let headers = HeaderMap::new();
        let resp = handle_cinny_config(&headers);
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn test_well_known_client_and_server() {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("mitch.pro"));

        let resp_client = handle_well_known(&Method::GET, "/.well-known/matrix/client", &headers);
        assert!(resp_client.is_some());
        assert_eq!(resp_client.unwrap().status(), StatusCode::OK);

        let resp_server = handle_well_known(&Method::GET, "/.well-known/matrix/server", &headers);
        assert!(resp_server.is_some());
        assert_eq!(resp_server.unwrap().status(), StatusCode::OK);

        let resp_opt = handle_well_known(&Method::OPTIONS, "/.well-known/matrix/client", &headers);
        assert!(resp_opt.is_some());
        assert_eq!(resp_opt.unwrap().status(), StatusCode::NO_CONTENT);
    }

    #[test]
    fn test_official_rooms_include_default_channels() {
        let aliases: Vec<&str> = OFFICIAL_ROOMS.iter().map(|(a, _, _)| *a).collect();
        assert!(aliases.contains(&"general"));
        assert!(aliases.contains(&"python-hate"));
        assert!(aliases.contains(&"rust"));
        assert!(aliases.contains(&"memory-safe"));
    }

    fn test_state() -> (Arc<AppState>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "mitch-server-matrix-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap_or_default();
        let cfg = crate::hosts::SiteConfig::load();
        let cfg = crate::hosts::SiteConfig {
            data_dir: dir.join("data"),
            ..cfg
        };
        let store = Arc::new(
            mitch_lib::data::DataStore::open(&dir, &dir.join("data"))
                .unwrap_or_else(|e| panic!("store: {e}")),
        );
        (Arc::new(AppState::new(cfg, Arc::clone(&store))), dir)
    }

    #[tokio::test]
    async fn test_matrix_profile_sync_helpers() {
        let (state, _dir) = test_state();

        let email = "syncuser@student.rjuhsd.us";
        let norm = mitch_lib::auth::normalize_email(email);
        let uid = mitch_lib::auth::make_email_id(email, 0, &state.id_secret);

        let profiles_file = state.data_dir().join("profiles.json");
        let profiles = json!({
            &norm: {
                "username": "syncuser",
                "displayName": "Sync User",
                "bio": "Hello from mitch.pro",
                "pfp": "/images/avatar.png"
            }
        });
        let _ = state.store.write_document(&profiles_file, &profiles);

        let matrix_users_file = state.data_dir().join("matrix_users.json");
        let matrix_users = json!({
            &uid: "syncuser"
        });
        let _ = state
            .store
            .write_document(&matrix_users_file, &matrix_users);

        // 1. Resolve by @syncuser:mitch.pro
        assert_eq!(
            find_profile_email_by_matrix_user_id(&state, "@syncuser:mitch.pro"),
            Some(norm.clone())
        );
        // 2. Resolve by username directly
        assert_eq!(
            find_profile_email_by_matrix_user_id(&state, "syncuser"),
            Some(norm.clone())
        );

        // 3. Test direct bio PUT via MSC1769 endpoint
        let bio_body = json!({ "bio": "New bio from Matrix client" });
        let bio_path = "/_matrix/client/v3/user/%40syncuser%3Amitch.pro/account_data/org.matrix.msc1769.custom_profile_fields";
        let resp = handle_matrix_gateway(
            &state,
            &Method::PUT,
            bio_path,
            &HeaderMap::new(),
            "",
            &serde_json::to_vec(&bio_body).unwrap(),
        )
        .await;
        assert!(resp.is_some());
        assert_eq!(resp.unwrap().status(), StatusCode::OK);

        // Verify bio was updated in profiles.json
        let updated_profiles = state.store.read_document(&profiles_file, json!({}));
        let prof = updated_profiles.get(&norm).unwrap();
        assert_eq!(
            prof.get("bio").unwrap().as_str().unwrap(),
            "New bio from Matrix client"
        );
    }

    #[tokio::test]
    async fn test_matrix_gifs_and_stickers() {
        let (state, _dir) = test_state();
        let headers = HeaderMap::new();

        // 1. Trending GIFs
        let resp = handle_api(
            &state,
            &Method::GET,
            "/api/matrix/gifs/trending",
            &headers,
            "",
            &[],
        )
        .await;
        assert!(resp.is_some());
        let r = resp.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
        let val: Value = serde_json::from_slice(&body_bytes).unwrap();
        let results = val.get("results").unwrap().as_array().unwrap();
        assert!(!results.is_empty());
        assert!(results[0].get("id").is_some());
        assert!(results[0].get("url").is_some());

        // 2. Search GIFs
        let resp = handle_api(
            &state,
            &Method::GET,
            "/api/matrix/gifs/search",
            &headers,
            "?q=cat",
            &[],
        )
        .await;
        assert!(resp.is_some());
        let r = resp.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
        let val: Value = serde_json::from_slice(&body_bytes).unwrap();
        let results = val.get("results").unwrap().as_array().unwrap();
        assert!(!results.is_empty());

        // 3. Sticker packs
        let resp = handle_api(
            &state,
            &Method::GET,
            "/api/matrix/stickers/packs",
            &headers,
            "",
            &[],
        )
        .await;
        assert!(resp.is_some());
        let r = resp.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
        let val: Value = serde_json::from_slice(&body_bytes).unwrap();
        let packs = val.get("packs").unwrap().as_array().unwrap();
        assert!(!packs.is_empty());
        assert!(packs.iter().any(|p| p.get("id").unwrap() == "pepe"));
    }

    #[test]
    fn test_check_user_is_mentioned() {
        let content_msc = json!({
            "m.mentions": {
                "user_ids": ["@target:mitch.pro"]
            },
            "body": "Hello there"
        });
        assert!(check_user_is_mentioned("@target:mitch.pro", "target", Some(&content_msc)));
        assert!(!check_user_is_mentioned("@other:mitch.pro", "other", Some(&content_msc)));

        let content_body = json!({
            "body": "Hey @target, how are you?"
        });
        assert!(check_user_is_mentioned("@target:mitch.pro", "target", Some(&content_body)));
        assert!(!check_user_is_mentioned("@other:mitch.pro", "other", Some(&content_body)));

        let content_room = json!({
            "m.mentions": {
                "room": true
            },
            "body": "Attention room"
        });
        assert!(check_user_is_mentioned("@any:mitch.pro", "any", Some(&content_room)));

        let content_plain = json!({
            "body": "Just talking about random things"
        });
        assert!(!check_user_is_mentioned("@target:mitch.pro", "target", Some(&content_plain)));
    }

    #[tokio::test]
    async fn test_matrix_per_user_slowmode_and_unban() {
        let (state, _dir) = test_state();

        let settings_file = state.data_dir().join("matrix_room_settings.json");
        let room_id = "!general:mitch.pro";

        // 1. Set per-user slowmode
        let initial_settings = json!({
            room_id: {
                "slowmodeSeconds": 0,
                "roomMuted": false,
                "mutedUsers": {},
                "bannedUsers": {
                    "@spammer:mitch.pro": {
                        "userId": "@spammer:mitch.pro",
                        "reason": "spam",
                        "bannedBy": "admin@mitch.pro",
                        "bannedAt": 123456
                    }
                },
                "userSlowmode": {
                    "@chatter:mitch.pro": 15
                }
            }
        });
        let _ = state.store.write_document(&settings_file, &initial_settings);

        let loaded = load_matrix_room_settings(&state, room_id);
        let user_slow = loaded.get("userSlowmode").and_then(|v| v.as_object()).unwrap();
        assert_eq!(user_slow.get("@chatter:mitch.pro").and_then(|v| v.as_i64()), Some(15));

        let banned = loaded.get("bannedUsers").and_then(|v| v.as_object()).unwrap();
        assert!(banned.contains_key("@spammer:mitch.pro"));

        // 2. Slowmode tracking respects individual sender
        assert_eq!(check_matrix_slowmode(room_id, "@chatter:mitch.pro", 15), 0);
        record_matrix_message_sent(room_id, "@chatter:mitch.pro");
        let wait = check_matrix_slowmode(room_id, "@chatter:mitch.pro", 15);
        assert!(wait > 0 && wait <= 15);

        // Another user has 0 wait
        assert_eq!(check_matrix_slowmode(room_id, "@innocent:mitch.pro", 15), 0);

        // 3. Clear slowmode and unban
        let mut all = state.store.read_document(&settings_file, json!({}));
        if let Some(room) = all.get_mut(room_id) {
            if let Some(us) = room.get_mut("userSlowmode").and_then(|v| v.as_object_mut()) {
                us.remove("@chatter:mitch.pro");
            }
            if let Some(bu) = room.get_mut("bannedUsers").and_then(|v| v.as_object_mut()) {
                bu.remove("@spammer:mitch.pro");
            }
            let _ = state.store.write_document(&settings_file, &all);
        }

        let updated = load_matrix_room_settings(&state, room_id);
        assert_eq!(
            updated.get("userSlowmode").and_then(|v| v.as_object()).unwrap().get("@chatter:mitch.pro"),
            None
        );
        assert_eq!(
            updated.get("bannedUsers").and_then(|v| v.as_object()).unwrap().get("@spammer:mitch.pro"),
            None
        );
    }

    #[tokio::test]
    async fn test_room_summary_resolution() {
        let (state, dir) = test_state();

        // Cache room ID for rust
        {
            let mut cache = official_room_id_map().lock().unwrap();
            cache.insert("rust".to_string(), "!rust_room_id:mitch.pro".to_string());
        }

        // Test with URL encoded alias
        let resp = handle_room_summary(&state, "%23rust%3Amitch.pro").await;
        assert_eq!(resp.status(), StatusCode::OK);

        // Test with bare alias
        let resp2 = handle_room_summary(&state, "rust").await;
        assert_eq!(resp2.status(), StatusCode::OK);

        // Test with cached room ID
        let resp3 = handle_room_summary(&state, "!rust_room_id:mitch.pro").await;
        assert_eq!(resp3.status(), StatusCode::OK);

        // Test with unknown room
        let resp_unknown = handle_room_summary(&state, "nonexistent-room-xyz").await;
        assert_eq!(resp_unknown.status(), StatusCode::NOT_FOUND);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_matrix_unban_fuzzy_resolution_and_blacklist_cleanup() {
        let (state, dir) = test_state();

        // 1. Populate profiles.json with Long Tran
        let profiles_file = state.data_dir().join("profiles.json");
        let profiles = json!({
            "long.tran@student.rjuhsd.us": {
                "email": "long.tran@student.rjuhsd.us",
                "username": "longtran",
                "displayName": "Long Tran",
                "nickname": "Long"
            }
        });
        let _ = state.store.write_document(&profiles_file, &profiles);

        // 2. Resolve candidates for "Long Tran"
        let resolved = resolve_all_matrix_user_ids_for_target(&state, "Long Tran");
        assert!(resolved.contains(&"@longtran:mitch.pro".to_string()));
        assert!(resolved.contains(&"@long.tran:mitch.pro".to_string()));

        let resolved_emails = resolve_all_emails_for_target(&state.store, &state.data_dir(), &state.id_secret, "Long Tran");
        assert!(resolved_emails.contains(&"long.tran@student.rjuhsd.us".to_string()));

        // 3. Populate matrix_room_settings.json with banned entry under "Long Tran" and "@longtran:mitch.pro"
        let settings_file = state.data_dir().join("matrix_room_settings.json");
        let settings = json!({
            "!general:mitch.pro": {
                "bannedUsers": {
                    "Long Tran": { "userId": "Long Tran", "reason": "test" },
                    "@longtran:mitch.pro": { "userId": "@longtran:mitch.pro", "reason": "test" }
                },
                "mutedUsers": {
                    "@longtran:mitch.pro": { "userId": "@longtran:mitch.pro" }
                }
            }
        });
        let _ = state.store.write_document(&settings_file, &settings);

        // Also put them in blacklist.json
        let bl_file = state.cfg.data_dir.join("blacklist.json");
        let bl = json!({
            "long.tran@student.rjuhsd.us": { "reason": "banned" }
        });
        let _ = state.store.write_document(&bl_file, &bl);

        // 4. Perform unban
        unban_matrix_user_from_room(&state.id_secret, &state.store, &state.data_dir(), "!general:mitch.pro", "@longtran:mitch.pro").await;

        let loaded_settings = state.store.read_document(&settings_file, json!({}));
        let room = loaded_settings.get("!general:mitch.pro").unwrap();
        let banned_users = room.get("bannedUsers").and_then(|v| v.as_object()).unwrap();
        assert!(banned_users.is_empty(), "bannedUsers should be cleaned: {banned_users:?}");

        // 5. Test mod_unban HTTP handler with staff authentication
        let admins_file = state.store.base_dir.join("data/admins.json");
        let admins = json!({
            "owner": "admin@mitch.pro",
            "admins": ["admin@mitch.pro"]
        });
        let _ = state.store.write_document(&admins_file, &admins);

        let sid = mitch_lib::auth::issue_login_session(
            &state.store,
            &state.id_secret,
            "admin@mitch.pro",
            "admin@mitch.pro",
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            "Cookie",
            HeaderValue::from_str(&format!("studentId={sid}")).unwrap(),
        );

        let unban_body = serde_json::to_vec(&json!({
            "userId": "Long Tran",
            "roomId": "all"
        })).unwrap();

        let resp = mod_unban(&state, &headers, &unban_body).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let loaded_bl = state.store.read_document(&bl_file, json!({}));
        assert!(loaded_bl.get("long.tran@student.rjuhsd.us").is_none(), "Blacklist must be lifted");
        assert!(loaded_bl.get("longtran@student.rjuhsd.us").is_none(), "Blacklist must be lifted");

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn sample_gif(id: &str) -> Value {
        json!({
            "id": id,
            "title": format!("gif {id}"),
            "url": format!("https://example.com/{id}.gif"),
            "preview": format!("https://example.com/{id}-preview.gif"),
            "width": 320,
            "height": 240,
        })
    }

    #[test]
    fn favorite_add_then_remove() {
        let mut list: Vec<Value> = Vec::new();
        let gif = sample_gif("abc");
        apply_gif_favorite_action(&mut list, "abc", "add", Some(&gif)).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].get("url").unwrap().as_str().unwrap(), "https://example.com/abc.gif");

        apply_gif_favorite_action(&mut list, "abc", "remove", None).unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn favorite_re_add_moves_to_front_without_duplicating() {
        let mut list: Vec<Value> = Vec::new();
        apply_gif_favorite_action(&mut list, "a", "add", Some(&sample_gif("a"))).unwrap();
        apply_gif_favorite_action(&mut list, "b", "add", Some(&sample_gif("b"))).unwrap();
        assert_eq!(list.len(), 2);
        // Re-favoriting "a" should move it back to the front, not duplicate it.
        apply_gif_favorite_action(&mut list, "a", "add", Some(&sample_gif("a"))).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].get("id").unwrap().as_str().unwrap(), "a");
        assert_eq!(list[1].get("id").unwrap().as_str().unwrap(), "b");
    }

    #[test]
    fn favorite_add_requires_gif_with_url() {
        let mut list: Vec<Value> = Vec::new();
        assert!(apply_gif_favorite_action(&mut list, "a", "add", None).is_err());
        let no_url = json!({ "id": "a", "title": "no url" });
        assert!(apply_gif_favorite_action(&mut list, "a", "add", Some(&no_url)).is_err());
        assert!(list.is_empty());
    }

    #[test]
    fn favorite_list_caps_at_max() {
        let mut list: Vec<Value> = Vec::new();
        for i in 0..(MAX_GIF_FAVORITES + 10) {
            let id = format!("g{i}");
            apply_gif_favorite_action(&mut list, &id, "add", Some(&sample_gif(&id))).unwrap();
        }
        assert_eq!(list.len(), MAX_GIF_FAVORITES);
        // Most recently added stays at the front.
        assert_eq!(
            list[0].get("id").unwrap().as_str().unwrap(),
            format!("g{}", MAX_GIF_FAVORITES + 9)
        );
    }

    #[tokio::test]
    async fn gifs_favorites_requires_auth() {
        let (state, _dir) = test_state();
        let headers = HeaderMap::new();

        let resp = handle_api(&state, &Method::GET, "/api/matrix/gifs/favorites", &headers, "", &[])
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let body = serde_json::to_vec(&json!({ "action": "add", "gif": sample_gif("x") })).unwrap();
        let resp = handle_api(
            &state,
            &Method::POST,
            "/api/matrix/gifs/favorites",
            &headers,
            "",
            &body,
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
