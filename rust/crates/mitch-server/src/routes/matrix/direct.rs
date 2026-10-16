use super::*;

// Account-data updates must be serialized with room creation: two concurrent
// requests otherwise overwrite m.direct or invite the same person twice.
type AccountLock = tokio::sync::Mutex<()>;
static DIRECT_LOCKS: OnceLock<Mutex<HashMap<String, std::sync::Weak<AccountLock>>>> =
    OnceLock::new();

fn account_lock(user: &str) -> Arc<AccountLock> {
    let mut locks = DIRECT_LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(lock) = locks.get(user).and_then(std::sync::Weak::upgrade) {
        return lock;
    }
    locks.retain(|_, lock| lock.strong_count() > 0);
    let lock = Arc::new(AccountLock::new(()));
    locks.insert(user.to_string(), Arc::downgrade(&lock));
    lock
}

fn segment(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

async fn request(
    headers: &HeaderMap,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, Box<Response>> {
    match call_conduit(
        path,
        method,
        Some(headers.clone()),
        body.map(|v| Bytes::from(v.to_string())),
    )
    .await
    {
        Ok((status, _, bytes)) if status.is_success() => {
            Ok(serde_json::from_slice(&bytes).unwrap_or(json!({})))
        }
        Ok((StatusCode::NOT_FOUND, _, _)) => Err(Box::new(cors_json_response(
            404,
            json!({"error":"Conversation not found."}),
        ))),
        Ok((StatusCode::UNAUTHORIZED, _, _)) => Err(Box::new(cors_json_response(
            401,
            json!({"error":"Your chat session expired. Reload Chat to sign in again."}),
        ))),
        _ => Err(Box::new(cors_json_response(
            502,
            json!({"error":"Chat is unavailable. Please try again."}),
        ))),
    }
}

pub(super) async fn open(state: &AppState, headers: &HeaderMap, body: &[u8]) -> Response {
    if !crate::routes::vm::vm_same_origin_request(headers) {
        return cors_json_response(403, json!({"error":"Open Chat from this website."}));
    }
    let Some(auth) = headers.get("Authorization").filter(|v| {
        v.to_str()
            .is_ok_and(|v| v.starts_with("Bearer ") && v.len() > 7)
    }) else {
        return cors_json_response(401, json!({"error":"Sign in to Chat first."}));
    };
    let mut upstream = HeaderMap::new();
    upstream.insert("Authorization", auth.clone());
    upstream.insert("Content-Type", HeaderValue::from_static("application/json"));
    // Always validate the supplied token; a website cookie cannot stand in for
    // the Matrix identity whose account data and private rooms we are editing.
    let who = match request(
        &upstream,
        Method::GET,
        "/_matrix/client/v3/account/whoami",
        None,
    )
    .await
    {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let Some(actor) = who["user_id"].as_str().filter(|v| !v.is_empty()) else {
        return cors_json_response(401, json!({"error":"Sign in to Chat first."}));
    };
    let Some(account) = resolve_matrix_account(state, headers, None).await else {
        return cors_json_response(401, json!({"error":"Sign in required."}));
    };
    if account.user_id != actor
        || account.norm_email.is_empty()
        || mitch_lib::auth::banned_info_for_email(&state.store, &account.norm_email).is_some()
    {
        return cors_json_response(
            403,
            json!({"error":"Chat access is unavailable for this account."}),
        );
    }
    let parsed: Value = serde_json::from_slice(body).unwrap_or(json!({}));
    let recipient = parsed["recipient"].as_str().unwrap_or("").trim();
    let Some(email) = crate::routes::admin::accounts::resolve(state, recipient) else {
        return cors_json_response(
            404,
            json!({"error":"Choose a member from People or Friends."}),
        );
    };
    if mitch_lib::auth::banned_info_for_email(&state.store, &email).is_some() {
        return cors_json_response(
            403,
            json!({"error":"This account is unavailable for chat."}),
        );
    }
    let Some(username) = matrix_username_for_email(state, &email).await else {
        return cors_json_response(
            409,
            json!({"error":"This person needs to open Chat once before they can receive messages."}),
        );
    };
    let target = format!("@{username}:mitch.pro");
    if target == actor {
        return cors_json_response(400, json!({"error":"Choose someone else to message."}));
    }
    let lock = account_lock(actor);
    let _guard = lock.lock().await;
    let data_path = format!(
        "/_matrix/client/v3/user/{}/account_data/m.direct",
        segment(actor)
    );
    let mut direct = match request(&upstream, Method::GET, &data_path, None).await {
        Ok(v) if v.is_object() => v,
        Err(r) if r.status() == StatusCode::NOT_FOUND => json!({}),
        Err(r) => return *r,
        _ => {
            return cors_json_response(
                502,
                json!({"error":"Could not read your conversations. Try again."}),
            )
        }
    };
    let joined = match request(
        &upstream,
        Method::GET,
        "/_matrix/client/v3/joined_rooms",
        None,
    )
    .await
    {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let Some(joined) = joined["joined_rooms"].as_array() else {
        return cors_json_response(
            502,
            json!({"error":"Could not read your conversations. Try again."}),
        );
    };
    let joined: Vec<&str> = joined.iter().filter_map(Value::as_str).collect();
    // Check existing rooms, including ones whose m.direct update failed after
    // creation. Never reuse a group or a room the recipient has left.
    let mut rooms: Vec<&str> = direct[&target]
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(Value::as_str)
                .filter(|id| joined.contains(id))
                .collect()
        })
        .unwrap_or_default();
    rooms.extend(
        joined
            .iter()
            .copied()
            .filter(|id| !rooms.contains(id))
            .collect::<Vec<_>>(),
    );
    let mut found = None;
    for room in rooms {
        let members = match request(
            &upstream,
            Method::GET,
            &format!("/_matrix/client/v3/rooms/{}/state", segment(room)),
            None,
        )
        .await
        {
            Ok(v) => v,
            Err(r) if r.status() == StatusCode::NOT_FOUND => continue,
            Err(r) => return *r,
        };
        let Some(events) = members.as_array() else {
            continue;
        };
        let active: Vec<_> = events
            .iter()
            .filter(|event| {
                event["type"] == "m.room.member"
                    && matches!(
                        event["content"]["membership"].as_str(),
                        Some("join" | "invite")
                    )
            })
            .collect();
        let target_event = active.iter().find(|event| event["state_key"] == target);
        let private = events.iter().any(|event| {
            event["type"] == "m.room.join_rules" && event["content"]["join_rule"] == "invite"
        });
        let marked = direct[&target]
            .as_array()
            .is_some_and(|v| v.iter().any(|v| v.as_str() == Some(room)));
        let invited_direct =
            target_event.is_some_and(|event| event["content"]["is_direct"] == true);
        if active.len() == 2
            && active.iter().any(|event| event["state_key"] == actor)
            && target_event.is_some()
            && (marked || invited_direct)
            && private
        {
            found = Some(room.to_string());
            break;
        }
    }
    let room = match found {
        Some(room) => room,
        None => {
            let created=match request(&upstream,Method::POST,"/_matrix/client/v3/createRoom",Some(json!({"is_direct":true,"visibility":"private","preset":"private_chat","invite":[target],"initial_state":[{"type":"m.room.encryption","state_key":"","content":{"algorithm":"m.megolm.v1.aes-sha2"}}]}))).await {Ok(v)=>v,Err(r)=>return *r};
            let Some(room) = created["room_id"].as_str() else {
                return cors_json_response(
                    502,
                    json!({"error":"Chat did not return a conversation. Try again."}),
                );
            };
            room.to_string()
        }
    };
    let mut list = direct[&target].as_array().cloned().unwrap_or_default();
    if !list.iter().any(|v| v.as_str() == Some(&room)) {
        list.push(json!(room));
    }
    direct[&target] = json!(list);
    if let Err(response) = request(&upstream, Method::PUT, &data_path, Some(direct)).await {
        return *response;
    }
    cors_json_response(200, json!({"roomId":room,"userId":target}))
}
