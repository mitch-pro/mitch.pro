//! Matrix message moderation and word filter.
//!
//! Port of  matching blocked terms, entity decoding,
//! and word boundary normalization.

use std::sync::OnceLock;

/// Terms blocked in Matrix chat messages.
pub const MATRIX_BLOCKED_TERMS: &[&str] = &[
    "porn",
    "pornography",
    "hentai",
    "nude",
    "nudity",
    "sex",
    "nsfw",
    "xxx",
    "cocaine",
    "heroin",
    "meth",
    "marijuana",
    "weed",
    "fentanyl",
    "lsd",
    "ecstasy",
    "gun",
    "firearm",
    "bomb",
    "explosive",
    "knife",
    "shooting",
    "murder",
    "gore",
    "suicide",
    "self harm",
    "cutting",
    "casino",
    "betting",
    "sportsbook",
    "poker",
    "hacking",
    "exploit",
    "malware",
    "ransomware",
    "ddos",
    "phishing",
    "keylogger",
    "proxy",
    "vpn",
    "unblock",
    "unblocked games",
    "bypass filter",
    "tor",
    "scramjet",
    "scrammerjet",
    "ultraviolet",
    "uv proxy",
    "rammerhead",
    "corrosion",
    "rhodium",
    "wisp server",
    "bare server",
    "holy unblocker",
    "incognito unblocker",
    "titanium network",
    "interstellar proxy",
    "selenite",
    "nebula proxy",
    "metallic proxy",
    "shuttle proxy",
    "artclass",
    "luna proxy",
    "croxyproxy",
    "proxysite",
    "kproxy",
    "web proxy",
    "gn math",
    "classroom 6x",
    "coolmath",
    "coolmath games",
    "cool math games",
    "math playground",
    "hoodamath",
    "tyrone unblocked",
    "tyrone games",
    "ubg100",
    "crazygames",
    "poki",
    "now gg",
    "goguardian bypass",
    "securly bypass",
    "bypass goguardian",
    "bypass securly",
    "disable goguardian",
    "kill goguardian",
    "extension killer",
    "tab cloaker",
    "tab cloaking",
    "dextensify",
    "ltmeat",
    "caub",
    "blooket hack",
    "blooket bot",
    "kahoot bot",
    "kahoot spammer",
    "gimkit bot",
    "roblox",
    "fortnite",
    "minecraft",
    "steam",
    "discord",
    "tiktok",
    "gaming",
    "games",
    "torrent",
    "pirate bay",
    "cracked",
    "warez",
    "rom download",
    "anonymous chat",
    "omegle",
    "omegle style services",
    "chatroom",
    "chatgpt",
    "gemini",
    "claude",
    "ai chatbot",
    "fuck",
    "fucking",
    "fucker",
    "motherfucker",
    "shit",
    "bullshit",
    "bitch",
    "asshole",
    "bastard",
    "cunt",
    "dick",
    "pussy",
    "faggot",
    "nigger",
    "niggers",
    "nigga",
    "niggas",
    "niggah",
    "niggaz",
    "nigguh",
    "faggots",
    "retard",
    "retards",
];

/// Slurs rejected anywhere inside a name, not just as a whole word, so
/// "mitch.<slur>" or "mitch<slur>" can't slip past the word-boundary chat filter.
const NAME_BLOCKED_SUBSTRINGS: &[&str] = &["nigger", "nigga", "faggot", "retard"];

/// True when a username, display name or nickname contains a blocked term:
/// any chat-blocked word, or a severe slur as a substring after folding case,
/// separators and common character swaps (1->i, 3->e, 0->o, 4->a, 5->s, @->a, $->s).
pub fn name_blocked(text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    if matrix_message_blocked(&serde_json::json!({ "body": text })) {
        return true;
    }
    let folded: String = text
        .to_lowercase()
        .chars()
        .map(|c| match c {
            '1' | '!' | '|' => 'i',
            '3' => 'e',
            '0' => 'o',
            '4' | '@' => 'a',
            '5' | '$' => 's',
            other => other,
        })
        .filter(|c| c.is_alphanumeric())
        .collect();
    NAME_BLOCKED_SUBSTRINGS.iter().any(|t| folded.contains(t))
}

/// Cheap pre-check for `strip_blocked_name_events`: true when the text might
/// contain a blocked name (the same folding as `name_blocked`, applied to the
/// whole body). A false positive only costs a JSON parse; it never changes output.
pub fn text_may_contain_blocked_name(text: &str) -> bool {
    let folded: String = text
        .to_lowercase()
        .chars()
        .map(|c| match c {
            '1' | '!' | '|' => 'i',
            '3' => 'e',
            '0' => 'o',
            '4' | '@' => 'a',
            '5' | '$' => 's',
            other => other,
        })
        .filter(|c| c.is_alphanumeric())
        .collect();
    NAME_BLOCKED_SUBSTRINGS.iter().any(|t| folded.contains(t))
}

/// True for an `m.room.member` event that shows a blocked name: the user ID's
/// localpart, the display name, or the previous display name (so "X changed
/// their name from <blocked>" lines go too).
fn member_event_shows_blocked_name(ev: &serde_json::Value) -> bool {
    if ev.get("type").and_then(|v| v.as_str()) != Some("m.room.member") {
        return false;
    }
    let localpart = ev
        .get("state_key")
        .and_then(|v| v.as_str())
        .map(|k| k.trim_start_matches('@').split(':').next().unwrap_or(""))
        .unwrap_or("");
    let names = [
        localpart,
        ev.pointer("/content/displayname")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        ev.pointer("/unsigned/prev_content/displayname")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    ];
    names.iter().any(|n| name_blocked(n))
}

/// Removes, from every event array in a Matrix response (sync timeline/state,
/// /messages, /members, /context), the membership events that show a blocked
/// name. Redaction can't hide these: a Matrix user ID is permanent, so a
/// redacted join line still renders the ID. Returns true if anything was removed.
pub fn strip_blocked_name_events(value: &mut serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(items) => {
            let before = items.len();
            items.retain(|item| !member_event_shows_blocked_name(item));
            let mut changed = items.len() != before;
            for item in items.iter_mut() {
                changed |= strip_blocked_name_events(item);
            }
            changed
        }
        serde_json::Value::Object(map) => {
            let mut changed = false;
            for v in map.values_mut() {
                changed |= strip_blocked_name_events(v);
            }
            changed
        }
        _ => false,
    }
}

/// Storage domain of student accounts (see `auth::normalize_email`).
pub const STUDENT_EMAIL_DOMAIN: &str = "@student.rjuhsd.us";

/// True for a normalized student address. Everything else (and not staff) is
/// an "outside" account.
pub fn is_student_email(norm_email: &str) -> bool {
    norm_email.to_lowercase().ends_with(STUDENT_EMAIL_DOMAIN)
}

fn approvals_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Outside-account Matrix approval status: `Some("approved" | "pending" |
/// "denied")`, or `None` if the account never asked.
pub fn outside_approval_status(store: &crate::data::DataStore, norm_email: &str) -> Option<String> {
    let conn = store.conn();
    conn.query_row(
        "SELECT status FROM matrix_outside_approvals WHERE email = ?1",
        [norm_email],
        |row| row.get::<_, String>(0),
    )
    .ok()
}

/// Records that an outside account tried to chat, so moderators can see it.
/// Never downgrades an existing decision.
pub fn outside_approval_request(store: &crate::data::DataStore, norm_email: &str) {
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT OR IGNORE INTO matrix_outside_approvals (email, status, requested_at) VALUES (?1, 'pending', ?2)",
        rusqlite::params![norm_email, approvals_now_ms()],
    );
}

/// A moderator's decision. Creates the row if the account hasn't asked yet.
pub fn outside_approval_decide(
    store: &crate::data::DataStore,
    norm_email: &str,
    approved: bool,
    decided_by: &str,
) -> bool {
    let status = if approved { "approved" } else { "denied" };
    let conn = store.conn();
    conn.execute(
        "INSERT INTO matrix_outside_approvals (email, status, requested_at, decided_at, decided_by)
         VALUES (?1, ?2, ?3, ?3, ?4)
         ON CONFLICT(email) DO UPDATE SET status = excluded.status,
            decided_at = excluded.decided_at, decided_by = excluded.decided_by",
        rusqlite::params![norm_email, status, approvals_now_ms(), decided_by],
    )
    .is_ok()
}

/// Accounts waiting for a decision, oldest first.
pub fn outside_approvals_pending(store: &crate::data::DataStore) -> Vec<serde_json::Value> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(
        "SELECT email, requested_at FROM matrix_outside_approvals WHERE status = 'pending' ORDER BY requested_at",
    ) else {
        return Vec::new();
    };
    stmt.query_map([], |row| {
        Ok(serde_json::json!({ "email": row.get::<_, String>(0)?, "requestedAt": row.get::<_, i64>(1)? }))
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

/// Normalizes text for word matching: lowercase, non-alphanumeric converted to
/// spaces, whitespace collapsed and trimmed.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_was_space = true;
    for c in text.chars() {
        if c.is_alphanumeric() {
            for lc in c.to_lowercase() {
                out.push(lc);
            }
            last_was_space = false;
        } else if !last_was_space {
            out.push(' ');
            last_was_space = true;
        }
    }
    out.trim().to_string()
}

/// Fallback for regexes to avoid .
fn unreachable_regex() -> regex::Regex {
    regex::Regex::new("a^").unwrap_or_else(|_| match regex::Regex::new("") {
        Ok(r) => r,
        Err(_) => unreachable!(),
    })
}

/// HTML entity decoder for message content inspection.
pub fn decode_entities(text: &str) -> String {
    static ENTITY_NUM_RE: OnceLock<regex::Regex> = OnceLock::new();
    let num_re = ENTITY_NUM_RE.get_or_init(|| {
        regex::Regex::new(r"(?i)&#(x[0-9a-f]+|\d+);?").unwrap_or_else(|_| unreachable_regex())
    });

    let s = num_re.replace_all(text, |caps: &regex::Captures| {
        let capture = &caps[1];
        let code = if capture.starts_with('x') || capture.starts_with('X') {
            u32::from_str_radix(&capture[1..], 16).ok()
        } else {
            capture.parse::<u32>().ok()
        };
        match code.and_then(char::from_u32) {
            Some(ch) => ch.to_string(),
            None => caps[0].to_string(),
        }
    });

    s.replace("&nbsp;", " ")
        .replace("&NBSP;", " ")
        .replace("&amp;", "&")
        .replace("&AMP;", "&")
        .replace("&lt;", "<")
        .replace("&LT;", "<")
        .replace("&gt;", ">")
        .replace("&GT;", ">")
        .replace("&quot;", "\"")
        .replace("&QUOT;", "\"")
        .replace("&apos;", "'")
        .replace("&APOS;", "'")
}

fn needles() -> &'static [String] {
    static NEEDLES: OnceLock<Vec<String>> = OnceLock::new();
    NEEDLES.get_or_init(|| {
        MATRIX_BLOCKED_TERMS
            .iter()
            .map(|t| format!(" {} ", normalize(t)))
            .collect()
    })
}

/// Checks whether a Matrix message content object contains blocked terms.
/// Inspects body, filename, caption, and HTML formatted body (with tags
/// removed).
pub fn matrix_message_blocked(content: &serde_json::Value) -> bool {
    if !content.is_object() {
        return false;
    }
    static TAG_RE: OnceLock<regex::Regex> = OnceLock::new();
    let tag_re = TAG_RE
        .get_or_init(|| regex::Regex::new(r"<[^>]*>").unwrap_or_else(|_| unreachable_regex()));

    let all_needles = needles();

    // Check main object and m.new_content edit replacement
    let mut variants = vec![content];
    if let Some(new_content) = content.get("m.new_content") {
        if new_content.is_object() {
            variants.push(new_content);
        }
    }

    for variant in variants {
        let mut texts = Vec::new();
        if let Some(body) = variant.get("body").and_then(|v| v.as_str()) {
            texts.push(body.to_string());
        }
        if let Some(filename) = variant.get("filename").and_then(|v| v.as_str()) {
            texts.push(filename.to_string());
        }
        if let Some(caption) = variant.get("caption").and_then(|v| v.as_str()) {
            texts.push(caption.to_string());
        }
        if let Some(formatted_body) = variant.get("formatted_body").and_then(|v| v.as_str()) {
            // Removing formatting catches split words such as pro<b>x</b>y
            let stripped_empty = tag_re.replace_all(formatted_body, "").to_string();
            let stripped_space = tag_re.replace_all(formatted_body, " ").to_string();
            texts.push(decode_entities(&stripped_empty));
            texts.push(decode_entities(&stripped_space));
        }

        for text in texts {
            let padded = format!(" {} ", normalize(&text));
            for needle in all_needles {
                if padded.contains(needle) {
                    return true;
                }
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {

    #[test]
    fn strips_member_events_with_blocked_names_from_any_event_array() {
        let mut v = serde_json::json!({
            "rooms": { "join": { "!r:mitch.pro": {
                "timeline": { "events": [
                    { "type": "m.room.member", "state_key": "@mitch.nigger:mitch.pro",
                      "content": { "membership": "join" } },
                    { "type": "m.room.member", "state_key": "@fine:mitch.pro",
                      "content": { "membership": "join", "displayname": "Sam" } },
                    { "type": "m.room.member", "state_key": "@sam:mitch.pro",
                      "content": { "membership": "join", "displayname": "Sam" },
                      "unsigned": { "prev_content": { "displayname": "mitchnigger" } } },
                    { "type": "m.room.message", "content": { "body": "hi" } }
                ] },
                "state": { "events": [
                    { "type": "m.room.member", "state_key": "@x:mitch.pro",
                      "content": { "membership": "ban", "displayname": "n1gger" } }
                ] }
            } } }
        });
        assert!(strip_blocked_name_events(&mut v));
        let tl = &v["rooms"]["join"]["!r:mitch.pro"]["timeline"]["events"];
        assert_eq!(tl.as_array().unwrap().len(), 2);
        assert!(v["rooms"]["join"]["!r:mitch.pro"]["state"]["events"]
            .as_array()
            .unwrap()
            .is_empty());
        // Already-clean responses are left alone.
        assert!(!strip_blocked_name_events(&mut v));
        assert!(text_may_contain_blocked_name(
            "{\"state_key\":\"@mitch.n1gger:x\"}"
        ));
        assert!(!text_may_contain_blocked_name("{\"body\":\"hello\"}"));
    }

    fn approvals_store() -> (std::path::PathBuf, crate::data::DataStore) {
        let dir = std::env::temp_dir().join(format!(
            "mitch-lib-matrix-approvals-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap_or_default();
        let store = crate::data::DataStore::open(&dir, &dir.join("data"))
            .unwrap_or_else(|e| panic!("store: {e}"));
        (dir, store)
    }

    #[test]
    fn names_with_slurs_are_blocked_even_when_joined_to_other_text() {
        assert!(name_blocked("mitch.nigger"));
        assert!(name_blocked("MitchNigga"));
        assert!(name_blocked("n1gg3r"));
        assert!(name_blocked("some faggots"));
        assert!(!name_blocked("Mitch"));
        assert!(!name_blocked("Scunthorpe United"));
        assert!(!name_blocked(""));
    }

    #[test]
    fn chat_filter_catches_plural_slur_forms() {
        assert!(matrix_message_blocked(
            &serde_json::json!({ "body": "you niggers" })
        ));
        assert!(matrix_message_blocked(
            &serde_json::json!({ "body": "retards" })
        ));
    }

    #[test]
    fn outside_approval_lifecycle() {
        let (dir, store) = approvals_store();
        let email = "someone@example.com";
        assert_eq!(outside_approval_status(&store, email), None);
        outside_approval_request(&store, email);
        assert_eq!(
            outside_approval_status(&store, email).as_deref(),
            Some("pending")
        );
        assert_eq!(outside_approvals_pending(&store).len(), 1);
        assert!(outside_approval_decide(&store, email, true, "mod@x"));
        assert_eq!(
            outside_approval_status(&store, email).as_deref(),
            Some("approved")
        );
        // A later chat attempt must not undo a decision.
        outside_approval_request(&store, email);
        assert_eq!(
            outside_approval_status(&store, email).as_deref(),
            Some("approved")
        );
        assert!(outside_approvals_pending(&store).is_empty());
        assert!(is_student_email("a@student.rjuhsd.us"));
        assert!(!is_student_email("a@example.com"));
        let _ = std::fs::remove_dir_all(dir);
    }
    use super::*;
    use serde_json::json;

    #[test]
    fn test_clean_message_passes() {
        let msg = json!({
            "msgtype": "m.text",
            "body": "Hello world, hope you have a nice class today!"
        });
        assert!(!matrix_message_blocked(&msg));
    }

    #[test]
    fn test_innocent_substrings_pass() {
        let msg = json!({
            "body": "This class uses a new method to study Torres Strait."
        });
        assert!(!matrix_message_blocked(&msg));
    }

    #[test]
    fn test_blocked_word_triggers() {
        let msg = json!({
            "body": "Let's play roblox right now"
        });
        assert!(matrix_message_blocked(&msg));

        let msg2 = json!({
            "body": "Do you know a good VPN?"
        });
        assert!(matrix_message_blocked(&msg2));
    }

    #[test]
    fn test_html_tag_bypass_detected() {
        let msg = json!({
            "body": "check this out",
            "format": "org.matrix.custom.html",
            "formatted_body": "pro<b>x</b>y site here"
        });
        assert!(matrix_message_blocked(&msg));
    }

    #[test]
    fn test_edit_content_checked() {
        let msg = json!({
            "body": "clean text initially",
            "m.new_content": {
                "body": "edited to mention discord"
            }
        });
        assert!(matrix_message_blocked(&msg));
    }
}
