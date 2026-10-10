//! Blog route group (server.js:11308-11369, 11685-11943) + helper logic
//! (server.js:6580-6895).
//!
//! - `GET /api/blog/me` — current user's blog role flags
//! - `GET /api/blog/subscription` — user's email notification subscription state
//! - `POST /api/blog/subscription` — update subscription state
//! - `POST /api/blog/upload` — upload blog image asset
//! - `GET /api/blog/posts` — list published posts (and own/all if privileged)
//! - `POST /api/blog/posts` — create a new post
//! - `GET /api/blog/posts/:key` — fetch single post by id or slug
//! - `PATCH|PUT /api/blog/posts/:key` — update post
//! - `DELETE /api/blog/posts/:key` — delete post and write to deletion log
//! - `GET /api/blog/posts/:key/comments` — list comments for a post
//! - `POST /api/blog/posts/:key/comments` — add comment
//! - `PATCH /api/blog/posts/:key/comments/:subId` — moderate comment status
//! - `DELETE /api/blog/posts/:key/comments/:subId` — delete comment
//! - `GET /api/blog/posts/:key/revisions` — list revision history
//! - `POST /api/blog/posts/:key/restore` — restore revision snapshot

use super::me::{cookies_of, data_file, json_response, me_uid, parse_body_strict};
use super::push::send_email_bg;
use crate::routes::admin::legacy::html_base_template;
use crate::state::AppState;
use crate::workers_email::site_url;
use axum::http::{HeaderMap, Method};
use axum::response::Response;
use base64::Engine;
use mitch_lib::admin::{blog_contributor_emails, log_admin_action, mask_email};
use mitch_lib::auth::{is_admin_email, is_moderator_email, normalize_email, valid_id};
use mitch_lib::blog::{
    blog_excerpt, blog_html_to_text, safe_blog_url, sanitize_blog_category, sanitize_blog_html,
    sanitize_blog_tags, slugify_blog_title,
};
use mitch_lib::profile::default_username_for_email;
use mitch_lib::school::now_millis;
use serde_json::{json, Value};
use std::sync::Arc;

/// Resolves the authenticated email for blog/backgrounds endpoints
/// (matches JS `authedEmailForRequest()`, server.js:11301-11306).
pub fn authed_email_for_request(state: &AppState, headers: &HeaderMap) -> Option<String> {
    let cookies = cookies_of(state, headers);
    let sid = me_uid(&cookies);
    if !valid_id(&sid, &state.id_secret) {
        return None;
    }
    if let Some(sess) = cookies.get("_authSession") {
        if let Ok(v) = serde_json::from_str::<Value>(sess) {
            if let Some(e) = v.get("email").and_then(|v| v.as_str()) {
                let trimmed = e.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    if let Some(e) = mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &sid) {
        let trimmed = e.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    if let Some(e) = mitch_lib::auth::names_get_email(&state.store, &sid) {
        let trimmed = e.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

/// Checks if an email is listed in `blog_contributors.json`.
pub fn is_blog_contributor_email(state: &AppState, email: &str) -> bool {
    if email.is_empty() {
        return false;
    }
    let norm = normalize_email(email);
    blog_contributor_emails(&state.store, state.data_dir())
        .iter()
        .any(|c| normalize_email(c) == norm)
}

/// `canWriteBlogEmail(email)`: admin, moderator, or contributor.
pub fn can_write_blog_email(state: &AppState, email: &str) -> bool {
    if email.is_empty() {
        return false;
    }
    is_admin_email(&state.store, email)
        || is_moderator_email(&state.store, email)
        || is_blog_contributor_email(state, email)
}

/// `blogRoleForEmail(email)`: admin, moderator, contributor, or user.
pub fn blog_role_for_email(state: &AppState, email: &str) -> &'static str {
    if is_admin_email(&state.store, email) {
        "admin"
    } else if is_moderator_email(&state.store, email) {
        "moderator"
    } else if is_blog_contributor_email(state, email) {
        "contributor"
    } else {
        "user"
    }
}

/// `blogAuthorName(email)` (server.js:6597).
pub fn blog_author_name(state: &AppState, email: &str) -> String {
    let norm = normalize_email(email);
    let profiles = state
        .store
        .read_document(&data_file(state, "profiles.json"), json!({}));
    if let Some(prof) = profiles.get(&norm) {
        if let Some(nick) = prof
            .get("nickname")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            return nick.to_string();
        }
        if let Some(disp) = prof
            .get("displayName")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            return disp.to_string();
        }
        if let Some(u) = prof
            .get("username")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            return u.to_string();
        }
    }
    default_username_for_email(&norm)
}

/// `blogPublished(post)` (server.js:6621).
pub fn blog_published(post: &Value) -> bool {
    let status = post
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("published");
    let publish_at = post
        .get("publishAt")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as i64;
    status == "published" || (status == "scheduled" && publish_at <= now_millis())
}

/// `blogCanManagePost(email, post)` (server.js:6743).
pub fn blog_can_manage_post(state: &AppState, email: &str, post: &Value) -> bool {
    if email.is_empty() {
        return false;
    }
    if is_admin_email(&state.store, email) {
        return true;
    }
    if !can_write_blog_email(state, email) {
        return false;
    }
    let author = post
        .get("authorEmail")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    normalize_email(email) == normalize_email(author)
}

/// `blogRevisionSnapshot(post, actorEmail, reason)` (server.js:6750).
pub fn blog_revision_snapshot(post: &Value, actor_email: &str, reason: &str) -> Value {
    let now = now_millis();
    let id: String = (0..8)
        .map(|_| format!("{:02x}", rand::random::<u8>()))
        .collect();
    let body_raw = post.get("body").and_then(|v| v.as_str()).unwrap_or("");
    let body_html_raw = post.get("bodyHtml").and_then(|v| v.as_str()).unwrap_or("");
    json!({
        "id": id,
        "ts": now,
        "actorEmail": normalize_email(actor_email),
        "reason": reason,
        "title": post.get("title").and_then(|v| v.as_str()).unwrap_or(""),
        "body": body_raw,
        "bodyHtml": sanitize_blog_html(body_html_raw, body_raw),
        "tags": sanitize_blog_tags(post.get("tags").unwrap_or(&json!([]))),
        "category": sanitize_blog_category(post.get("category").and_then(|v| v.as_str()).unwrap_or("")),
        "coverImage": safe_blog_url(post.get("coverImage").and_then(|v| v.as_str()).unwrap_or(""), true),
        "featured": post.get("featured").and_then(|v| v.as_bool()).unwrap_or(false),
        "status": post.get("status").and_then(|v| v.as_str()).unwrap_or("published"),
        "publishAt": post.get("publishAt").and_then(|v| v.as_f64()).unwrap_or(0.0) as u64,
        "publishedAt": post.get("publishedAt").and_then(|v| v.as_f64()).unwrap_or(0.0) as u64,
    })
}

/// `pushBlogRevision(post, actorEmail, reason)` (server.js:6769).
pub fn push_blog_revision(post: &mut Value, actor_email: &str, reason: &str) {
    let rev = blog_revision_snapshot(post, actor_email, reason);
    let mut revisions = post
        .get("revisions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    revisions.insert(0, rev);
    if revisions.len() > 25 {
        revisions.truncate(25);
    }
    if let Some(map) = post.as_object_mut() {
        map.insert("revisions".to_string(), Value::Array(revisions));
    }
}

/// `publicBlogPost(post, includeBody, viewerEmail)` (server.js:6775).
pub fn public_blog_post(
    state: &AppState,
    post: &Value,
    include_body: bool,
    viewer_email: &str,
) -> Value {
    let author_email = post
        .get("authorEmail")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let own =
        !viewer_email.is_empty() && normalize_email(viewer_email) == normalize_email(author_email);
    let writer = !viewer_email.is_empty() && can_write_blog_email(state, viewer_email);
    let admin = !viewer_email.is_empty() && is_admin_email(&state.store, viewer_email);
    let can_manage = blog_can_manage_post(state, viewer_email, post);
    let body_raw = post.get("body").and_then(|v| v.as_str()).unwrap_or("");
    let body_html_raw = post.get("bodyHtml").and_then(|v| v.as_str()).unwrap_or("");
    let body_html = sanitize_blog_html(body_html_raw, body_raw);
    let body_text = if !body_raw.is_empty() {
        body_raw.to_string()
    } else {
        blog_html_to_text(&body_html)
    };
    let author_name = post
        .get("authorName")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| blog_author_name(state, author_email));

    let can_mod_comments = admin
        || (own && writer)
        || (!viewer_email.is_empty() && is_moderator_email(&state.store, viewer_email));

    let revisions_count = post
        .get("revisions")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);

    let mut obj = json!({
        "id": post.get("id").and_then(|v| v.as_str()).unwrap_or(""),
        "slug": post.get("slug").and_then(|v| v.as_str()).unwrap_or(""),
        "title": post.get("title").and_then(|v| v.as_str()).unwrap_or(""),
        "excerpt": blog_excerpt(&body_text),
        "status": post.get("status").and_then(|v| v.as_str()).unwrap_or("published"),
        "authorName": author_name,
        "tags": sanitize_blog_tags(post.get("tags").unwrap_or(&json!([]))),
        "category": sanitize_blog_category(post.get("category").and_then(|v| v.as_str()).unwrap_or("")),
        "coverImage": safe_blog_url(post.get("coverImage").and_then(|v| v.as_str()).unwrap_or(""), true),
        "featured": post.get("featured").and_then(|v| v.as_bool()).unwrap_or(false),
        "publishAt": post.get("publishAt").and_then(|v| v.as_f64()).unwrap_or(0.0) as u64,
        "createdAt": post.get("createdAt").and_then(|v| v.as_f64()).unwrap_or(0.0) as u64,
        "updatedAt": post.get("updatedAt").and_then(|v| v.as_f64()).unwrap_or(0.0) as u64,
        "publishedAt": post.get("publishedAt").and_then(|v| v.as_f64()).unwrap_or(0.0) as u64,
        "canEdit": can_manage,
        "canDelete": can_manage,
        "canModerateComments": can_mod_comments,
        "revisionsCount": revisions_count,
    });

    if include_body {
        if let Some(map) = obj.as_object_mut() {
            map.insert("body".to_string(), json!(body_text));
            map.insert("bodyHtml".to_string(), json!(body_html));
        }
    }
    obj
}

/// `publicBlogComment(comment, viewerEmail, canModerate)` (server.js:6814).
pub fn public_blog_comment(
    state: &AppState,
    comment: &Value,
    viewer_email: &str,
    can_moderate: bool,
) -> Value {
    let author_email = comment
        .get("authorEmail")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let author_name = comment
        .get("authorName")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| blog_author_name(state, author_email));
    let masked_email = if can_moderate {
        mask_email(author_email)
    } else {
        String::new()
    };
    let can_delete = can_moderate
        || (!viewer_email.is_empty()
            && normalize_email(viewer_email) == normalize_email(author_email));

    json!({
        "id": comment.get("id").and_then(|v| v.as_str()).unwrap_or(""),
        "postId": comment.get("postId").and_then(|v| v.as_str()).unwrap_or(""),
        "authorName": author_name,
        "authorEmail": masked_email,
        "body": comment.get("body").and_then(|v| v.as_str()).unwrap_or("").chars().take(1200).collect::<String>(),
        "status": comment.get("status").and_then(|v| v.as_str()).unwrap_or("pending"),
        "ts": comment.get("ts").and_then(|v| v.as_f64()).unwrap_or(0.0) as u64,
        "canDelete": can_delete,
    })
}

/// `uniqueBlogSlug(posts, title, exceptId)` (server.js:6648).
pub fn unique_blog_slug(posts: &[Value], title: &str, except_id: &str) -> String {
    let base = slugify_blog_title(title);
    let mut slug = base.clone();
    let mut i = 2;
    while posts.iter().any(|p| {
        p.get("id").and_then(|v| v.as_str()).unwrap_or("") != except_id
            && p.get("slug").and_then(|v| v.as_str()).unwrap_or("") == slug
    }) {
        slug = format!("{base}-{i}");
        i += 1;
    }
    slug
}

/// `logBlogDeletion(post, actorEmail)` (server.js:6836).
pub fn log_blog_deletion(state: &AppState, post: &Value, actor_email: &str) {
    let actor = normalize_email(actor_email);
    let id: String = (0..8)
        .map(|_| format!("{:02x}", rand::random::<u8>()))
        .collect();
    let entry = json!({
        "id": id,
        "ts": now_millis(),
        "postId": post.get("id").and_then(|v| v.as_str()).unwrap_or(""),
        "slug": post.get("slug").and_then(|v| v.as_str()).unwrap_or(""),
        "title": post.get("title").and_then(|v| v.as_str()).unwrap_or("").chars().take(180).collect::<String>(),
        "status": post.get("status").and_then(|v| v.as_str()).unwrap_or("published"),
        "authorEmail": normalize_email(post.get("authorEmail").and_then(|v| v.as_str()).unwrap_or("")),
        "authorName": post.get("authorName").and_then(|v| v.as_str()).unwrap_or("").chars().take(80).collect::<String>(),
        "deletedBy": actor,
        "deletedByRole": blog_role_for_email(state, &actor),
    });

    let file = data_file(state, "blog_delete_log.json");
    let logs = state.store.read_document(&file, json!([]));
    let mut list = logs.as_array().cloned().unwrap_or_default();
    list.insert(0, entry.clone());
    if list.len() > 500 {
        list.truncate(500);
    }
    let _ = state.store.write_document(&file, &Value::Array(list));

    let actor_log = if actor.is_empty() { "blog" } else { &actor };
    log_admin_action(
        &state.store,
        state.data_dir(),
        actor_log,
        "delete_blog_post",
        json!({
            "postId": entry.get("postId"),
            "slug": entry.get("slug"),
            "title": entry.get("title"),
            "authorEmail": entry.get("authorEmail"),
            "deletedByRole": entry.get("deletedByRole"),
        }),
    );
}

/// `makeBlogNotificationHtml(email, post, link)` (server.js:1996).
pub fn make_blog_notification_html(
    state: &AppState,
    email: &str,
    post: &Value,
    link: &str,
) -> String {
    let pref_url = format!("{}/preferences/#privacy", site_url(state, email));
    let title = post.get("title").and_then(|v| v.as_str()).unwrap_or("");
    let author_name = post
        .get("authorName")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("mitch.pro");
    let body_text = post.get("body").and_then(|v| v.as_str()).unwrap_or("");
    let excerpt = blog_excerpt(body_text);

    let content = format!(
        r#"
    <h2 style="margin: 0 0 8px; font-size: 22px; font-weight: 800; color: #f4f4f5; text-align: left; line-height: 1.3;">📰 {}</h2>
    <p style="margin: 0 0 20px; font-size: 13px; color: #94a3b8;">Published by <strong>{}</strong></p>
    <div style="background-color: rgba(255, 255, 255, 0.03); border-left: 4px solid #a855f7; border-radius: 4px; padding: 16px 20px; margin-bottom: 24px; font-style: italic; line-height: 1.7; color: #cbd5e1;">
      "{}"
    </div>
    <div style="text-align: left; margin-bottom: 24px;">
      <a href="{}" style="display: inline-block; background: linear-gradient(135deg, #a855f7, #6366f1); color: #ffffff; text-decoration: none; padding: 12px 24px; border-radius: 8px; font-weight: 700;">Read Full Post</a>
    </div>
    <p style="margin: 32px 0 0; font-size: 11px; color: #64748b; text-align: center;">
      You received this because you are subscribed to blog alerts. <br>
      You can manage your notification settings in <a href="{}" style="color: #64748b; text-decoration: underline;">Preferences</a>.
    </p>
    "#,
        title, author_name, excerpt, link, pref_url
    );

    html_base_template(state, email, &format!("New blog post: {title}"), &content)
}

/// `notifyBlogSubscribers(post)` (server.js:6886).
pub fn notify_blog_subscribers(state: &Arc<AppState>, post: &Value) {
    let file = data_file(state, "blog_subscribers.json");
    let raw = state.store.read_document(&file, json!({}));
    let mut subscribers: Vec<String> = Vec::new();
    if let Some(arr) = raw.as_array() {
        for v in arr {
            if let Some(s) = v.as_str() {
                let norm = normalize_email(s);
                if !norm.is_empty() {
                    subscribers.push(norm);
                }
            }
        }
    } else if let Some(map) = raw.as_object() {
        for (email, enabled) in map {
            if enabled.as_bool() == Some(true) {
                let norm = normalize_email(email);
                if !norm.is_empty() {
                    subscribers.push(norm);
                }
            }
        }
    }

    let slug = post.get("slug").and_then(|v| v.as_str()).unwrap_or("");
    let title = post.get("title").and_then(|v| v.as_str()).unwrap_or("");
    for email in subscribers {
        let link = format!(
            "{}/blog/#post/{}",
            site_url(state, &email),
            crate::handler::encode_uri_component(slug)
        );
        let html = make_blog_notification_html(state, &email, post, &link);
        send_email_bg(state, &email, &format!("New blog post: {title}"), &html);
    }
}

/// `publishDueBlogPosts(posts)` (server.js:6626).
pub fn publish_due_blog_posts(state: &Arc<AppState>, posts: &mut [Value]) {
    let now = now_millis();
    let mut changed = false;
    for post in posts.iter_mut() {
        let status = post.get("status").and_then(|v| v.as_str()).unwrap_or("");
        let publish_at = post
            .get("publishAt")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as i64;
        if status == "scheduled" && publish_at <= now {
            if let Some(map) = post.as_object_mut() {
                map.insert("status".to_string(), json!("published"));
                let pub_at = map
                    .get("publishedAt")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0) as u64;
                if pub_at == 0 {
                    map.insert("publishedAt".to_string(), json!(now));
                }
                map.insert("updatedAt".to_string(), json!(now));
                changed = true;
                if map.get("notifiedAt").is_none() {
                    let post_val = Value::Object(map.clone());
                    notify_blog_subscribers(state, &post_val);
                    map.insert("notifiedAt".to_string(), json!(now));
                }
            }
        }
    }
    if changed {
        let file = data_file(state, "blog_posts.json");
        let _ = state
            .store
            .write_document(&file, &Value::Array(posts.to_vec()));
    }
}

fn check_custom_rate_limit(
    state: &AppState,
    headers: &HeaderMap,
    bucket: &str,
) -> Option<Response> {
    let cookies = cookies_of(state, headers);
    let val = cookies
        .get("studentId")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cookies.get("id").unwrap_or(""));
    let id_key = if valid_id(val, &state.id_secret) {
        format!("id:{val}")
    } else {
        "anon".to_string()
    };
    let ip = crate::handler::get_real_ip(headers, None);
    if let Some((code, msg)) = state.rate_limit_check(&ip, &id_key, bucket) {
        Some(json_response(code, json!({ "error": msg })))
    } else {
        None
    }
}

/// Dispatches `/api/blog/*` routes.
pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
) -> Option<Response> {
    if !path.starts_with("/api/blog/") {
        return None;
    }

    // GET /api/blog/me
    if path == "/api/blog/me" && *method == Method::GET {
        let Some(email) = authed_email_for_request(state, headers) else {
            return Some(json_response(401, json!({ "error": "not logged in" })));
        };
        let is_contributor = is_blog_contributor_email(state, &email);
        let is_admin = is_admin_email(&state.store, &email);
        let is_moderator = is_moderator_email(&state.store, &email);
        let can_write = is_admin || is_moderator || is_contributor;
        return Some(json_response(
            200,
            json!({
                "canWrite": can_write,
                "isContributor": is_contributor,
                "isAdmin": is_admin,
                "isModerator": is_moderator,
            }),
        ));
    }

    // GET /api/blog/subscription
    if path == "/api/blog/subscription" && *method == Method::GET {
        let Some(email) = authed_email_for_request(state, headers) else {
            return Some(json_response(401, json!({ "error": "not logged in" })));
        };
        let norm = normalize_email(&email);
        let file = data_file(state, "blog_subscribers.json");
        let raw = state.store.read_document(&file, json!({}));
        let enabled = if let Some(map) = raw.as_object() {
            map.get(&norm).and_then(|v| v.as_bool()).unwrap_or(false)
        } else if let Some(arr) = raw.as_array() {
            arr.iter()
                .any(|v| v.as_str().map(normalize_email) == Some(norm.clone()))
        } else {
            false
        };
        return Some(json_response(200, json!({ "enabled": enabled })));
    }

    // POST /api/blog/subscription
    if path == "/api/blog/subscription" && *method == Method::POST {
        let Some(email) = authed_email_for_request(state, headers) else {
            return Some(json_response(401, json!({ "error": "not logged in" })));
        };
        let Some(body) = parse_body_strict(body_bytes) else {
            return Some(json_response(400, json!({ "error": "bad json" })));
        };
        let norm = normalize_email(&email);
        let file = data_file(state, "blog_subscribers.json");
        let raw = state.store.read_document(&file, json!({}));
        let mut map = if let Some(obj) = raw.as_object() {
            obj.clone()
        } else if let Some(arr) = raw.as_array() {
            let mut m = serde_json::Map::new();
            for item in arr {
                if let Some(s) = item.as_str() {
                    let n = normalize_email(s);
                    if !n.is_empty() {
                        m.insert(n, json!(true));
                    }
                }
            }
            m
        } else {
            serde_json::Map::new()
        };

        if body.get("enabled").and_then(|v| v.as_bool()) == Some(true) {
            map.insert(norm.clone(), json!(true));
        } else {
            map.remove(&norm);
        }
        let _ = state
            .store
            .write_document(&file, &Value::Object(map.clone()));
        let is_enabled = map.get(&norm).and_then(|v| v.as_bool()).unwrap_or(false);
        return Some(json_response(
            200,
            json!({ "ok": true, "enabled": is_enabled }),
        ));
    }

    // POST /api/blog/upload
    if path == "/api/blog/upload" && *method == Method::POST {
        if let Some(resp) = check_custom_rate_limit(state, headers, "/api/blog/upload") {
            return Some(resp);
        }
        let Some(email) = authed_email_for_request(state, headers) else {
            return Some(json_response(401, json!({ "error": "not logged in" })));
        };
        if !can_write_blog_email(state, &email) {
            return Some(json_response(403, json!({ "error": "forbidden" })));
        }
        let Some(body) = parse_body_strict(body_bytes) else {
            return Some(json_response(400, json!({ "error": "bad json" })));
        };
        let mime = body
            .get("mime")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        let data = body.get("data").and_then(|v| v.as_str()).unwrap_or("");
        let ext = match mime.as_str() {
            "image/png" => "png",
            "image/jpeg" | "image/jpg" => "jpg",
            "image/webp" => "webp",
            "image/gif" => "gif",
            _ => "",
        };
        if ext.is_empty() {
            return Some(json_response(
                400,
                json!({ "error": "unsupported image type" }),
            ));
        }
        let prefix = format!("data:{mime};base64,");
        if !data.starts_with(&prefix) {
            return Some(json_response(400, json!({ "error": "invalid image data" })));
        }
        let payload = &data[prefix.len()..];
        if payload.is_empty()
            || !payload.chars().all(|c| {
                c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=' || c.is_whitespace()
            })
        {
            return Some(json_response(400, json!({ "error": "invalid image data" })));
        }
        let clean_payload: String = payload.chars().filter(|c| !c.is_whitespace()).collect();
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(clean_payload.as_bytes())
        else {
            return Some(json_response(400, json!({ "error": "invalid image data" })));
        };
        if bytes.is_empty() || bytes.len() > 900_000 {
            return Some(json_response(413, json!({ "error": "image too large" })));
        }

        let upload_dir = state.cfg.webroot.join("blog").join("uploads");
        let _ = std::fs::create_dir_all(&upload_dir);
        let id: String = (0..12)
            .map(|_| format!("{:02x}", rand::random::<u8>()))
            .collect();
        let filename = format!("{id}.{ext}");
        let full_path = upload_dir.join(&filename);
        if std::fs::write(&full_path, &bytes).is_err() {
            return Some(json_response(
                500,
                json!({ "error": "failed to save image" }),
            ));
        }
        log_admin_action(
            &state.store,
            state.data_dir(),
            &email,
            "upload_blog_image",
            json!({ "filename": filename, "mime": mime, "bytes": bytes.len() }),
        );
        return Some(json_response(
            200,
            json!({ "ok": true, "url": format!("/blog/uploads/{filename}") }),
        ));
    }

    // GET /api/blog/posts
    if path == "/api/blog/posts" && *method == Method::GET {
        let Some(email) = authed_email_for_request(state, headers) else {
            return Some(json_response(401, json!({ "error": "not logged in" })));
        };
        let norm = normalize_email(&email);
        let staff =
            is_admin_email(&state.store, &email) || is_moderator_email(&state.store, &email);
        let writer = can_write_blog_email(state, &email);

        let file = data_file(state, "blog_posts.json");
        let raw = state.store.read_document(&file, json!([]));
        let mut posts = raw.as_array().cloned().unwrap_or_default();
        publish_due_blog_posts(state, &mut posts);

        let mut filtered: Vec<Value> = posts
            .iter()
            .filter(|post| {
                blog_published(post)
                    || (writer
                        && (staff
                            || normalize_email(
                                post.get("authorEmail")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(""),
                            ) == norm))
            })
            .cloned()
            .collect();

        filtered.sort_by(|a, b| {
            let ts_a = a
                .get("publishedAt")
                .or_else(|| a.get("updatedAt"))
                .or_else(|| a.get("createdAt"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as u64;
            let ts_b = b
                .get("publishedAt")
                .or_else(|| b.get("updatedAt"))
                .or_else(|| b.get("createdAt"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as u64;
            ts_b.cmp(&ts_a)
        });

        let posts_public: Vec<Value> = filtered
            .iter()
            .map(|p| public_blog_post(state, p, false, &email))
            .collect();

        let mut categories: Vec<String> = posts
            .iter()
            .filter_map(|p| {
                let cat = sanitize_blog_category(
                    p.get("category").and_then(|v| v.as_str()).unwrap_or(""),
                );
                if cat.is_empty() {
                    None
                } else {
                    Some(cat)
                }
            })
            .collect();
        categories.sort();
        categories.dedup();

        let mut tags: Vec<String> = posts
            .iter()
            .flat_map(|p| sanitize_blog_tags(p.get("tags").unwrap_or(&json!([]))))
            .filter(|t| !t.is_empty())
            .collect();
        tags.sort();
        tags.dedup();

        return Some(json_response(
            200,
            json!({
                "posts": posts_public,
                "canWrite": writer,
                "categories": categories,
                "tags": tags,
            }),
        ));
    }

    // POST /api/blog/posts
    if path == "/api/blog/posts" && *method == Method::POST {
        if let Some(resp) = check_custom_rate_limit(state, headers, "/api/blog/write") {
            return Some(resp);
        }
        let Some(email) = authed_email_for_request(state, headers) else {
            return Some(json_response(401, json!({ "error": "not logged in" })));
        };
        if !can_write_blog_email(state, &email) {
            return Some(json_response(403, json!({ "error": "forbidden" })));
        }
        let Some(body) = parse_body_strict(body_bytes) else {
            return Some(json_response(400, json!({ "error": "bad json" })));
        };

        let title = body
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<&str>>()
            .join(" ");
        let title = mitch_lib::jsval::js_slice_utf16(title.trim(), 140);
        let raw_text = body.get("body").and_then(|v| v.as_str()).unwrap_or("");
        let raw_text = mitch_lib::jsval::js_slice_utf16(raw_text.trim(), 20000);
        let body_html_input = body
            .get("bodyHtml")
            .or_else(|| body.get("html"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let clean_html = sanitize_blog_html(body_html_input, &raw_text);
        let text_from_html = blog_html_to_text(&clean_html);
        let text = if !text_from_html.is_empty() {
            mitch_lib::jsval::js_slice_utf16(&text_from_html, 20000)
        } else {
            raw_text
        };

        let tags = sanitize_blog_tags(body.get("tags").unwrap_or(&json!([])));
        let category =
            sanitize_blog_category(body.get("category").and_then(|v| v.as_str()).unwrap_or(""));
        let cover_image = safe_blog_url(
            body.get("coverImage")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
            true,
        );
        let requested_status = body
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("published")
            .to_lowercase();
        let publish_at = body
            .get("publishAt")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as i64;
        let now = now_millis();
        let status = if requested_status == "draft" {
            "draft"
        } else if requested_status == "scheduled" && publish_at > now + 30_000 {
            "scheduled"
        } else {
            "published"
        };

        if title.chars().count() < 3 {
            return Some(json_response(
                400,
                json!({ "error": "title must be at least 3 characters" }),
            ));
        }
        if text.is_empty() && !clean_html.contains("<img ") {
            return Some(json_response(400, json!({ "error": "body required" })));
        }

        let file = data_file(state, "blog_posts.json");
        let raw = state.store.read_document(&file, json!([]));
        let mut posts = raw.as_array().cloned().unwrap_or_default();
        let staff_writer =
            is_admin_email(&state.store, &email) || is_moderator_email(&state.store, &email);

        let id: String = (0..10)
            .map(|_| format!("{:02x}", rand::random::<u8>()))
            .collect();
        let slug = unique_blog_slug(&posts, &title, "");
        let author_name = blog_author_name(state, &email);

        let mut post = json!({
            "id": id,
            "slug": slug,
            "title": title,
            "body": text,
            "bodyHtml": clean_html,
            "tags": tags,
            "category": category,
            "coverImage": cover_image,
            "featured": staff_writer && body.get("featured").and_then(|v| v.as_bool()).unwrap_or(false),
            "status": status,
            "authorEmail": normalize_email(&email),
            "authorName": author_name,
            "createdAt": now,
            "updatedAt": now,
            "publishAt": if status == "scheduled" { publish_at } else { 0 },
            "publishedAt": if status == "published" { now } else { 0 },
            "revisions": []
        });

        posts.insert(0, post.clone());
        let _ = state
            .store
            .write_document(&file, &Value::Array(posts.clone()));

        if status == "published" {
            notify_blog_subscribers(state, &post);
            if let Some(map) = post.as_object_mut() {
                map.insert("notifiedAt".to_string(), json!(now_millis()));
            }
            posts[0] = post.clone();
            let _ = state.store.write_document(&file, &Value::Array(posts));
        }

        return Some(json_response(
            200,
            json!({ "ok": true, "post": public_blog_post(state, &post, true, &email) }),
        ));
    }

    // Specific post routes under /api/blog/posts/
    if let Some(tail) = path.strip_prefix("/api/blog/posts/") {
        let parts_raw: Vec<&str> = tail.split('/').collect();
        let mut parts: Vec<String> = Vec::new();
        for part in parts_raw {
            let Ok(decoded) = form_urlencoded::parse(part.as_bytes())
                .next()
                .map(|(k, _)| k.into_owned())
                .ok_or(())
            else {
                return Some(json_response(400, json!({ "error": "bad post id" })));
            };
            parts.push(decoded);
        }

        let key = parts.first().map(|s| s.trim()).unwrap_or("");
        let action = parts.get(1).map(|s| s.trim()).filter(|s| !s.is_empty());
        let sub_id = parts.get(2).map(|s| s.trim()).filter(|s| !s.is_empty());

        let Some(email) = authed_email_for_request(state, headers) else {
            return Some(json_response(401, json!({ "error": "not logged in" })));
        };

        let file = data_file(state, "blog_posts.json");
        let raw = state.store.read_document(&file, json!([]));
        let mut posts = raw.as_array().cloned().unwrap_or_default();
        publish_due_blog_posts(state, &mut posts);

        let idx = posts.iter().position(|p| {
            p.get("id").and_then(|v| v.as_str()) == Some(key)
                || p.get("slug").and_then(|v| v.as_str()) == Some(key)
        });
        let Some(post_idx) = idx else {
            return Some(json_response(404, json!({ "error": "post not found" })));
        };
        let mut post = posts[post_idx].clone();

        let norm = normalize_email(&email);
        let admin = is_admin_email(&state.store, &email);
        let staff = admin || is_moderator_email(&state.store, &email);
        let author_email = post
            .get("authorEmail")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let own = normalize_email(author_email) == norm;

        // /api/blog/posts/:key/comments
        if action == Some("comments") {
            let can_moderate = staff || (own && can_write_blog_email(state, &email));
            let comments_file = data_file(state, "blog_comments.json");
            let raw_comments = state.store.read_document(&comments_file, json!({}));
            let mut comments_map = raw_comments.as_object().cloned().unwrap_or_default();
            let post_id = post.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let mut rows = comments_map
                .get(post_id)
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            if *method == Method::GET && sub_id.is_none() {
                let filtered_comments: Vec<Value> = rows
                    .iter()
                    .filter(|comment| {
                        can_moderate
                            || comment.get("status").and_then(|v| v.as_str()) == Some("approved")
                            || normalize_email(
                                comment
                                    .get("authorEmail")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(""),
                            ) == norm
                    })
                    .rev()
                    .take(200)
                    .collect::<Vec<&Value>>()
                    .into_iter()
                    .rev()
                    .map(|c| public_blog_comment(state, c, &email, can_moderate))
                    .collect();
                return Some(json_response(
                    200,
                    json!({ "comments": filtered_comments, "canModerate": can_moderate }),
                ));
            }

            if *method == Method::POST && sub_id.is_none() {
                if let Some(resp) = check_custom_rate_limit(state, headers, "/api/blog/comment") {
                    return Some(resp);
                }
                let Some(body) = parse_body_strict(body_bytes) else {
                    return Some(json_response(400, json!({ "error": "bad json" })));
                };
                let comment_body = body
                    .get("body")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .split_whitespace()
                    .collect::<Vec<&str>>()
                    .join(" ");
                let comment_body = mitch_lib::jsval::js_slice_utf16(comment_body.trim(), 1200);
                if comment_body.chars().count() < 2 {
                    return Some(json_response(400, json!({ "error": "comment required" })));
                }
                let comment_id: String = (0..8)
                    .map(|_| format!("{:02x}", rand::random::<u8>()))
                    .collect();
                let comment = json!({
                    "id": comment_id,
                    "postId": post_id,
                    "authorEmail": norm,
                    "authorName": blog_author_name(state, &email),
                    "body": comment_body,
                    "status": if can_moderate { "approved" } else { "pending" },
                    "ts": now_millis(),
                });
                rows.push(comment.clone());
                if rows.len() > 250 {
                    rows = rows[rows.len() - 250..].to_vec();
                }
                comments_map.insert(post_id.to_string(), Value::Array(rows));
                let _ = state
                    .store
                    .write_document(&comments_file, &Value::Object(comments_map));
                let pending = comment.get("status").and_then(|v| v.as_str()) != Some("approved");
                return Some(json_response(
                    200,
                    json!({
                        "ok": true,
                        "comment": public_blog_comment(state, &comment, &email, can_moderate),
                        "pending": pending,
                    }),
                ));
            }

            let Some(target_sub_id) = sub_id else {
                return Some(json_response(405, json!({ "error": "method not allowed" })));
            };

            let comment_idx = rows
                .iter()
                .position(|r| r.get("id").and_then(|v| v.as_str()) == Some(target_sub_id));
            let Some(c_idx) = comment_idx else {
                return Some(json_response(404, json!({ "error": "comment not found" })));
            };
            let mut comment = rows[c_idx].clone();
            let comment_author = comment
                .get("authorEmail")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let comment_own = normalize_email(comment_author) == norm;

            if *method == Method::PATCH {
                if !can_moderate {
                    return Some(json_response(403, json!({ "error": "forbidden" })));
                }
                let Some(body) = parse_body_strict(body_bytes) else {
                    return Some(json_response(400, json!({ "error": "bad json" })));
                };
                let status = body
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_lowercase();
                if status != "approved" && status != "pending" && status != "rejected" {
                    return Some(json_response(400, json!({ "error": "invalid status" })));
                }
                if let Some(map) = comment.as_object_mut() {
                    map.insert("status".to_string(), json!(status));
                    map.insert("reviewedBy".to_string(), json!(norm));
                    map.insert("reviewedAt".to_string(), json!(now_millis()));
                }
                rows[c_idx] = comment.clone();
                comments_map.insert(post_id.to_string(), Value::Array(rows));
                let _ = state
                    .store
                    .write_document(&comments_file, &Value::Object(comments_map));
                log_admin_action(
                    &state.store,
                    state.data_dir(),
                    &email,
                    "moderate_blog_comment",
                    json!({
                        "postId": post_id,
                        "slug": post.get("slug"),
                        "commentId": target_sub_id,
                        "status": status,
                    }),
                );
                return Some(json_response(
                    200,
                    json!({
                        "ok": true,
                        "comment": public_blog_comment(state, &comment, &email, can_moderate),
                    }),
                ));
            }

            if *method == Method::DELETE {
                if !can_moderate && !comment_own {
                    return Some(json_response(403, json!({ "error": "forbidden" })));
                }
                rows.remove(c_idx);
                comments_map.insert(post_id.to_string(), Value::Array(rows));
                let _ = state
                    .store
                    .write_document(&comments_file, &Value::Object(comments_map));
                log_admin_action(
                    &state.store,
                    state.data_dir(),
                    &email,
                    "delete_blog_comment",
                    json!({
                        "postId": post_id,
                        "slug": post.get("slug"),
                        "commentId": target_sub_id,
                    }),
                );
                return Some(json_response(200, json!({ "ok": true })));
            }

            return Some(json_response(405, json!({ "error": "method not allowed" })));
        }

        // /api/blog/posts/:key/revisions
        if action == Some("revisions") {
            if *method != Method::GET {
                return Some(json_response(405, json!({ "error": "method not allowed" })));
            }
            if !blog_can_manage_post(state, &email, &post) {
                return Some(json_response(403, json!({ "error": "forbidden" })));
            }
            let rev_list = post
                .get("revisions")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let revisions: Vec<Value> = rev_list
                .into_iter()
                .take(25)
                .map(|rev| {
                    json!({
                        "id": rev.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                        "ts": rev.get("ts").and_then(|v| v.as_f64()).unwrap_or(0.0) as u64,
                        "actorEmail": mask_email(rev.get("actorEmail").and_then(|v| v.as_str()).unwrap_or("")),
                        "reason": rev.get("reason").and_then(|v| v.as_str()).unwrap_or("edit"),
                        "title": rev.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                        "status": rev.get("status").and_then(|v| v.as_str()).unwrap_or(""),
                        "category": rev.get("category").and_then(|v| v.as_str()).unwrap_or(""),
                        "tags": sanitize_blog_tags(rev.get("tags").unwrap_or(&json!([]))),
                    })
                })
                .collect();
            return Some(json_response(200, json!({ "revisions": revisions })));
        }

        // /api/blog/posts/:key/restore
        if action == Some("restore") {
            if let Some(resp) = check_custom_rate_limit(state, headers, "/api/blog/write") {
                return Some(resp);
            }
            if *method != Method::POST {
                return Some(json_response(405, json!({ "error": "method not allowed" })));
            }
            if !blog_can_manage_post(state, &email, &post) {
                return Some(json_response(403, json!({ "error": "forbidden" })));
            }
            let Some(body) = parse_body_strict(body_bytes) else {
                return Some(json_response(400, json!({ "error": "bad json" })));
            };
            let revision_id = body
                .get("revisionId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let rev_list = post
                .get("revisions")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let rev_match = rev_list
                .iter()
                .find(|r| r.get("id").and_then(|v| v.as_str()) == Some(revision_id))
                .cloned();
            let Some(revision) = rev_match else {
                return Some(json_response(404, json!({ "error": "revision not found" })));
            };

            push_blog_revision(&mut post, &email, "restore_current");
            let rev_title = revision
                .get("title")
                .or_else(|| post.get("title"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let title = mitch_lib::jsval::js_slice_utf16(rev_title, 140);
            let body_text = revision.get("body").and_then(|v| v.as_str()).unwrap_or("");
            let body_text = mitch_lib::jsval::js_slice_utf16(body_text, 20000);
            let body_html = sanitize_blog_html(
                revision
                    .get("bodyHtml")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
                &body_text,
            );
            let tags = sanitize_blog_tags(revision.get("tags").unwrap_or(&json!([])));
            let category = sanitize_blog_category(
                revision
                    .get("category")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
            );
            let cover_image = safe_blog_url(
                revision
                    .get("coverImage")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
                true,
            );
            let featured = revision
                .get("featured")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let rev_status = revision
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("published");
            let status = if rev_status == "draft" || rev_status == "scheduled" {
                rev_status
            } else {
                "published"
            };
            let publish_at = revision
                .get("publishAt")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as u64;
            let published_at = revision
                .get("publishedAt")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as u64;

            if let Some(map) = post.as_object_mut() {
                map.insert("title".to_string(), json!(title));
                map.insert("body".to_string(), json!(body_text));
                map.insert("bodyHtml".to_string(), json!(body_html));
                map.insert("tags".to_string(), json!(tags));
                map.insert("category".to_string(), json!(category));
                map.insert("coverImage".to_string(), json!(cover_image));
                map.insert("featured".to_string(), json!(featured));
                map.insert("status".to_string(), json!(status));
                map.insert("publishAt".to_string(), json!(publish_at));
                map.insert("publishedAt".to_string(), json!(published_at));
                map.insert("updatedAt".to_string(), json!(now_millis()));
            }

            posts[post_idx] = post.clone();
            let _ = state.store.write_document(&file, &Value::Array(posts));

            log_admin_action(
                &state.store,
                state.data_dir(),
                &email,
                "restore_blog_revision",
                json!({
                    "postId": post.get("id"),
                    "slug": post.get("slug"),
                    "revisionId": revision_id,
                }),
            );

            return Some(json_response(
                200,
                json!({ "ok": true, "post": public_blog_post(state, &post, true, &email) }),
            ));
        }

        if action.is_some() {
            return Some(json_response(404, json!({ "error": "not found" })));
        }

        // GET /api/blog/posts/:key
        if *method == Method::GET {
            if !blog_published(&post) && !(can_write_blog_email(state, &email) && (staff || own)) {
                return Some(json_response(404, json!({ "error": "post not found" })));
            }
            return Some(json_response(
                200,
                json!({
                    "post": public_blog_post(state, &post, true, &email),
                    "canWrite": can_write_blog_email(state, &email),
                }),
            ));
        }

        // PATCH|PUT /api/blog/posts/:key
        if *method == Method::PATCH || *method == Method::PUT {
            if let Some(resp) = check_custom_rate_limit(state, headers, "/api/blog/write") {
                return Some(resp);
            }
            if !can_write_blog_email(state, &email) || (!admin && !own) {
                return Some(json_response(403, json!({ "error": "forbidden" })));
            }
            let Some(body) = parse_body_strict(body_bytes) else {
                return Some(json_response(400, json!({ "error": "bad json" })));
            };

            let previous_status = post
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("published")
                .to_string();
            push_blog_revision(&mut post, &email, "edit");

            if let Some(title_val) = body.get("title").and_then(|v| v.as_str()) {
                let cleaned = title_val
                    .split_whitespace()
                    .collect::<Vec<&str>>()
                    .join(" ");
                let trimmed = mitch_lib::jsval::js_slice_utf16(cleaned.trim(), 140);
                if trimmed.chars().count() < 3 {
                    return Some(json_response(
                        400,
                        json!({ "error": "title must be at least 3 characters" }),
                    ));
                }
                let cur_title = post.get("title").and_then(|v| v.as_str()).unwrap_or("");
                if trimmed != cur_title {
                    let cur_id = post.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let new_slug = unique_blog_slug(&posts, &trimmed, cur_id);
                    if let Some(map) = post.as_object_mut() {
                        map.insert("title".to_string(), json!(trimmed));
                        map.insert("slug".to_string(), json!(new_slug));
                    }
                }
            }

            if let Some(body_val) = body.get("body").and_then(|v| v.as_str()) {
                let raw_text = mitch_lib::jsval::js_slice_utf16(body_val.trim(), 20000);
                let body_html_input = body
                    .get("bodyHtml")
                    .or_else(|| body.get("html"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let clean_html = sanitize_blog_html(body_html_input, &raw_text);
                let text_from_html = blog_html_to_text(&clean_html);
                let text = if !text_from_html.is_empty() {
                    mitch_lib::jsval::js_slice_utf16(&text_from_html, 20000)
                } else {
                    raw_text
                };
                if text.is_empty() && !clean_html.contains("<img ") {
                    return Some(json_response(400, json!({ "error": "body required" })));
                }
                if let Some(map) = post.as_object_mut() {
                    map.insert("body".to_string(), json!(text));
                    map.insert("bodyHtml".to_string(), json!(clean_html));
                }
            } else if body.get("bodyHtml").is_some() || body.get("html").is_some() {
                let body_html_input = body
                    .get("bodyHtml")
                    .or_else(|| body.get("html"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let cur_body = post.get("body").and_then(|v| v.as_str()).unwrap_or("");
                let clean_html = sanitize_blog_html(body_html_input, cur_body);
                let text = mitch_lib::jsval::js_slice_utf16(&blog_html_to_text(&clean_html), 20000);
                if text.is_empty() && !clean_html.contains("<img ") {
                    return Some(json_response(400, json!({ "error": "body required" })));
                }
                if let Some(map) = post.as_object_mut() {
                    map.insert("body".to_string(), json!(text));
                    map.insert("bodyHtml".to_string(), json!(clean_html));
                }
            }

            if let Some(tags_val) = body.get("tags") {
                if let Some(map) = post.as_object_mut() {
                    map.insert("tags".to_string(), json!(sanitize_blog_tags(tags_val)));
                }
            }
            if let Some(cat_val) = body.get("category").and_then(|v| v.as_str()) {
                if let Some(map) = post.as_object_mut() {
                    map.insert(
                        "category".to_string(),
                        json!(sanitize_blog_category(cat_val)),
                    );
                }
            }
            if let Some(cov_val) = body.get("coverImage").and_then(|v| v.as_str()) {
                if let Some(map) = post.as_object_mut() {
                    map.insert(
                        "coverImage".to_string(),
                        json!(safe_blog_url(cov_val, true)),
                    );
                }
            }
            if staff {
                if let Some(featured_val) = body.get("featured").and_then(|v| v.as_bool()) {
                    if let Some(map) = post.as_object_mut() {
                        map.insert("featured".to_string(), json!(featured_val));
                    }
                }
            }

            let now = now_millis();
            if let Some(st_val) = body.get("status").and_then(|v| v.as_str()) {
                let requested = st_val.to_lowercase();
                let publish_at = body
                    .get("publishAt")
                    .or_else(|| post.get("publishAt"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0) as i64;
                let new_status = if requested == "draft" {
                    "draft"
                } else if requested == "scheduled" && publish_at > now + 30_000 {
                    "scheduled"
                } else {
                    "published"
                };
                if let Some(map) = post.as_object_mut() {
                    map.insert("status".to_string(), json!(new_status));
                    map.insert(
                        "publishAt".to_string(),
                        json!(if new_status == "scheduled" {
                            publish_at
                        } else {
                            0
                        }),
                    );
                }
            } else if body.get("publishAt").is_some()
                && post.get("status").and_then(|v| v.as_str()) == Some("scheduled")
            {
                let publish_at = body
                    .get("publishAt")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0) as i64;
                let target_publish_at = if publish_at > now + 30_000 {
                    publish_at
                } else {
                    now
                };
                if let Some(map) = post.as_object_mut() {
                    map.insert("publishAt".to_string(), json!(target_publish_at));
                }
            }

            let cur_status = post
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("published")
                .to_string();
            if let Some(map) = post.as_object_mut() {
                map.insert("updatedAt".to_string(), json!(now));
                if cur_status == "published" && previous_status != "published" {
                    map.insert("publishedAt".to_string(), json!(now));
                }
            }

            posts[post_idx] = post.clone();
            let _ = state
                .store
                .write_document(&file, &Value::Array(posts.clone()));

            if cur_status == "published"
                && previous_status != "published"
                && post.get("notifiedAt").is_none()
            {
                notify_blog_subscribers(state, &post);
                if let Some(map) = post.as_object_mut() {
                    map.insert("notifiedAt".to_string(), json!(now_millis()));
                }
                posts[post_idx] = post.clone();
                let _ = state.store.write_document(&file, &Value::Array(posts));
            }

            return Some(json_response(
                200,
                json!({ "ok": true, "post": public_blog_post(state, &post, true, &email) }),
            ));
        }

        // DELETE /api/blog/posts/:key
        if *method == Method::DELETE {
            if let Some(resp) = check_custom_rate_limit(state, headers, "/api/blog/write") {
                return Some(resp);
            }
            if !can_write_blog_email(state, &email) || (!admin && !own) {
                return Some(json_response(403, json!({ "error": "forbidden" })));
            }
            let deleted_post = post.clone();
            posts.remove(post_idx);
            let _ = state.store.write_document(&file, &Value::Array(posts));
            log_blog_deletion(state, &deleted_post, &email);
            return Some(json_response(200, json!({ "ok": true })));
        }

        return Some(json_response(405, json!({ "error": "method not allowed" })));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unique_blog_slug() {
        let posts = vec![
            json!({ "id": "1", "slug": "hello-world" }),
            json!({ "id": "2", "slug": "hello-world-2" }),
        ];
        assert_eq!(
            unique_blog_slug(&posts, "Hello, World!", ""),
            "hello-world-3"
        );
        assert_eq!(
            unique_blog_slug(&posts, "Hello, World!", "1"),
            "hello-world"
        );
        assert_eq!(
            unique_blog_slug(&posts, "Brand New Post", ""),
            "brand-new-post"
        );
    }

    #[test]
    fn test_blog_published() {
        assert!(blog_published(&json!({ "status": "published" })));
        assert!(blog_published(
            &json!({ "status": "scheduled", "publishAt": 1000 })
        ));
        assert!(!blog_published(&json!({ "status": "draft" })));
        assert!(!blog_published(
            &json!({ "status": "scheduled", "publishAt": now_millis() + 100000 })
        ));
    }

    #[test]
    fn test_push_blog_revision() {
        let mut post = json!({
            "title": "Old Title",
            "body": "Old Body",
            "revisions": []
        });
        push_blog_revision(&mut post, "editor@mitch.pro", "edit");
        let revs = post.get("revisions").and_then(|v| v.as_array()).unwrap();
        assert_eq!(revs.len(), 1);
        assert_eq!(
            revs[0].get("title").and_then(|v| v.as_str()),
            Some("Old Title")
        );
        assert_eq!(revs[0].get("reason").and_then(|v| v.as_str()), Some("edit"));
    }

    use axum::body::to_bytes;
    use axum::http::HeaderValue;

    fn test_state() -> (Arc<AppState>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "mitch_blog_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(dir.join("data"));
        let _ = std::fs::create_dir_all(dir.join("webserver/blog/uploads"));
        let cfg = crate::hosts::SiteConfig::load();
        let cfg = crate::hosts::SiteConfig {
            data_dir: dir.join("data"),
            webroot: dir.join("webserver"),
            base_dir: dir.clone(),
            ..cfg
        };
        let store = Arc::new(
            mitch_lib::data::DataStore::open(&dir, &dir.join("data"))
                .unwrap_or_else(|e| panic!("store: {e}")),
        );
        (Arc::new(AppState::new(cfg, Arc::clone(&store))), dir)
    }

    fn auth_headers(state: &AppState, email: &str) -> HeaderMap {
        let sess = mitch_lib::auth::create_auth_session(
            &state.store,
            &state.id_secret,
            &normalize_email(email),
            email,
            "",
            "",
            false,
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_str(&format!(
                "mitch_session={}; studentId={}",
                sess.token, sess.sid
            ))
            .unwrap(),
        );
        headers
    }

    #[tokio::test]
    async fn test_blog_lifecycle() {
        let (state, _dir) = test_state();
        let author = "writer@student.rjuhsd.us";
        let norm_author = normalize_email(author);

        // Make author a contributor
        let contrib_file = data_file(&state, "blog_contributors.json");
        let _ = state
            .store
            .write_document(&contrib_file, &json!([norm_author.clone()]));

        let headers = auth_headers(&state, author);

        // 1. GET /api/blog/me
        let resp = handle(&state, &Method::GET, "/api/blog/me", &headers, &[])
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let val: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(val.get("canWrite").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            val.get("isContributor").and_then(|v| v.as_bool()),
            Some(true)
        );

        // 2. Subscription toggle
        let sub_get = handle(
            &state,
            &Method::GET,
            "/api/blog/subscription",
            &headers,
            &[],
        )
        .await
        .unwrap();
        assert_eq!(sub_get.status(), 200);
        let bytes = to_bytes(sub_get.into_body(), usize::MAX).await.unwrap();
        let val: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(val.get("enabled").and_then(|v| v.as_bool()), Some(false));

        let sub_post = handle(
            &state,
            &Method::POST,
            "/api/blog/subscription",
            &headers,
            &serde_json::to_vec(&json!({ "enabled": true })).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(sub_post.status(), 200);

        let sub_get2 = handle(
            &state,
            &Method::GET,
            "/api/blog/subscription",
            &headers,
            &[],
        )
        .await
        .unwrap();
        let bytes = to_bytes(sub_get2.into_body(), usize::MAX).await.unwrap();
        let val: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(val.get("enabled").and_then(|v| v.as_bool()), Some(true));

        // 3. Upload image
        let dummy_png_base64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
        let upload_body = json!({
            "mime": "image/png",
            "data": format!("data:image/png;base64,{dummy_png_base64}")
        });
        let upload_resp = handle(
            &state,
            &Method::POST,
            "/api/blog/upload",
            &headers,
            &serde_json::to_vec(&upload_body).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(upload_resp.status(), 200);
        let bytes = to_bytes(upload_resp.into_body(), usize::MAX).await.unwrap();
        let val: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(val
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap()
            .starts_with("/blog/uploads/"));

        // 4. Create blog post
        let create_body = json!({
            "title": "Welcome to our New Blog",
            "body": "This is our inaugural post with interesting updates.",
            "tags": ["welcome", "news"],
            "category": "Announcements"
        });
        let create_resp = handle(
            &state,
            &Method::POST,
            "/api/blog/posts",
            &headers,
            &serde_json::to_vec(&create_body).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(create_resp.status(), 200);
        let bytes = to_bytes(create_resp.into_body(), usize::MAX).await.unwrap();
        let val: Value = serde_json::from_slice(&bytes).unwrap();
        let post = val.get("post").unwrap();
        let post_id = post.get("id").and_then(|v| v.as_str()).unwrap().to_string();
        let post_slug = post
            .get("slug")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        assert_eq!(post_slug, "welcome-to-our-new-blog");

        // 5. GET /api/blog/posts
        let list_resp = handle(&state, &Method::GET, "/api/blog/posts", &headers, &[])
            .await
            .unwrap();
        assert_eq!(list_resp.status(), 200);
        let bytes = to_bytes(list_resp.into_body(), usize::MAX).await.unwrap();
        let list_val: Value = serde_json::from_slice(&bytes).unwrap();
        let posts_arr = list_val.get("posts").and_then(|v| v.as_array()).unwrap();
        assert_eq!(posts_arr.len(), 1);

        // 6. GET /api/blog/posts/:key
        let single_resp = handle(
            &state,
            &Method::GET,
            &format!("/api/blog/posts/{post_slug}"),
            &headers,
            &[],
        )
        .await
        .unwrap();
        assert_eq!(single_resp.status(), 200);

        // 7. PATCH /api/blog/posts/:key
        let patch_body = json!({
            "title": "Welcome to our New Blog Updated",
            "body": "Updated body content text."
        });
        let patch_resp = handle(
            &state,
            &Method::PATCH,
            &format!("/api/blog/posts/{post_id}"),
            &headers,
            &serde_json::to_vec(&patch_body).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(patch_resp.status(), 200);

        // 8. Revisions
        let rev_resp = handle(
            &state,
            &Method::GET,
            &format!("/api/blog/posts/{post_id}/revisions"),
            &headers,
            &[],
        )
        .await
        .unwrap();
        assert_eq!(rev_resp.status(), 200);
        let bytes = to_bytes(rev_resp.into_body(), usize::MAX).await.unwrap();
        let rev_val: Value = serde_json::from_slice(&bytes).unwrap();
        let revs = rev_val.get("revisions").and_then(|v| v.as_array()).unwrap();
        assert_eq!(revs.len(), 1);
        let rev_id = revs[0]
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        // 9. Restore revision
        let restore_body = json!({ "revisionId": rev_id });
        let restore_resp = handle(
            &state,
            &Method::POST,
            &format!("/api/blog/posts/{post_id}/restore"),
            &headers,
            &serde_json::to_vec(&restore_body).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(restore_resp.status(), 200);
        let bytes = to_bytes(restore_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let rest_val: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            rest_val
                .get("post")
                .unwrap()
                .get("title")
                .and_then(|v| v.as_str()),
            Some("Welcome to our New Blog")
        );

        // 10. Comments
        let comment_body = json!({ "body": "Great inaugural article!" });
        let comment_post = handle(
            &state,
            &Method::POST,
            &format!("/api/blog/posts/{post_id}/comments"),
            &headers,
            &serde_json::to_vec(&comment_body).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(comment_post.status(), 200);
        let bytes = to_bytes(comment_post.into_body(), usize::MAX)
            .await
            .unwrap();
        let comm_val: Value = serde_json::from_slice(&bytes).unwrap();
        let comm_id = comm_val
            .get("comment")
            .unwrap()
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        let comment_list = handle(
            &state,
            &Method::GET,
            &format!("/api/blog/posts/{post_id}/comments"),
            &headers,
            &[],
        )
        .await
        .unwrap();
        assert_eq!(comment_list.status(), 200);

        let comment_patch = handle(
            &state,
            &Method::PATCH,
            &format!("/api/blog/posts/{post_id}/comments/{comm_id}"),
            &headers,
            &serde_json::to_vec(&json!({ "status": "approved" })).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(comment_patch.status(), 200);

        let comment_del = handle(
            &state,
            &Method::DELETE,
            &format!("/api/blog/posts/{post_id}/comments/{comm_id}"),
            &headers,
            &[],
        )
        .await
        .unwrap();
        assert_eq!(comment_del.status(), 200);

        // 11. Delete blog post
        let del_resp = handle(
            &state,
            &Method::DELETE,
            &format!("/api/blog/posts/{post_id}"),
            &headers,
            &[],
        )
        .await
        .unwrap();
        assert_eq!(del_resp.status(), 200);

        let del_log_file = data_file(&state, "blog_delete_log.json");
        let del_logs = state.store.read_document(&del_log_file, json!([]));
        assert_eq!(del_logs.as_array().unwrap().len(), 1);
    }
}
