//! DM shared layer (plan Step 11) — `dmAddressIndex`/`resolveMemberRef`
//! (server.js:2394-2417), the conversation auto-delete helpers
//! (server.js:6177-6275), and `validateE2eEnvelope` (server.js:2282-2332).
//!
//! All documents (profiles.json, names.json, tokens.json, dms.json,
//! groups.json, chat_expiry.json) are `loadJson`/`saveJson` files → the
//! SQLite-backed document layer, matching every other consumer.

use crate::auth::normalize_email;
use crate::data::DataStore;
use crate::profile::{default_username_for_email, normalize_username};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

/// `dmStoreFiles(req)` (server.js:6201-6207) — sexypickleclub.com reads and
/// writes the pickle_* stores so Cellar conversations stay separate.
pub struct DmStore {
    pub dms: &'static str,
    pub groups: &'static str,
    pub pickle: bool,
}

pub const DMS_MAIN: DmStore = DmStore {
    dms: "dms.json",
    groups: "groups.json",
    pickle: false,
};
pub const DMS_PICKLE: DmStore = DmStore {
    dms: "pickle_dms.json",
    groups: "pickle_groups.json",
    pickle: true,
};

fn dm_cleared_store_key(store: &DmStore) -> &'static str {
    if store.pickle {
        "pickle"
    } else {
        "main"
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// One user's per-conversation cleared-at map (`{convoKey: timestamp}`),
/// or an empty object if they've never cleared anything — real
/// `dm_cleared` table (one row per email+store) instead of the old
/// dm_cleared.json/pickle_dm_cleared.json blobs.
pub fn dm_cleared_get(store: &DataStore, dm_store: &DmStore, norm_email: &str) -> Value {
    let conn = store.conn();
    let raw: Option<String> = conn
        .query_row(
            "SELECT data FROM dm_cleared WHERE email = ?1 AND store = ?2",
            rusqlite::params![norm_email, dm_cleared_store_key(dm_store)],
            |r| r.get(0),
        )
        .ok();
    raw.and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}))
}

pub fn dm_cleared_set(store: &DataStore, dm_store: &DmStore, norm_email: &str, data: &Value) {
    let data_str = serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string());
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO dm_cleared (email, store, data, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(email, store) DO UPDATE SET data = ?3, updated_at = ?4",
        rusqlite::params![norm_email, dm_cleared_store_key(dm_store), data_str, now_millis()],
    );
}

/// Moves both the main and pickle cleared-state rows (if any) onto the
/// user's new normalized email.
pub fn rename_dm_cleared_email(store: &DataStore, old_norm: &str, new_norm: &str) {
    let conn = store.conn();
    let _ = conn.execute(
        "UPDATE dm_cleared SET email = ?2 WHERE email = ?1",
        rusqlite::params![old_norm, new_norm],
    );
}

/// `Number(process.env.NAME || default)` — JS falsy (missing, 0, NaN, "")
/// falls to the default.
fn env_or_f64(name: &str, default: f64) -> f64 {
    let v = std::env::var(name)
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|n| *n != 0.0 && n.is_finite());
    v.unwrap_or(default)
}

/// `MAX_JSON_BODY_BYTES` (server.js:414) and `MAX_CHAT_JSON_BODY_BYTES`
/// (server.js:419-421) — `min(4 MiB, max(MAX_JSON_BODY_BYTES, env || 2300KiB))`.
pub fn max_json_body_bytes() -> usize {
    env_or_f64("MAX_JSON_BODY_BYTES", 256.0 * 1024.0).max(0.0) as usize
}

pub fn max_chat_json_body_bytes() -> usize {
    let chat = env_or_f64("MAX_CHAT_JSON_BODY_BYTES", 2300.0 * 1024.0);
    (4.0f64 * 1024.0 * 1024.0)
        .min(chat.max(max_json_body_bytes() as f64))
        .max(0.0) as usize
}

/// `MAX_E2E_ENVELOPE_BYTES` (server.js:422).
pub fn max_e2e_envelope_bytes() -> usize {
    (max_chat_json_body_bytes().saturating_sub(4096)).min(2200 * 1024)
}

/// `CHAT_EXPIRY_OPTIONS` (server.js:6177).
pub const CHAT_EXPIRY_OPTIONS: [f64; 4] = [30_000.0, 60_000.0, 300_000.0, 3_600_000.0];

/// `dmExpiryKey(a, b)` — `'dm:' + sorted norms joined by '|'`.
pub fn dm_expiry_key(a: &str, b: &str) -> String {
    let mut pair = [normalize_email(a), normalize_email(b)];
    pair.sort();
    format!("dm:{}|{}", pair[0], pair[1])
}

/// `groupExpiryKey(groupId)`.
pub fn group_expiry_key(group_id: &str) -> String {
    format!("group:{}", group_id)
}

/// `getChatExpiry(key)` — a Number of the stored value, validated against the
/// four option windows (`Number(undefined) || 0` → 0 for missing keys).
pub fn get_chat_expiry(store: &DataStore, data_dir: &Path, key: &str) -> f64 {
    if key.is_empty() {
        return 0.0;
    }
    let all = store.read_document(&data_dir.join("chat_expiry.json"), json!({}));
    let v = all
        .get(key)
        .and_then(crate::jsval::number)
        .unwrap_or(f64::NAN);
    let v = if v.is_nan() { 0.0 } else { v };
    if CHAT_EXPIRY_OPTIONS.contains(&v) {
        v
    } else {
        0.0
    }
}

/// `setChatExpiry(key, ms)` (server.js:6191-6196) — a valid window stores,
/// everything else (including 0) DELETES the key.
pub fn set_chat_expiry(store: &DataStore, data_dir: &Path, key: &str, ms: f64) {
    let mut all = store.read_document(&data_dir.join("chat_expiry.json"), json!({}));
    let Some(obj) = all.as_object_mut() else {
        return;
    };
    if ms != 0.0 && CHAT_EXPIRY_OPTIONS.contains(&ms) {
        obj.insert(key.to_string(), json!(ms));
    } else {
        obj.remove(key);
    }
    store
        .write_document(&data_dir.join("chat_expiry.json"), &all)
        .ok();
}

/// `isDmMessageRead(m)` (server.js:6213-6221).
pub fn is_dm_message_read(m: &Value) -> bool {
    if m.is_null() {
        return false;
    }
    if m.get("kind").and_then(|v| v.as_str()) == Some("group") {
        let sender = normalize_email(&crate::jsval::string(m.get("from").unwrap_or(&Value::Null)));
        return m
            .get("readBy")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .any(|r| normalize_email(&crate::jsval::string(r)) != sender)
            })
            .unwrap_or(false);
    }
    m.get("read").and_then(|v| v.as_bool()).unwrap_or(false)
        || crate::jsval::truthy(m.get("readAt").unwrap_or(&Value::Null))
}

/// `getExpiryForMsg(m, isPickle)` (server.js:6223-6231) — reads
/// chat_expiry.json per call like the JS `getChatExpiry`.
pub fn get_expiry_for_msg(store: &DataStore, data_dir: &Path, m: &Value, is_pickle: bool) -> f64 {
    if m.is_null() {
        return 0.0;
    }
    let pfx = if is_pickle { "spc:" } else { "" };
    if m.get("kind").and_then(|v| v.as_str()) == Some("group") {
        let gid = crate::jsval::string(m.get("groupId").unwrap_or(&Value::Null));
        return get_chat_expiry(store, data_dir, &format!("{pfx}{}", group_expiry_key(&gid)));
    }
    let from = crate::jsval::string(m.get("from").unwrap_or(&Value::Null));
    let to = crate::jsval::string(m.get("to").unwrap_or(&Value::Null));
    get_chat_expiry(
        store,
        data_dir,
        &format!("{pfx}{}", dm_expiry_key(&from, &to)),
    )
}

/// `isMessageExpired(m, isPickle, now)` (server.js:6233-6251) — auto-delete
/// only deletes messages AFTER they have been read.
pub fn is_message_expired(
    store: &DataStore,
    data_dir: &Path,
    m: &Value,
    is_pickle: bool,
    now: f64,
) -> bool {
    if m.is_null() {
        return false;
    }
    if !is_dm_message_read(m) {
        return false;
    }
    let expiry = get_expiry_for_msg(store, data_dir, m, is_pickle);
    let ts = m.get("ts").and_then(crate::jsval::number).unwrap_or(0.0);
    if expiry > 0.0 {
        // Delete if an hour old, older than expiry, past expiresAt, or read
        // for >= expiry.
        let expires_at = m.get("expiresAt").and_then(crate::jsval::number);
        let read_at = m.get("readAt").and_then(crate::jsval::number);
        (now - ts >= 3_600_000.0)
            || (now - ts >= expiry)
            || expires_at.map(|e| now > e).unwrap_or(false)
            || read_at.map(|r| now - r >= expiry).unwrap_or(false)
    } else {
        m.get("expiresAt")
            .and_then(crate::jsval::number)
            .map(|e| now > e)
            .unwrap_or(false)
    }
}

/// `pruneDms(dms, isPickle)` (server.js:6253-6276) — walks newest-first,
/// drops expired (read-gated) messages and keeps the newest 100 per
/// conversation, then unshift-reverses back to chronological order.
pub fn prune_dms(
    store: &DataStore,
    data_dir: &Path,
    dms: &[Value],
    is_pickle: bool,
    now: f64,
) -> Vec<Value> {
    let mut counts: HashMap<String, u32> = HashMap::new();
    let mut keep_rev: Vec<Value> = Vec::new();
    for msg in dms.iter().rev() {
        if msg.is_null() {
            continue;
        }
        if is_message_expired(store, data_dir, msg, is_pickle, now) {
            continue;
        }
        let convo_id = if msg.get("kind").and_then(|v| v.as_str()) == Some("group") {
            format!(
                "g::{}",
                crate::jsval::string(msg.get("groupId").unwrap_or(&Value::Null))
            )
        } else {
            let u1 = normalize_email(&crate::jsval::string(
                msg.get("from").unwrap_or(&Value::Null),
            ));
            let u2 = normalize_email(&crate::jsval::string(msg.get("to").unwrap_or(&Value::Null)));
            let mut pair = [u1, u2];
            pair.sort();
            format!("d::{}::{}", pair[0], pair[1])
        };
        let count = counts.entry(convo_id).or_insert(0);
        if *count < 100 {
            keep_rev.push(msg.clone());
            *count += 1;
        }
    }
    keep_rev.reverse();
    keep_rev
}

/// The address index — JS Maps with get-only access, so HashMaps.
#[derive(Clone)]
pub struct DmAddrIndex {
    emails: std::collections::HashSet<String>,
    by_username: HashMap<String, String>,
    by_mask: HashMap<String, String>,
}

fn add_email(idx: &mut DmAddrIndex, raw: &str) {
    if raw.is_empty() {
        return;
    }
    let raw_lower = raw.to_lowercase().trim().to_string();
    let norm = normalize_email(&raw_lower);
    if !norm.contains('@') || idx.emails.contains(&norm) {
        return;
    }
    idx.emails.insert(norm.clone());
    add_username(idx, &default_username_for_email(&norm), &norm);
    // maskEmail (server.js:1380) is effectively the identity for truthy
    // emails — both putMask keys normalize to the same value.
    let put_mask = |key: String, val: &str, by_mask: &mut HashMap<String, String>| {
        let key = normalize_email(&key.to_lowercase());
        if !key.is_empty() && !by_mask.contains_key(&key) {
            by_mask.insert(key, val.to_string());
        }
    };
    put_mask(
        crate::admin::mask_email(&norm).to_string(),
        &norm,
        &mut idx.by_mask,
    );
    put_mask(
        crate::admin::mask_email(&raw_lower).to_string(),
        &norm,
        &mut idx.by_mask,
    );
}

fn add_username(idx: &mut DmAddrIndex, uname: &str, norm: &str) {
    let uname = normalize_username(uname);
    if !uname.is_empty() && !idx.by_username.contains_key(&uname) {
        idx.by_username.insert(uname, norm.to_string());
    }
}

/// `dmAddressIndex()` WITHOUT the 15s cache — callers should use
/// [`address_index_cached`] so the JS staleness behavior is preserved.
pub fn build_address_index(store: &DataStore, data_dir: &Path) -> DmAddrIndex {
    let mut idx = DmAddrIndex {
        emails: std::collections::HashSet::new(),
        by_username: HashMap::new(),
        by_mask: HashMap::new(),
    };
    let profiles = store.read_document(&data_dir.join("profiles.json"), json!({}));
    for (email, p) in profiles.as_object().unwrap_or(&Map::new()) {
        add_email(&mut idx, email);
        let norm = normalize_email(email);
        let uname = p.get("username").and_then(|v| v.as_str()).unwrap_or("");
        add_username(&mut idx, uname, &norm);
        add_username(&mut idx, &default_username_for_email(&norm), &norm);
    }
    for email in crate::auth::names_all_emails(store) {
        add_email(&mut idx, &email);
    }
    let tokens = store.read_document(&store.base_dir.join("data/tokens.json"), json!({}));
    for data in tokens.as_object().unwrap_or(&Map::new()).values() {
        if let Some(email) = data.get("email").and_then(|v| v.as_str()) {
            add_email(&mut idx, email);
        }
    }
    idx
}

static ADDR_IDX: Mutex<Option<(DmAddrIndex, i64)>> = Mutex::new(None);

/// `dmAddressIndex()` with the JS 15s staleness cache.
pub fn address_index_cached(store: &DataStore, data_dir: &Path) -> DmAddrIndex {
    let now = crate::school::now_millis();
    {
        let guard = ADDR_IDX.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((idx, at)) = guard.as_ref() {
            if now - at < 15_000 {
                return idx.clone();
            }
        }
    }
    let idx = build_address_index(store, data_dir);
    let mut guard = ADDR_IDX.lock().unwrap_or_else(|e| e.into_inner());
    *guard = Some((idx.clone(), now));
    idx
}

/// `resolveMemberRef(raw)` (server.js:2394-2417).
pub fn resolve_member_ref(idx: &DmAddrIndex, raw: &str) -> String {
    let q = raw.to_lowercase().trim().to_string();
    if q.is_empty() {
        return String::new();
    }
    if q.contains('@') {
        let norm = normalize_email(&q);
        if idx.emails.contains(&norm) {
            return norm;
        }
        if let Some(by_m) = idx.by_mask.get(&norm) {
            return by_m.clone();
        }
        let local = norm
            .split('@')
            .next()
            .unwrap_or("")
            .split('+')
            .next()
            .unwrap_or("")
            .replace('.', "");
        if let Some(by_u) = idx.by_username.get(&normalize_username(&local)) {
            return by_u.clone();
        }
        return norm;
    }
    idx.by_username
        .get(&normalize_username(&q))
        .cloned()
        .unwrap_or_default()
}

/// `isHex(value, exactLength)` (server.js:2277) — `!exactLength` (0) skips
/// the length equality check.
pub fn is_hex(value: &Value, exact_len: usize) -> bool {
    let text = crate::jsval::string(value);
    (exact_len == 0 || text.len() == exact_len)
        && !text.is_empty()
        && text.len().is_multiple_of(2)
        && text.bytes().all(|b| b.is_ascii_hexdigit())
}

fn valid_cipher_payload(payload: Option<&Map<String, Value>>) -> bool {
    let Some(p) = payload else {
        return false;
    };
    if !is_hex(p.get("iv").unwrap_or(&Value::Null), 24)
        || !is_hex(p.get("ciphertext").unwrap_or(&Value::Null), 0)
    {
        return false;
    }
    match p.get("recipientPubKey") {
        None => true,
        Some(r) => is_hex(r, 130) && crate::jsval::string(r).starts_with("04"),
    }
}

/// `validateE2eEnvelope(rawText, groupExpected)` (server.js:2282-2332) —
/// `Err((status, error))` on failure.
pub fn validate_e2e_envelope(
    raw_text: &str,
    group_expected: bool,
) -> Result<(), (u16, &'static str)> {
    if raw_text.len() > max_e2e_envelope_bytes() {
        return Err((413, "encrypted message too large"));
    }
    let envelope: Value = match serde_json::from_str(raw_text) {
        Ok(v) => v,
        Err(_) => return Err((400, "invalid encrypted message")),
    };
    if !envelope.is_object() || envelope.get("e2e").and_then(|v| v.as_bool()) != Some(true) {
        return Err((400, "invalid encrypted message"));
    }
    // `Number(envelope.version || 1)` — a falsy version (0, "", null) falls
    // to 1 BEFORE the range check, so version 0 is valid in JS.
    let version_v = envelope.get("version");
    let version = if version_v.map(crate::jsval::truthy).unwrap_or(false) {
        version_v.and_then(crate::jsval::number).unwrap_or(f64::NAN)
    } else {
        1.0
    };
    if !(version.is_finite() && version.fract() == 0.0 && (1.0..=3.0).contains(&version)) {
        return Err((400, "unsupported encrypted message version"));
    }
    let sender = envelope.get("senderPubKey").unwrap_or(&Value::Null);
    if !is_hex(sender, 130) || !crate::jsval::string(sender).starts_with("04") {
        return Err((400, "invalid encrypted sender key"));
    }
    let is_group = envelope
        .get("group")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if is_group != group_expected {
        return Err((400, "encrypted message target mismatch"));
    }
    let Some(obj) = envelope.as_object() else {
        return Err((400, "invalid encrypted message"));
    };
    if is_group {
        let Some(payloads) = obj.get("payloads").and_then(|v| v.as_object()) else {
            return Err((400, "invalid encrypted group payload"));
        };
        let n = payloads.len();
        if n == 0 || n > 64 {
            return Err((400, "invalid encrypted group payload"));
        }
        for (recipient, payload) in payloads {
            if normalize_email(recipient).is_empty() || !valid_cipher_payload(payload.as_object()) {
                return Err((400, "invalid encrypted group payload"));
            }
        }
    } else if !valid_cipher_payload(Some(obj)) {
        return Err((400, "invalid encrypted message payload"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
    use super::*;

    fn temp_store(name: &str) -> (DataStore, std::path::PathBuf) {
        // Under full parallel load this box's tmpfs intermittently fails the
        // WAL setup inside DataStore::open with SQLITE_IOERR_READ. Retry on a
        // fresh dir — the tests are deterministic, an I/O error is never a
        // real assertion failure.
        let mut last_err = None;
        for attempt in 0..5u32 {
            let dir = std::env::temp_dir().join(format!(
                "dm-test-{name}-{}-{attempt}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(dir.join("data")).unwrap();
            match DataStore::open(&dir, &dir.join("data")) {
                Ok(store) => return (store, dir.join("data")),
                Err(e) => {
                    last_err = Some(e);
                    let _ = std::fs::remove_dir_all(&dir);
                    std::thread::sleep(std::time::Duration::from_millis(
                        50 * u64::from(attempt + 1),
                    ));
                }
            }
        }
        panic!("temp_store open failed after retries: {:?}", last_err);
    }

    #[test]
    fn expiry_keys_sort_and_match_js() {
        // Bun goldens: dmExpiryKey('B@x.c','a@x.c') === 'dm:a@x.c|b@x.c'.
        assert_eq!(dm_expiry_key("B@x.c", "a@x.c"), "dm:a@x.c|b@x.c");
        assert_eq!(dm_expiry_key("", ""), "dm:|");
        assert_eq!(group_expiry_key("abc"), "group:abc");
    }

    #[test]
    fn chat_expiry_validates_the_four_windows() {
        let (store, data_dir) = temp_store("expiry");
        for v in [30_000.0, 60_000.0, 300_000.0, 3_600_000.0] {
            let all = store.read_document(&data_dir.join("chat_expiry.json"), json!({}));
            let mut all = all.as_object().cloned().unwrap_or_default();
            all.insert("k".into(), json!(v));
            let _ = store.write_document(&data_dir.join("chat_expiry.json"), &Value::Object(all));
            assert_eq!(get_chat_expiry(&store, &data_dir, "k"), v);
        }
        for v in [0.0, 10_000.0, 7_200_000.0, -1.0] {
            let all = store.read_document(&data_dir.join("chat_expiry.json"), json!({}));
            let mut all = all.as_object().cloned().unwrap_or_default();
            all.insert("k".into(), json!(v));
            let _ = store.write_document(&data_dir.join("chat_expiry.json"), &Value::Object(all));
            assert_eq!(get_chat_expiry(&store, &data_dir, "k"), 0.0);
        }
        assert_eq!(get_chat_expiry(&store, &data_dir, ""), 0.0);
        assert_eq!(get_chat_expiry(&store, &data_dir, "missing"), 0.0);
    }

    #[test]
    fn prune_dms_keeps_100_per_convo_and_drops_expired_only_when_read() {
        let (store, data_dir) = temp_store("prune");
        let now = 1_000_000.0;
        // An UNREAD message past expiresAt survives; a READ one is purged.
        let unread = json!({"kind": "dm", "from": "a@x.c", "to": "b@x.c", "ts": 1.0, "read": false, "expiresAt": 500.0});
        let read = json!({"kind": "dm", "from": "a@x.c", "to": "b@x.c", "ts": 1.0, "read": true, "readAt": 2.0, "expiresAt": 500.0});
        let dms = vec![read.clone(), unread.clone()];
        let pruned = prune_dms(&store, &data_dir, &dms, false, now);
        assert_eq!(pruned, vec![unread]);
        // 101 same-conversation messages → newest 100 kept, oldest dropped.
        let many: Vec<Value> = (0..101)
            .map(|i| {
                json!({"kind": "dm", "from": "a@x.c", "to": "b@x.c", "ts": i as f64, "read": false})
            })
            .collect();
        let pruned = prune_dms(&store, &data_dir, &many, false, now);
        assert_eq!(pruned.len(), 100);
        assert_eq!(pruned[0].get("ts").and_then(|v| v.as_f64()), Some(1.0));
        assert_eq!(pruned[99].get("ts").and_then(|v| v.as_f64()), Some(100.0));
        // Different conversations keep separate budgets (75 each, all kept).
        let mixed: Vec<Value> = (0..150)
            .map(|i| {
                json!({"kind": "dm", "from": "a@x.c", "to": if i % 2 == 0 { "b@x.c" } else { "c@x.c" }, "ts": i as f64, "read": false})
            })
            .collect();
        let pruned = prune_dms(&store, &data_dir, &mixed, false, now);
        assert_eq!(pruned.len(), 150);
        let to_b = pruned
            .iter()
            .filter(|m| m.get("to").and_then(|v| v.as_str()) == Some("b@x.c"))
            .count();
        let to_c = pruned
            .iter()
            .filter(|m| m.get("to").and_then(|v| v.as_str()) == Some("c@x.c"))
            .count();
        assert_eq!(to_b, 75);
        assert_eq!(to_c, 75);
        // Group messages key off groupId.
        let g = vec![
            json!({"kind": "group", "groupId": "g1", "from": "a@x.c", "ts": 5.0, "readBy": ["a@x.c"]}),
        ];
        assert_eq!(prune_dms(&store, &data_dir, &g, false, now), g);
    }

    #[test]
    fn resolve_member_ref_ladder() {
        let (store, data_dir) = temp_store("addr");
        // profiles.json drives the index.
        let profiles = json!({
            "alice@student.rjuhsd.us": {"username": "alice"},
            "bob@student.rjuhsd.us": {"username": "bobby"},
        });
        let _ = store.write_document(&data_dir.join("profiles.json"), &profiles);
        let idx = build_address_index(&store, &data_dir);
        // Direct email.
        assert_eq!(
            resolve_member_ref(&idx, " Alice@Student.Rjuhsd.Us "),
            "alice@student.rjuhsd.us"
        );
        // Username.
        assert_eq!(resolve_member_ref(&idx, "BOBBY"), "bob@student.rjuhsd.us");
        // A non-email ref only matches through byUsername verbatim
        // (normalizeUsername is lowercase/trim only) — dots/plus stripping
        // happens INSIDE the email branch.
        assert_eq!(resolve_member_ref(&idx, "a.li+ce"), "");
        // The email branch first normalizes (dots/plus stripped in the local
        // part by normalizeEmail itself) → "ali@…" is unknown and the local
        // fallback "ali" is not a username, so the norm is returned verbatim.
        assert_eq!(
            resolve_member_ref(&idx, "a.li+ce@student.rjuhsd.us"),
            "ali@student.rjuhsd.us"
        );
        // A dotted known address normalizes onto the index.
        assert_eq!(
            resolve_member_ref(&idx, "b.obby@student.rjuhsd.us"),
            "bob@student.rjuhsd.us"
        );
        // Unknown email stays normalized (JS returns norm).
        assert_eq!(resolve_member_ref(&idx, "zed@nowhere.io"), "zed@nowhere.io");
        // Unknown non-email ref → ''.
        assert_eq!(resolve_member_ref(&idx, "nobody"), "");
        assert_eq!(resolve_member_ref(&idx, ""), "");
    }

    #[test]
    fn envelope_validation_matches_js_ladder() {
        let sender = "04".to_string() + &"a".repeat(128);
        let good = serde_json::to_string(&json!({
            "e2e": true, "version": 1, "senderPubKey": sender,
            "iv": "0102030405060708090a0b0c", "ciphertext": "deadbeef"
        }))
        .unwrap();
        assert!(validate_e2e_envelope(&good, false).is_ok());
        // Group envelope: needs payloads 1..=64.
        let mut group = json!({
            "e2e": true, "version": 2, "senderPubKey": sender, "group": true,
            "payloads": {}
        });
        group["payloads"]["a@x.c"] =
            json!({"iv": "0102030405060708090a0b0c", "ciphertext": "ffee"});
        let group_txt = serde_json::to_string(&group).unwrap();
        assert!(validate_e2e_envelope(&group_txt, true).is_ok());
        // Empty payloads → invalid.
        group["payloads"] = json!({});
        assert_eq!(
            validate_e2e_envelope(&serde_json::to_string(&group).unwrap(), true),
            Err((400, "invalid encrypted group payload"))
        );
        // Target mismatch.
        assert_eq!(
            validate_e2e_envelope(&good, true),
            Err((400, "encrypted message target mismatch"))
        );
        // Bad version.
        let bad_ver = serde_json::to_string(&json!({"e2e": true, "version": 4, "senderPubKey": sender, "iv": "0102030405060708090a0b0c", "ciphertext": "ff"})).unwrap();
        assert_eq!(
            validate_e2e_envelope(&bad_ver, false),
            Err((400, "unsupported encrypted message version"))
        );
        // Bad sender key.
        assert_eq!(
            validate_e2e_envelope(&serde_json::to_string(&json!({"e2e": true, "senderPubKey": "zz", "iv": "0102030405060708090a0b0c", "ciphertext": "ff"})).unwrap(), false),
            Err((400, "invalid encrypted sender key"))
        );
        // Missing ciphertext.
        assert_eq!(
            validate_e2e_envelope(
                &serde_json::to_string(
                    &json!({"e2e": true, "senderPubKey": sender, "iv": "0102030405060708090a0b0c"})
                )
                .unwrap(),
                false
            ),
            Err((400, "invalid encrypted message payload"))
        );
        // recipientPubKey optional but must be valid when present.
        let with_recip = serde_json::to_string(&json!({"e2e": true, "senderPubKey": sender, "iv": "0102030405060708090a0b0c", "ciphertext": "ff", "recipientPubKey": sender})).unwrap();
        assert!(validate_e2e_envelope(&with_recip, false).is_ok());
        // Not JSON at all.
        assert_eq!(
            validate_e2e_envelope("{nope", false),
            Err((400, "invalid encrypted message"))
        );
    }
}
