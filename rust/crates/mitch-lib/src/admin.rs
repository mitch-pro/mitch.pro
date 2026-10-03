//! Shared admin-layer helpers — port of server.js's admin support code:
//! logAdminAction (3149-3159), maskEmail (1352), admin passphrase store
//! (2602-2635), publicSessionId (3144), publicProfileReports (3161),
//! buildAdvancedAdminData (3180-3274), blog contributors (6253), moderator
//! panel config (6596-6613), the moderator action vocabulary (6615-6732),
//! moderator request store (6734-6786), and canGrantPremium* (7008-7024).

use crate::auth::normalize_email;
use crate::data::DataStore;
use serde_json::{json, Value};
use std::path::Path;

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `logAdminAction(actor, action, details)` — admin_actions.json, newest
/// first via unshift, capped at 1000.
pub fn log_admin_action(
    store: &DataStore,
    data_dir: &Path,
    actor: &str,
    action: &str,
    details: Value,
) {
    let file = data_dir.join("admin_actions.json");
    let logs = store
        .read_document(&file, json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut next = Vec::with_capacity(logs.len() + 1);
    next.push(json!({
        "id": crate::crypto::random_bytes_hex(8),
        "ts": now_millis(),
        "actor": if actor.is_empty() { "admin" } else { actor },
        "action": action,
        "details": details,
    }));
    next.extend(logs.into_iter().take(999));
    let _ = store.write_document(&file, &json!(next));
}

/// `logCheat(email, game, details, ip)` — server.js:665-679. cheat_logs.json,
/// newest first via unshift, capped at 1000.
pub fn log_cheat(
    store: &DataStore,
    data_dir: &Path,
    email: &str,
    game: &str,
    details: &str,
    ip: &str,
) {
    let file = data_dir.join("cheat_logs.json");
    let logs = store
        .read_document(&file, json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut next = Vec::with_capacity(logs.len() + 1);
    next.push(json!({
        "email": if email.is_empty() { "unknown" } else { email },
        "game": if game.is_empty() { "unknown" } else { game },
        "details": if details.is_empty() { "" } else { details },
        "ts": now_millis(),
        "ip": if ip.is_empty() { "unknown" } else { ip },
    }));
    next.extend(logs.into_iter().take(999));
    let _ = store.write_document(&file, &json!(next));
}

/// `maskEmail(email)` — server.js:1352. Despite the name this is an IDENTITY
/// function (the JS predates masking); empty input returns 'anonymous'.
pub fn mask_email(email: &str) -> String {
    if email.is_empty() {
        return "anonymous".to_string();
    }
    email.to_string()
}

/// `loadAdminPassphrase()` — data/admin_passphrase.json map.
pub fn load_admin_passphrase(store: &DataStore, data_dir: &Path) -> Value {
    store.read_document(&data_dir.join("admin_passphrase.json"), json!({}))
}

/// `saveAdminPassphraseForUser(norm, entry)`.
pub fn save_admin_passphrase_for_user(
    store: &DataStore,
    data_dir: &Path,
    norm: &str,
    entry: Value,
) {
    let mut data = load_admin_passphrase(store, data_dir);
    if let Some(map) = data.as_object_mut() {
        map.insert(norm.to_string(), entry);
    }
    let _ = store.write_document(&data_dir.join("admin_passphrase.json"), &data);
}

/// `verifyAdminPassphraseRaw(sid, passphrase)` — per-user hash with the
/// admin@mitch.pro fallback; argon2 PHC verify. Returns false on any failure.
pub fn verify_admin_passphrase_raw(
    store: &DataStore,
    id_secret: &[u8],
    data_dir: &Path,
    sid: &str,
    passphrase: &str,
) -> bool {
    let email =
        crate::auth::email_from_sid(store, id_secret, sid).unwrap_or_else(|| "admin".to_string());
    let norm = normalize_email(&email);
    let data = load_admin_passphrase(store, data_dir);
    let mut hash = data
        .get(&norm)
        .and_then(|e| e.get("hash"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if hash.is_empty() {
        hash = data
            .get("admin@mitch.pro")
            .and_then(|e| e.get("hash"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
    }
    if hash.is_empty() {
        return false;
    }
    let pass = passphrase.trim();
    if pass.is_empty() {
        return false;
    }
    crate::crypto::argon2_verify(&hash, pass)
}

/// `publicSessionId(id)` — sha256(id).hex[0..12] or 'unknown'.
pub fn public_session_id(id: &str) -> String {
    if id.is_empty() {
        return "unknown".to_string();
    }
    crate::crypto::sha256_hex(id.as_bytes())[..12].to_string()
}

/// `publicProfileReports(limit)` — masked profile-report view.
pub fn public_profile_reports(store: &DataStore, data_dir: &Path, limit: usize) -> Vec<Value> {
    let reports = store
        .read_document(&data_dir.join("profile_reports.json"), json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let start = reports.len().saturating_sub(limit);
    reports[start..]
        .iter()
        .rev()
        .map(|report| {
            json!({
                "id": report.get("id").map(|v| v.as_str().unwrap_or("").to_string()).unwrap_or_default(),
                "ts": report.get("ts").and_then(|v| v.as_i64()).unwrap_or(0),
                "status": report.get("status").and_then(|v| v.as_str()).unwrap_or("Needs review"),
                "reporter": mask_email(report.get("reporterEmail").and_then(|v| v.as_str()).unwrap_or("")),
                "target": mask_email(report.get("targetEmail").and_then(|v| v.as_str()).unwrap_or("")),
                "targetHandle": report.get("targetHandle").map(|v| v.as_str().unwrap_or("").to_string()).unwrap_or_default(),
                "reason": report.get("reason").map(|v| v.as_str().unwrap_or("").chars().take(140).collect::<String>()).unwrap_or_default(),
                "details": report.get("details").map(|v| v.as_str().unwrap_or("").chars().take(800).collect::<String>()).unwrap_or_default(),
                "resolvedBy": report.get("resolvedBy").and_then(|v| v.as_str()).map(mask_email).unwrap_or_default(),
                "resolvedAt": report.get("resolvedAt").and_then(|v| v.as_i64()).unwrap_or(0),
                "resolution": report.get("resolution").map(|v| v.as_str().unwrap_or("").chars().take(300).collect::<String>()).unwrap_or_default(),
            })
        })
        .collect()
}

/// `buildAdvancedAdminData()` — server.js:3180-3274, the admin dashboard
/// payload. Pure document reads (websockets not needed for any field).
pub fn build_advanced_admin_data(store: &DataStore, data_dir: &Path) -> Value {
    let now = now_millis();

    let admin_actions_raw = store
        .read_document(&data_dir.join("admin_actions.json"), json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let admin_actions: Vec<Value> = admin_actions_raw
        .iter()
        .take(250)
        .map(|entry| {
            json!({
                "id": entry.get("id").map(|v| v.as_str().unwrap_or("").to_string()).unwrap_or_default(),
                "ts": entry.get("ts").and_then(|v| v.as_i64()).unwrap_or(0),
                "actor": entry.get("actor").and_then(|v| v.as_str()).unwrap_or("admin"),
                "action": entry.get("action").and_then(|v| v.as_str()).unwrap_or("admin_action"),
                "details": entry.get("details").cloned().unwrap_or(json!({})),
            })
        })
        .collect();

    let chat_reports_raw = store
        .read_document(&data_dir.join("chat_reports.json"), json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default();
    let chat_reports_all: Vec<Value> = chat_reports_raw
        .iter()
        .rev()
        .take(150)
        .map(|report| {
            json!({
                "type": "chat",
                "ts": report.get("ts").and_then(|v| v.as_i64()).unwrap_or(0),
                "reportedBy": report.get("reportedBy").or_else(|| report.get("reporter")).and_then(|v| v.as_str()).unwrap_or(""),
                "reason": report.get("reason").and_then(|v| v.as_str()).unwrap_or("Chat report"),
                "context": report.get("context").cloned().unwrap_or(json!([])),
                "status": report.get("status").and_then(|v| v.as_str()).unwrap_or("Needs review"),
            })
        })
        .collect();
    let profile_reports = public_profile_reports(store, data_dir, 250);

    // sentNotifications: admin_notice rows in coin_gifts.json, deduped on
    // batchId+targetEmail, newest first.
    let gifts = store.read_document(&data_dir.join("coin_gifts.json"), json!({}));
    let mut sent_notifications: Vec<Value> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Some(map) = gifts.as_object() {
        for (target_email, notices) in map {
            let Some(arr) = notices.as_array() else {
                continue;
            };
            for notice in arr {
                if notice.get("kind").and_then(|v| v.as_str()) != Some("admin_notice") {
                    continue;
                }
                let key = format!(
                    "{}:{}",
                    notice
                        .get("batchId")
                        .and_then(|v| v.as_str())
                        .unwrap_or_else(|| notice.get("id").and_then(|v| v.as_str()).unwrap_or("")),
                    target_email
                );
                if !seen.insert(key) {
                    continue;
                }
                sent_notifications.push(json!({
                    "id": notice.get("id").map(|v| v.as_str().unwrap_or("").to_string()).unwrap_or_default(),
                    "batchId": notice.get("batchId").map(|v| v.as_str().unwrap_or("").to_string()).unwrap_or_default(),
                    "targetEmail": target_email,
                    "title": notice.get("title").and_then(|v| v.as_str()).unwrap_or("Admin notification"),
                    "from": notice.get("from").and_then(|v| v.as_str()).unwrap_or("admin"),
                    "source": notice.get("source").and_then(|v| v.as_str()).unwrap_or("mitchdog.com"),
                    "ts": notice.get("ts").and_then(|v| v.as_i64()).unwrap_or(0),
                    "read": notice.get("read").and_then(|v| v.as_bool()).unwrap_or(false),
                }));
            }
        }
    }
    sent_notifications.sort_by(|a, b| {
        b.get("ts")
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            .cmp(&a.get("ts").and_then(|v| v.as_i64()).unwrap_or(0))
    });

    let user_stats = store.read_document(&data_dir.join("user_stats.json"), json!({}));
    let active_users: Vec<Value> = user_stats
        .as_object()
        .map(|m| {
            let mut rows: Vec<(i64, Value)> = m
                .iter()
                .filter_map(|(email, info)| {
                    let last = info.get("last_active_at").and_then(|v| v.as_i64())?;
                    (last > 0 && now - last < 2 * 60 * 1000)
                        .then(|| (last, json!({ "email": email, "lastActiveAt": last })))
                })
                .collect();
            rows.sort_by_key(|a| std::cmp::Reverse(a.0));
            rows.into_iter().take(50).map(|(_, v)| v).collect()
        })
        .unwrap_or_default();

    let blacklist = store.read_document(&data_dir.join("blacklist.json"), json!({}));
    let mut banned_accounts: Vec<Value> = blacklist
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(email, info)| {
                    json!({
                        "email": email,
                        "reason": info.get("reason").and_then(|v| v.as_str()).unwrap_or("Banned by admin"),
                        "bannedAt": info.get("banned_at").or_else(|| info.get("blacklisted_at")).and_then(|v| v.as_i64()).unwrap_or(0),
                        "by": info.get("by").or_else(|| info.get("admin")).and_then(|v| v.as_str()).unwrap_or("admin"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    banned_accounts.sort_by(|a, b| {
        b.get("bannedAt")
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            .cmp(&a.get("bannedAt").and_then(|v| v.as_i64()).unwrap_or(0))
    });

    let chat_reports_top: Vec<Value> = {
        let mut sorted = chat_reports_all.clone();
        sorted.sort_by(|a, b| {
            b.get("ts")
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                .cmp(&a.get("ts").and_then(|v| v.as_i64()).unwrap_or(0))
        });
        sorted.into_iter().take(250).collect()
    };
    let cheat_logs: Vec<Value> = store
        .read_document(&data_dir.join("cheat_logs.json"), json!([]))
        .as_array()
        .map(|a| a.iter().take(250).cloned().collect())
        .unwrap_or_default();
    let cheat_logs_count = store
        .read_document(&data_dir.join("cheat_logs.json"), json!([]))
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);

    json!({
        "generatedAt": now,
        "policy": {
            "adminActions": "Admin actions are logged so privileged changes can be reviewed later.",
            "chatReports": "Staff should use explicit user reports for moderation context.",
            "consent": "This monitoring should match the site terms and be limited to authorized admins with a security or moderation need.",
        },
        "stats": {
            "adminActions": admin_actions.len(),
            "reports": chat_reports_all.len(),
            "profileReports": profile_reports.len(),
            "sentNotifications": sent_notifications.len(),
            "bannedAccounts": banned_accounts.len(),
            "activeUsers": active_users.len(),
            "cheatLogs": cheat_logs_count,
        },
        "activeUsers": active_users,
        "bannedAccounts": banned_accounts,
        "adminActions": admin_actions,
        "reports": chat_reports_top,
        "canvasReports": [],
        "chatReports": chat_reports_top,
        "profileReports": profile_reports,
        "sentNotifications": sent_notifications.iter().take(250).cloned().collect::<Vec<_>>(),
        "heatmap": store.read_document(&data_dir.join("heatmap.json"), json!({})),
        "cheatLogs": cheat_logs,
    })
}

/// `blogContributorEmails()` — accepts an array or an `{email: active}` map.
pub fn blog_contributor_emails(store: &DataStore, data_dir: &Path) -> Vec<String> {
    let raw = store.read_document(&data_dir.join("blog_contributors.json"), json!([]));
    match &raw {
        Value::Array(a) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .filter(|s| !s.is_empty())
            .collect(),
        Value::Object(o) => o
            .iter()
            .filter(|(_, active)| active.as_bool() != Some(false))
            .map(|(email, _)| email.clone())
            .collect(),
        _ => Vec::new(),
    }
}

/// `isBlogContributorEmail(email)` — used by /api/me.
pub fn is_blog_contributor_email(store: &DataStore, data_dir: &Path, email: &str) -> bool {
    if email.is_empty() {
        return false;
    }
    let norm = normalize_email(email);
    blog_contributor_emails(store, data_dir)
        .iter()
        .any(|c| normalize_email(c) == norm)
}

/// `moderatorPanelConfig()` + `sanitizeModeratorPanelLinks`.
pub fn sanitize_moderator_panel_links(links: &Value) -> Vec<Value> {
    let Some(arr) = links.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .map(|link| {
            json!({
                "label": link.get("label").and_then(|v| v.as_str()).unwrap_or("").trim().chars().take(60).collect::<String>(),
                "href": link.get("href").and_then(|v| v.as_str()).unwrap_or("").trim().chars().take(180).collect::<String>(),
            })
        })
        .filter(|link| {
            let label = link.get("label").and_then(|v| v.as_str()).unwrap_or("");
            let href = link.get("href").and_then(|v| v.as_str()).unwrap_or("");
            !label.is_empty()
                && href.starts_with('/')
                && !href.starts_with("//")
                && !href.contains('\\')
        })
        .take(20)
        .collect()
}

pub fn moderator_panel_config(store: &DataStore, data_dir: &Path) -> Value {
    let raw = store.read_document(
        &data_dir.join("moderator_panel.json"),
        json!({ "links": [] }),
    );
    let links = raw.get("links").cloned().unwrap_or(json!([]));
    json!({ "links": sanitize_moderator_panel_links(&links) })
}

// ── Moderator action requests ────────────────────────────────────────────────

/// `MODERATOR_ACTION_LABELS` — verbatim.
pub const MODERATOR_ACTION_LABELS: &[(&str, &str)] = &[
    ("grant_premium", "Grant premium"),
    ("revoke_premium", "Revoke premium"),
    ("send_notification", "Send notification"),
    ("unsend_notification", "Unsend notification"),
    ("gift_coins", "Give coins"),
    ("burn_coins", "Burn coins"),
    ("economy_multiplier", "Update coin multiplier"),
    ("broadcast", "Global broadcast"),
    ("shadow_ban", "Toggle shadow-ban"),
    ("ban_account", "Ban account"),
    ("unban_account", "Unban account"),
    ("casino_rig", "Set casino rig chance"),
    ("casino_toggle", "Toggle casino"),
    ("prox_block", "Block mitch.prox domain"),
    ("content_mirror", "Update mirror link"),
    ("content_featured", "Set featured game"),
    ("moderator_role", "Update moderator role"),
    ("moderator_panel", "Update moderator panel"),
    ("admin_role", "Update administrator role"),
];

pub fn moderator_action_label(action: &str) -> Option<&'static str> {
    MODERATOR_ACTION_LABELS
        .iter()
        .find(|(k, _)| *k == action)
        .map(|(_, v)| *v)
}

/// `MODERATOR_ACTION_BY_URL` — verbatim.
pub fn moderator_action_by_url(url: &str) -> &'static str {
    match url {
        "/api/admin/grant-premium" => "grant_premium",
        "/api/admin/revoke-premium" => "revoke_premium",
        "/api/admin/send-notification" => "send_notification",
        "/api/admin/unsend-notification" => "unsend_notification",
        "/api/admin/gift-coins" => "gift_coins",
        "/api/admin/economy/burn" => "burn_coins",
        "/api/admin/economy/multiplier" => "economy_multiplier",
        "/api/admin/broadcast" => "broadcast",
        "/api/admin/shadow-ban" => "shadow_ban",
        "/api/admin/restricted-mode" => "shadow_ban",
        "/api/admin/ban-account" => "ban_account",
        "/api/admin/unban-account" => "unban_account",
        "/api/admin/casino/rig" => "casino_rig",
        "/api/admin/casino/toggle" => "casino_toggle",
        "/api/admin/prox/block" => "prox_block",
        "/api/admin/content/mirror" => "content_mirror",
        "/api/admin/content/featured" => "content_featured",
        "/api/admin/moderators" => "moderator_role",
        "/api/admin/moderator-panel" => "moderator_panel",
        "/api/admin/admins" => "admin_role",
        _ => "",
    }
}

/// `moderatorActionFromBody(body)` — explicit action or targetUrl fallback.
pub fn moderator_action_from_body(body: &Value) -> String {
    let explicit = body
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if !explicit.is_empty() && moderator_action_label(explicit).is_some() {
        return explicit.to_string();
    }
    let target_url = body
        .get("targetUrl")
        .or_else(|| body.get("url"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    moderator_action_by_url(target_url).to_string()
}

/// Error carrying an HTTP status (mirrors `adminActionError`).
#[derive(Debug, Clone)]
pub struct AdminActionError {
    pub status: u16,
    pub message: String,
}

impl AdminActionError {
    pub fn new(status: u16, message: &str) -> Self {
        Self {
            status,
            message: message.to_string(),
        }
    }
}

fn canonical_email(raw: Option<&Value>, label: &str) -> Result<String, AdminActionError> {
    let email = raw
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase()
        .trim()
        .to_string();
    let ok = {
        let mut parts = email.split('@');
        let local = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("");
        !local.is_empty()
            && !rest.is_empty()
            && rest.contains('.')
            && !email.contains(|c: char| c.is_whitespace())
            && parts.next().is_none()
    };
    if !ok {
        return Err(AdminActionError::new(
            400,
            &format!("valid {label} required"),
        ));
    }
    Ok(email)
}

fn trim_slice(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// `cleanModeratorActionPayload(action, payload)` — canonicalizes a payload,
/// rejecting invalid input with a 400.
pub fn clean_moderator_action_payload(
    action: &str,
    payload: &Value,
) -> Result<Value, AdminActionError> {
    if moderator_action_label(action).is_none() {
        return Err(AdminActionError::new(400, "unknown moderator action"));
    }
    let p = if payload.is_object() {
        payload
    } else {
        &json!({})
    };
    let s = |k: &str| p.get(k).and_then(|v| v.as_str()).unwrap_or("");
    match action {
        "grant_premium" => Ok(json!({
            "targetEmail": canonical_email(p.get("targetEmail"), "target email")?,
            "reason": trim_slice(s("reason").trim(), 200),
        })),
        "revoke_premium" => Ok(json!({
            "email": canonical_email(
                p.get("email").or_else(|| p.get("targetEmail")),
                "email"
            )?,
            "reason": trim_slice(s("reason").trim(), 200),
        })),
        "send_notification" => {
            let all_users = p.get("allUsers").and_then(|v| v.as_bool()) == Some(true);
            Ok(json!({
                "allUsers": all_users,
                "targetEmail": if all_users { json!("") } else { json!(canonical_email(p.get("targetEmail"), "target email")?) },
                "title": trim_slice(s("title").trim(), 80),
                "message": trim_slice(s("message").trim(), 1000),
            }))
        }
        "unsend_notification" => Ok(json!({
            "id": trim_slice(s("id").trim(), 80),
            "batchId": trim_slice(s("batchId").trim(), 80),
        })),
        "gift_coins" => Ok(json!({
            "targetEmail": canonical_email(p.get("targetEmail"), "target email")?,
            "amount": p.get("amount").and_then(|v| v.as_f64()).unwrap_or(f64::NAN),
            "reason": trim_slice(s("reason").trim(), 160),
        })),
        "burn_coins" => Ok(json!({
            "email": canonical_email(p.get("email"), "email")?,
            "amount": p.get("amount").and_then(|v| v.as_f64()).unwrap_or(f64::NAN),
        })),
        "economy_multiplier" => Ok(json!({
            "multiplier": p.get("multiplier").and_then(|v| v.as_f64()).unwrap_or(f64::NAN),
        })),
        "broadcast" => Ok(json!({
            "msg": trim_slice(s("msg").trim(), 500),
            "type": trim_slice(s("type").trim(), 30),
        })),
        "shadow_ban" => Ok(json!({ "email": canonical_email(p.get("email"), "email")? })),
        "ban_account" => Ok(json!({
            "email": canonical_email(p.get("email"), "email")?,
            "reason": trim_slice(s("reason").trim(), 200),
        })),
        "unban_account" => Ok(json!({ "email": canonical_email(p.get("email"), "email")? })),
        "casino_rig" => Ok(json!({
            "chance": p.get("chance").and_then(|v| v.as_f64()).unwrap_or(f64::NAN),
        })),
        "casino_toggle" => Ok(json!({})),
        "prox_block" => {
            let domain_raw = s("domain").to_lowercase().trim().to_string();
            let no_scheme = domain_raw
                .strip_prefix("http://")
                .or_else(|| domain_raw.strip_prefix("https://"))
                .unwrap_or(&domain_raw);
            let domain: String = no_scheme
                .split(['/', '?', '#'])
                .next()
                .unwrap_or("")
                .chars()
                .take(180)
                .collect();
            Ok(json!({ "domain": domain }))
        }
        "content_mirror" => Ok(json!({ "url": trim_slice(s("url").trim(), 500) })),
        "content_featured" => Ok(json!({ "href": trim_slice(s("href").trim(), 240) })),
        "moderator_role" => Ok(json!({
            "email": canonical_email(p.get("email"), "email")?,
            "active": p.get("active").and_then(|v| v.as_bool()) == Some(true),
        })),
        "moderator_panel" => Ok(
            json!({ "links": sanitize_moderator_panel_links(p.get("links").unwrap_or(&json!([]))) }),
        ),
        _ => Err(AdminActionError::new(400, "unknown moderator action")),
    }
}

/// `loadModeratorRequests()` / `saveModeratorRequests()`.
pub fn load_moderator_requests(store: &DataStore, data_dir: &Path) -> Vec<Value> {
    store
        .read_document(&data_dir.join("moderator_requests.json"), json!([]))
        .as_array()
        .cloned()
        .unwrap_or_default()
}

pub fn save_moderator_requests(store: &DataStore, data_dir: &Path, requests: &[Value]) {
    let capped: Vec<Value> = requests.iter().take(1000).cloned().collect();
    let _ = store.write_document(&data_dir.join("moderator_requests.json"), &json!(capped));
}

/// `publicModeratorRequest(req)`.
pub fn public_moderator_request(req: &Value) -> Value {
    let action = req.get("action").and_then(|v| v.as_str()).unwrap_or("");
    json!({
        "id": req.get("id").and_then(|v| v.as_str()).unwrap_or(""),
        "requestedBy": req.get("requestedBy").and_then(|v| v.as_str()).unwrap_or(""),
        "requestedByEmail": req.get("requestedByEmail").and_then(|v| v.as_str()).unwrap_or(""),
        "action": action,
        "label": req.get("label").and_then(|v| v.as_str())
            .unwrap_or(moderator_action_label(action).unwrap_or("Moderator action")),
        "payload": req.get("payload").cloned().unwrap_or(json!({})),
        "status": req.get("status").and_then(|v| v.as_str()).unwrap_or("pending"),
        "requestedAt": req.get("requestedAt").and_then(|v| v.as_i64()).unwrap_or(0),
        "resolvedAt": req.get("resolvedAt").and_then(|v| v.as_i64()).unwrap_or(0),
        "resolvedBy": req.get("resolvedBy").and_then(|v| v.as_str()).unwrap_or(""),
        "note": req.get("note").and_then(|v| v.as_str()).unwrap_or(""),
        "result": req.get("result").cloned().unwrap_or(Value::Null),
        "error": req.get("error").and_then(|v| v.as_str()).unwrap_or(""),
    })
}

/// `createModeratorActionRequest(sid, body)` — validates the action + payload,
/// unshifts a pending entry, logs it.
pub fn create_moderator_action_request(
    store: &DataStore,
    id_secret: &[u8],
    data_dir: &Path,
    sid: &str,
    body: &Value,
) -> Result<Value, AdminActionError> {
    let requested_by_email = crate::auth::email_from_sid(store, id_secret, sid)
        .unwrap_or_else(|| "moderator".to_string());
    let action = moderator_action_from_body(body);
    if action.is_empty() {
        return Err(AdminActionError::new(400, "unknown moderator action"));
    }
    let empty = json!({});
    let payload_in = body.get("payload").unwrap_or(&empty);
    let payload = clean_moderator_action_payload(&action, payload_in)?;
    let label = body
        .get("label")
        .and_then(|v| v.as_str())
        .map(|l| trim_slice(l.trim(), 100))
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| {
            moderator_action_label(&action)
                .unwrap_or("Moderator action")
                .to_string()
        });
    let entry = json!({
        "id": crate::crypto::random_bytes_hex(10),
        "requestedBy": public_session_id(sid),
        "requestedByEmail": requested_by_email,
        "action": action,
        "label": label,
        "payload": payload,
        "status": "pending",
        "requestedAt": now_millis(),
        "resolvedAt": 0,
        "resolvedBy": "",
        "note": "",
        "result": null,
        "error": "",
    });
    let mut requests = load_moderator_requests(store, data_dir);
    requests.insert(0, entry.clone());
    save_moderator_requests(store, data_dir, &requests);
    log_admin_action(
        store,
        data_dir,
        &requested_by_email,
        "moderator_request",
        json!({
            "id": entry.get("id").cloned().unwrap_or_default(),
            "action": action,
            "label": label,
        }),
    );
    Ok(public_moderator_request(&entry))
}

// ── Premium granting ─────────────────────────────────────────────────────────

/// `can_grant_premium_email(email)` — checks whether an email is an authorized owner, co-owner, or admin.
pub fn can_grant_premium_email(store: &DataStore, email: &str) -> bool {
    if email.is_empty() || !crate::auth::is_admin_email(store, email) {
        return false;
    }
    let norm = normalize_email(email);
    crate::auth::is_owner_email(store, &norm)
        || crate::auth::is_co_owner_email(store, &norm)
        || crate::auth::admin_member_emails(store)
            .iter()
            .any(|a| normalize_email(a) == norm)
}

/// `canGrantPremiumId(sid)`.
pub fn can_grant_premium_id(store: &DataStore, id_secret: &[u8], sid: &str) -> bool {
    if sid.is_empty() || !crate::auth::is_admin_id(store, id_secret, sid, false) {
        return false;
    }
    let email = crate::auth::email_from_sid(store, id_secret, sid).unwrap_or_default();
    can_grant_premium_email(store, &email)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_store(tag: &str) -> (std::path::PathBuf, std::path::PathBuf, DataStore) {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-admin-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        let store = DataStore::open(&base, &base.join("data")).unwrap();
        // Helpers take the DATA dir; tests return both for convenience.
        (base.clone(), base.join("data"), store)
    }

    #[test]
    fn log_admin_action_unshifts_and_caps() {
        let (base, data, store) = temp_store("log");
        // Seed the cap boundary directly (a 1005-iteration loop costs ~35s on
        // a slow-disk dev box for the same assertion).
        let seeded: Vec<Value> = (100..1100)
            .rev()
            .map(|i| json!({ "id": i.to_string(), "ts": i, "actor": "a", "action": "x", "details": {} }))
            .collect();
        store
            .write_document(&data.join("admin_actions.json"), &json!(seeded))
            .unwrap();
        for i in 0..5 {
            log_admin_action(
                &store,
                &data,
                "admin@mitch.pro",
                "test_action",
                json!({ "i": i }),
            );
        }
        let logs = store
            .read_document(&base.join("data/admin_actions.json"), json!([]))
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(logs.len(), 1000);
        assert_eq!(
            logs[0]
                .get("details")
                .and_then(|d| d.get("i"))
                .and_then(|v| v.as_i64()),
            Some(4)
        );
        assert_eq!(
            logs[0].get("actor").and_then(|v| v.as_str()),
            Some("admin@mitch.pro")
        );
        // The five oldest seeded entries (ts 100-104) are dropped by the cap.
        assert_eq!(logs[999].get("ts").and_then(|v| v.as_i64()), Some(105));
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn mask_email_matches_js() {
        // maskEmail is an identity function in the JS (server.js:1352).
        assert_eq!(
            mask_email("mitchfogler@student.rjuhsd.us"),
            "mitchfogler@student.rjuhsd.us"
        );
        assert_eq!(mask_email("ab@c.com"), "ab@c.com");
        assert_eq!(mask_email(""), "anonymous");
    }

    #[test]
    fn clean_payload_validates() {
        let ok = clean_moderator_action_payload(
            "grant_premium",
            &json!({ "targetEmail": "user@mitch.pro", "reason": "because" }),
        )
        .unwrap();
        assert_eq!(
            ok.get("targetEmail").and_then(|v| v.as_str()),
            Some("user@mitch.pro")
        );
        let bad = clean_moderator_action_payload(
            "gift_coins",
            &json!({ "targetEmail": "not-an-email", "amount": 5 }),
        );
        assert_eq!(bad.unwrap_err().status, 400);
        let unknown = clean_moderator_action_payload("nonsense", &json!({}));
        assert_eq!(unknown.unwrap_err().message, "unknown moderator action");
    }

    #[test]
    fn moderator_panel_links_sanitize() {
        let links = json!([
            { "label": "Wiki", "href": "/wiki/" },
            { "label": "", "href": "/x" },
            { "label": "Evil", "href": "//evil.com" },
            { "label": "Back", "href": "\\evil" },
            { "label": "Abs", "href": "https://x.com" },
        ]);
        let out = sanitize_moderator_panel_links(&links);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].get("label").and_then(|v| v.as_str()), Some("Wiki"));
    }

    #[test]
    fn advanced_data_shapes() {
        let (base, data, store) = temp_store("advanced");
        log_admin_action(
            &store,
            &data,
            "admin@mitch.pro",
            "view_admin_tools",
            json!({}),
        );
        store
            .write_document(
                &base.join("data/coin_gifts.json"),
                &json!({ "user@mitch.pro": [{
                    "kind": "admin_notice", "id": "n1", "batchId": "b1",
                    "title": "Hi", "message": "m", "from": "admin@mitch.pro",
                    "ts": 5, "read": false,
                }]}),
            )
            .unwrap();
        let advanced = build_advanced_admin_data(&store, &data);
        assert_eq!(
            advanced
                .get("stats")
                .and_then(|s| s.get("adminActions"))
                .and_then(|v| v.as_i64()),
            Some(1)
        );
        assert_eq!(
            advanced
                .get("sentNotifications")
                .and_then(|v| v.as_array())
                .map(|a| a.len()),
            Some(1)
        );
        std::fs::remove_dir_all(base).ok();
    }
}
