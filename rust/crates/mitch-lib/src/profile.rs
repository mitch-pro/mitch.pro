//! Profile/member identity helpers (server.js:2326-2364, 6043-6174).
//! Byte-compatible ports of `normalizeUsername`, `defaultUsernameForEmail`,
//! `getUidForEmail`, `resolveTargetEmail`, `processMemberFields`, and the
//! profile-image URL sanitizers.

use crate::auth;
use crate::data::DataStore;
use serde_json::{json, Value};
use std::path::Path;

/// `normalizeUsername` (server.js:2326-2328).
pub fn normalize_username(username: &str) -> String {
    username.trim().to_lowercase()
}

/// `defaultUsernameForEmail` (server.js:2334-2345) without the optional
/// `used` dedup set (the member-list call sites never pass one).
pub fn default_username_for_email(email: &str) -> String {
    let local: String = email
        .split('@')
        .next()
        .unwrap_or("")
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    // Collapse runs of '-', trim leading/trailing [._-].
    let mut collapsed = String::new();
    let mut last_dash = false;
    for c in local.chars() {
        if c == '-' {
            if !last_dash {
                collapsed.push(c);
            }
            last_dash = true;
        } else {
            collapsed.push(c);
            last_dash = false;
        }
    }
    let trimmed = collapsed
        .trim_start_matches(['.', '_', '-'])
        .trim_end_matches(['.', '_', '-']);
    if trimmed.is_empty() {
        "user".to_string()
    } else {
        trimmed.to_string()
    }
}

/// `processMemberFields` (server.js:6147-6174). When `profile` is `None` the
/// profiles document is loaded here, exactly like the JS lazy load.
pub fn process_member_fields(
    store: &DataStore,
    data_dir: &Path,
    member_email: &str,
    profile: Option<&Value>,
    viewer_email: Option<&str>,
) -> Value {
    if member_email.is_empty() {
        return json!({ "displayName": Value::Null, "email": "" });
    }
    let norm_target = auth::normalize_email(member_email);
    let norm_viewer = viewer_email.map(auth::normalize_email).unwrap_or_default();
    let viewer_can_see = norm_viewer == norm_target
        || auth::is_admin_email(store, viewer_email.unwrap_or(""))
        || auth::is_moderator_email(store, viewer_email.unwrap_or(""));
    let profiles;
    let p = match profile {
        Some(p) => p.clone(),
        None => {
            profiles = store.read_document(&data_dir.join("profiles.json"), json!({}));
            profiles
                .get(norm_target.as_str())
                .cloned()
                .unwrap_or(json!({}))
        }
    };
    let username = p
        .get("username")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| default_username_for_email(&norm_target));
    // A saved display name must be visible to other members (JS comment at
    // server.js:6156) — nickname/displayName win over the generated username.
    let public_name = p
        .get("displayName")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            p.get("nickname")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or(username.as_str())
        .to_string();
    if !viewer_can_see {
        return json!({ "displayName": public_name, "email": username });
    }
    json!({
        "displayName": public_name,
        "email": crate::admin::mask_email(member_email),
    })
}

/// `getUidForEmail` (server.js:6043-6059): names.json reverse lookup, then
/// the current-generation email id.
pub fn get_uid_for_email(store: &DataStore, id_secret: &[u8], email: &str) -> Option<String> {
    if email.is_empty() {
        return None;
    }
    let norm = auth::normalize_email(email);
    if let Some(sid) = auth::names_find_sid_by_norm_email(store, &norm) {
        return Some(sid);
    }
    let gen = auth::current_session_generation(store, &norm);
    Some(auth::make_email_id(&norm, gen.max(0) as u64, id_secret))
}

/// `emailFromHash(hash)` (server.js:6186-6220).
pub fn email_from_hash(
    store: &DataStore,
    data_dir: &Path,
    id_secret: &[u8],
    hash: &str,
) -> Option<String> {
    if hash.is_empty() {
        return None;
    }
    if hash.contains('@') {
        return Some(hash.to_string());
    }
    if let Some(email) = crate::auth::names_get_email(store, hash) {
        if !email.is_empty() {
            return Some(email);
        }
    }
    let username = normalize_username(hash);
    let profiles = store.read_document(&data_dir.join("profiles.json"), json!({}));
    if let Some(map) = profiles.as_object() {
        for (email, p) in map {
            if !username.is_empty() {
                if let Some(u) = p.get("username").and_then(|v| v.as_str()) {
                    if normalize_username(u) == username {
                        return Some(email.clone());
                    }
                }
            }
            if get_uid_for_email(store, id_secret, email).as_deref() == Some(hash) {
                return Some(email.clone());
            }
        }
    }
    let stats = store.read_document(&data_dir.join("user_stats.json"), json!({}));
    if let Some(map) = stats.as_object() {
        for email in map.keys() {
            if get_uid_for_email(store, id_secret, email).as_deref() == Some(hash) {
                return Some(email.clone());
            }
        }
    }
    let tokens = store.read_document(&data_dir.join("tokens.json"), json!({}));
    if let Some(map) = tokens.as_object() {
        for t in map.values() {
            if let Some(email) = t.get("email").and_then(|v| v.as_str()) {
                if get_uid_for_email(store, id_secret, email).as_deref() == Some(hash) {
                    return Some(email.to_string());
                }
            }
        }
    }
    None
}

/// `resolveTargetEmail` (server.js:6060-6124): direct/normalized match, then
/// mask/uid match, then username/displayName exact, then local part, then
/// substring. All against passwords.json + profiles.json.
pub fn resolve_target_email(
    store: &DataStore,
    data_dir: &Path,
    id_secret: &[u8],
    input: &str,
) -> Option<String> {
    if input.is_empty() {
        return None;
    }
    let target = input.trim().to_lowercase();
    if target.is_empty() {
        return None;
    }
    let passwords = store.read_document(&data_dir.join("passwords.json"), json!({}));
    let password_map = passwords.as_object()?;
    let emails: Vec<String> = password_map
        .keys()
        .map(|e| e.trim().to_lowercase())
        .collect();
    let has_pw = |email: &str| password_map.contains_key(email);

    // 1. Direct or normalized match.
    if has_pw(&target) {
        return Some(target);
    }
    let norm_target = auth::normalize_email(&target);
    if has_pw(&norm_target) {
        return Some(norm_target.clone());
    }

    // 2. Match by maskEmail or getUidForEmail.
    let trimmed_input = input.trim().to_string();
    for email in &emails {
        if auth::normalize_email(email) == norm_target {
            return Some(email.clone());
        }
        if crate::admin::mask_email(email).to_lowercase() == target {
            return Some(email.clone());
        }
        if get_uid_for_email(store, id_secret, email).as_deref() == Some(trimmed_input.as_str()) {
            return Some(email.clone());
        }
    }

    // 3. Match by username / displayName (exact, case-insensitive).
    let profiles = store.read_document(&data_dir.join("profiles.json"), json!({}));
    if let Some(map) = profiles.as_object() {
        for (norm_email, profile) in map {
            if let Some(username) = profile.get("username").and_then(|v| v.as_str()) {
                if normalize_username(username) == target && has_pw(norm_email) {
                    return Some(norm_email.clone());
                }
            }
            if let Some(display) = profile.get("displayName").and_then(|v| v.as_str()) {
                if display.to_lowercase().trim() == target && has_pw(norm_email) {
                    return Some(norm_email.clone());
                }
            }
        }
    }

    // 4. Match by local part.
    for email in &emails {
        if email.split('@').next() == Some(target.as_str()) {
            return Some(email.clone());
        }
    }

    // 5. Match by display-name substring.
    if let Some(map) = profiles.as_object() {
        for (norm_email, profile) in map {
            if let Some(username) = profile.get("username").and_then(|v| v.as_str()) {
                if normalize_username(username).contains(&target) && has_pw(norm_email) {
                    return Some(norm_email.clone());
                }
            }
            if let Some(display) = profile.get("displayName").and_then(|v| v.as_str()) {
                if display.to_lowercase().contains(&target) && has_pw(norm_email) {
                    return Some(norm_email.clone());
                }
            }
        }
    }

    None
}

/// `PROFILE_IMAGE_MIME_RE` (server.js:2058).
fn profile_image_mime_ok(mime: &str) -> bool {
    let lower = mime.to_lowercase();
    matches!(
        lower.as_str(),
        "image/png" | "image/jpeg" | "image/jpg" | "image/webp" | "image/gif"
    )
}

/// `sanitizeProfileImageUrl` (server.js:2060-2097). `allow_data` maps to
/// `opts.allowData !== false`; `max_url_length`/`max_data_bytes` to the opts.
pub fn sanitize_profile_image_url(
    value: &str,
    allow_data: bool,
    max_url_length: usize,
    max_data_bytes: usize,
) -> String {
    let raw = value.trim();
    if raw.is_empty() {
        return String::new();
    }
    if raw
        .chars()
        .any(|c| c < '\u{20}' || c == '\u{7f}' || matches!(c, '<' | '>' | '"' | '`'))
    {
        return String::new();
    }

    if raw.to_lowercase().starts_with("data:") {
        if !allow_data {
            return String::new();
        }
        // ^data:([^;,]+);base64,([a-z0-9+/=\s]+)$
        let rest = &raw[5..];
        let Some((mime_part, payload)) = rest.split_once(";base64,") else {
            return String::new();
        };
        if mime_part.contains(';') || mime_part.contains(',') {
            return String::new();
        }
        if !profile_image_mime_ok(mime_part) {
            return String::new();
        }
        // JS validates the base64 charset with ([a-z0-9+/=\s]+) anchored at
        // both ends, then strips whitespace.
        let charset_ok = payload.chars().all(|c| {
            c.is_ascii_lowercase()
                || c.is_ascii_uppercase()
                || c.is_ascii_digit()
                || matches!(c, '+' | '/' | '=' | ' ' | '\t' | '\n' | '\r')
        });
        if !charset_ok {
            return String::new();
        }
        let stripped: String = payload.chars().filter(|c| !c.is_whitespace()).collect();
        if stripped.is_empty() || !stripped.len().is_multiple_of(4) {
            return String::new();
        }
        let bytes = crate::crypto::base64_decode(&stripped);
        if bytes.is_empty() || bytes.len() > max_data_bytes {
            return String::new();
        }
        format!("data:{};base64,{}", mime_part, stripped)
    } else {
        if raw.len() > max_url_length {
            return String::new();
        }
        match url::Url::parse(raw) {
            Ok(u) => {
                if u.scheme() != "https" && u.scheme() != "http" {
                    return String::new();
                }
                if u.username() != "" || u.password().is_some() {
                    return String::new();
                }
                // JS new URL lowercases the host; u.to_string() differs from
                // u.href only in trailing-slash normalization both share.
                u.to_string()
            }
            Err(_) => String::new(),
        }
    }
}

/// `sanitizeProfileWebsiteUrl` (server.js:2092-2104).
pub fn sanitize_profile_website_url(value: &str) -> String {
    let raw = value.trim();
    if raw.is_empty() || raw.len() > 300 {
        return String::new();
    }
    if raw
        .chars()
        .any(|c| c.is_control() || matches!(c, '<' | '>' | '"' | '`'))
    {
        return String::new();
    }
    match url::Url::parse(raw) {
        Ok(u) => {
            if u.scheme() != "https" && u.scheme() != "http" {
                return String::new();
            }
            u.to_string()
        }
        Err(_) => String::new(),
    }
}

/// `isValidUsername` (server.js:2330-2332).
pub fn is_valid_username(username: &str) -> bool {
    let norm = normalize_username(username);
    let ok_len = (2..=40).contains(&norm.chars().count());
    let ok_chars = !norm.is_empty()
        && norm
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'));
    ok_len && ok_chars && !username.contains('@')
}

/// `defaultUsernameForEmail(email, used)` with the dedup set — used by
/// `ensureProfileDefaults` (server.js:2334-2345).
pub fn default_username_for_email_used(
    email: &str,
    used: &mut std::collections::HashSet<String>,
) -> String {
    let local: String = email
        .split('@')
        .next()
        .unwrap_or("")
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let mut collapsed = String::new();
    let mut last_dash = false;
    for c in local.chars() {
        if c == '-' {
            if !last_dash {
                collapsed.push(c);
            }
            last_dash = true;
        } else {
            collapsed.push(c);
            last_dash = false;
        }
    }
    let local = collapsed
        .trim_start_matches(['.', '_', '-'])
        .trim_end_matches(['.', '_', '-'])
        .to_string();
    let local = if local.is_empty() {
        "user".to_string()
    } else {
        local
    };
    let mut candidate = local.clone();
    let mut i = 2;
    while used.contains(&candidate) {
        candidate = format!("{}-{}", local, i);
        i += 1;
    }
    used.insert(candidate.clone());
    candidate
}

/// `canonicalDeliveryEmail` (server.js:1383-1458): map a normalized storage
/// key back to the real dotted address via names.json → profiles.json →
/// tokens.json, with sid resolution for non-email inputs.
pub fn canonical_delivery_email(
    store: &DataStore,
    data_dir: &Path,
    id_secret: &[u8],
    raw: &str,
) -> String {
    let mut raw = raw.trim().to_string();
    if raw.is_empty() {
        return String::new();
    }
    if !raw.contains('@') {
        if let Some(from_sid) = crate::auth::email_from_sid(store, id_secret, &raw) {
            raw = from_sid;
        } else if let Some(hit) = crate::auth::names_get_email(store, &raw) {
            if hit.contains('@') {
                raw = hit;
            }
        }
        if !raw.contains('@') {
            return raw;
        }
    }
    let norm = crate::auth::normalize_email(&raw);
    let local_dotted = |e: &str| {
        e.split('@')
            .next()
            .map(|l| l.contains('.'))
            .unwrap_or(false)
    };

    // 1. names table — dotted local or any-case difference wins.
    let name_candidates = crate::auth::names_candidates_for_norm_email(store, &norm);
    for e in &name_candidates {
        if e.contains('@') && (local_dotted(e) || e != &norm) {
            return e.to_lowercase().trim().to_string();
        }
    }

    // 2. profiles.json — user-customized profile email.
    let profiles = store.read_document(&data_dir.join("profiles.json"), json!({}));
    if let Some(p) = profiles.get(norm.as_str()) {
        if let Some(e) = p.get("email").and_then(|v| v.as_str()) {
            if e.contains('@')
                && crate::auth::normalize_email(e) == norm
                && (local_dotted(e) || e != norm)
            {
                return e.to_lowercase().trim().to_string();
            }
        }
    }

    // 3. tokens.json — enrollment tokens.
    let tokens = store.read_document(&data_dir.join("tokens.json"), json!({}));
    if let Some(map) = tokens.as_object() {
        for data in map.values() {
            if let Some(e) = data.get("email").and_then(|v| v.as_str()) {
                if e.contains('@')
                    && crate::auth::normalize_email(e) == norm
                    && (local_dotted(e) || e != norm)
                {
                    return e.to_lowercase().trim().to_string();
                }
            }
        }
    }

    // 4. Any match in the names table.
    if let Some(e) = name_candidates.first() {
        return e.to_lowercase().trim().to_string();
    }

    raw
}

/// `displayEmail` (server.js:1460-148成分5): profile email when dotted, else
/// the canonical address, swapping the @student.rjuhsd.us storage domain for
/// the @student.mitch.pro display domain when no better candidate exists.
pub fn display_email(
    store: &DataStore,
    data_dir: &Path,
    id_secret: &[u8],
    norm_or_email: &str,
) -> String {
    let raw = norm_or_email.to_string();
    if raw.is_empty() {
        return String::new();
    }
    let norm = crate::auth::normalize_email(&raw);
    let profiles = store.read_document(&data_dir.join("profiles.json"), json!({}));
    if let Some(p) = profiles.get(norm.as_str()) {
        if let Some(e) = p.get("email").and_then(|v| v.as_str()) {
            if e.split('@')
                .next()
                .map(|l| l.contains('.'))
                .unwrap_or(false)
            {
                return e.to_string();
            }
        }
    }
    let canonical = canonical_delivery_email(store, data_dir, id_secret, &raw);
    if canonical
        .split('@')
        .next()
        .map(|l| l.contains('.'))
        .unwrap_or(false)
    {
        return canonical.replace("@student.rjuhsd.us", "@student.mitch.pro");
    }
    raw.replace("@student.rjuhsd.us", "@student.mitch.pro")
}

/// `ensureProfileDefaults` (server.js:2411-2446): normalize + persist a
/// profile record with the full default set. Returns the stored record.
pub fn ensure_profile_defaults(
    store: &DataStore,
    data_dir: &Path,
    id_secret: &[u8],
    norm_email: &str,
    original_email: &str,
    patch: &Value,
) -> Value {
    let profiles_file = data_dir.join("profiles.json");
    let profiles = store.read_document(&profiles_file, json!({}));
    let mut used = std::collections::HashSet::new();
    if let Some(map) = profiles.as_object() {
        for (email, p) in map {
            if email != norm_email {
                let u =
                    normalize_username(p.get("username").and_then(|v| v.as_str()).unwrap_or(""));
                if !u.is_empty() {
                    used.insert(u);
                }
            }
        }
    }
    let existing = profiles.get(norm_email).cloned().unwrap_or(json!({}));
    // JS: patch.username || existing.username || '' (truthiness).
    let truthy_str = |v: Option<&Value>| -> Option<String> {
        match v {
            Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
            _ => None,
        }
    };
    let username_raw = truthy_str(patch.get("username"))
        .or_else(|| truthy_str(existing.get("username")))
        .unwrap_or_default();
    let mut username = normalize_username(&username_raw);
    if !is_valid_username(&username) || used.contains(&username) {
        username = default_username_for_email_used(norm_email, &mut used);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    // JS `a ?? b ?? ''` — only null/undefined fall through, '' wins when set.
    let nullish = |keys: &[Option<&Value>]| -> Option<String> {
        for v in keys.iter().flatten() {
            if !v.is_null() {
                if let Some(s) = v.as_str() {
                    return Some(s.to_string());
                }
            }
        }
        Some(String::new())
    };
    let existing_clone = existing.clone();
    let nickname = nullish(&[
        patch.get("nickname"),
        existing.get("nickname"),
        existing.get("displayName"),
    ])
    .unwrap_or_default()
    .trim()
    .chars()
    .take(40)
    .collect::<String>();
    let display_name = nullish(&[
        patch.get("displayName"),
        existing.get("displayName"),
        patch.get("nickname"),
        existing.get("nickname"),
    ])
    .unwrap_or_default()
    .trim()
    .chars()
    .take(40)
    .collect::<String>();
    let email_src = truthy_str(existing.get("email"))
        .or_else(|| Some(original_email.to_string()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| norm_email.to_string());
    let completed = existing
        .get("hasCompletedTutorial")
        .and_then(|v| v.as_bool())
        .or_else(|| {
            existing
                .get("has_completed_tutorial")
                .and_then(|v| v.as_bool())
        })
        .unwrap_or(false)
        || patch
            .get("hasCompletedTutorial")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
    let js_or_empty = |key: &str| -> Value {
        match existing.get(key) {
            Some(Value::String(s)) if s.is_empty() => json!(""),
            Some(v) if !v.is_null() => v.clone(),
            _ => json!(""),
        }
    };
    // Start from `...existing` so unknown keys (2fa fields etc.) survive,
    // then override the normalized fields exactly like the JS spread.
    let mut record = existing_clone;
    // existing is always an object or {} — reset to a fresh object if the
    // store ever hands back something else.
    if !record.is_object() {
        record = serde_json::json!({});
    }
    let obj = record
        .as_object_mut()
        .unwrap_or_else(|| unreachable!("just checked record is an object"));
    obj.insert(
        "email".into(),
        json!(canonical_delivery_email(
            store, data_dir, id_secret, &email_src
        )),
    );
    obj.insert("username".into(), json!(username));
    obj.insert("nickname".into(), json!(nickname));
    obj.insert("displayName".into(), json!(display_name));
    obj.insert("bio".into(), js_or_empty("bio"));
    obj.insert("website".into(), js_or_empty("website"));
    obj.insert("pfp".into(), js_or_empty("pfp"));
    obj.insert("background".into(), js_or_empty("background"));
    let trim_take = |keys: &[Option<&Value>], n: usize| -> String {
        nullish(keys)
            .unwrap_or_default()
            .trim()
            .chars()
            .take(n)
            .collect::<String>()
    };
    obj.insert(
        "gradYear".into(),
        json!(trim_take(
            &[patch.get("gradYear"), existing.get("gradYear")],
            20
        )),
    );
    obj.insert(
        "gender".into(),
        json!(trim_take(
            &[patch.get("gender"), existing.get("gender")],
            40
        )),
    );
    obj.insert(
        "referralSource".into(),
        json!(trim_take(
            &[patch.get("referralSource"), existing.get("referralSource")],
            80
        )),
    );
    obj.insert("hasCompletedTutorial".into(), json!(completed));
    obj.insert(
        "createdAt".into(),
        existing
            .get("createdAt")
            .cloned()
            .filter(|v| !v.is_null() && *v != json!(0))
            .unwrap_or(json!(now)),
    );
    obj.insert("updatedAt".into(), json!(now));
    obj.insert(
        "profileBonusClaimed".into(),
        json!(existing
            .get("profileBonusClaimed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)),
    );

    let mut profiles_out = profiles;
    if let Some(map) = profiles_out.as_object_mut() {
        map.insert(norm_email.to_string(), record.clone());
    }
    let _ = store.write_document(&profiles_file, &profiles_out);
    record
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_usernames_match_js() {
        assert_eq!(
            default_username_for_email("Mitch Fogler@mitch.pro"),
            "mitch-fogler"
        );
        assert_eq!(default_username_for_email("a@b.c"), "a");
        assert_eq!(default_username_for_email("...@@@"), "user");
        assert_eq!(default_username_for_email("--x--@y.z"), "x");
    }

    #[test]
    fn image_url_rejects_control_chars_and_bad_mime() {
        assert_eq!(
            sanitize_profile_image_url("https://x.test/a.png", true, 1000, 100),
            "https://x.test/a.png"
        );
        assert_eq!(
            sanitize_profile_image_url("javascript:alert(1)", true, 1000, 100),
            ""
        );
        assert_eq!(
            sanitize_profile_image_url("https://u:p@x.test/a.png", true, 1000, 100),
            ""
        );
        assert_eq!(
            sanitize_profile_image_url("data:text/html;base64,AAAA", true, 1000, 100),
            ""
        );
        assert_eq!(
            sanitize_profile_image_url("data:image/png;base64,aGk=", true, 1000, 100),
            "data:image/png;base64,aGk="
        );
        assert_eq!(
            sanitize_profile_image_url("data:image/png;base64,!!!", true, 1000, 100),
            ""
        );
    }
}
