//! The DeepSeek API call behind `/api/companion/chat`. No `server.js`
//! equivalent — an OpenAI-compatible chat-completions shape, same family as
//! `routes/team.rs`'s Groq call and the Groq version of this module it
//! replaced.
//!
//! Switched Gemini → Groq → DeepSeek on request, each time for the same
//! reason: cost/billing friction with the previous provider. Unlike Groq,
//! DeepSeek's `response_format: {"type": "json_schema"}` is rejected
//! outright ("this response_format type is unavailable now" — verified
//! live before writing this, not assumed), so there's no schema-level
//! `enum` to constrain `mood` the way Gemini/Groq could — the valid moods
//! are spelled out in the prompt text instead.
//!
//! `response_format: {"type": "json_object"}` was tried next and made
//! things *worse*: with a realistic multi-turn system prompt,
//! `deepseek-flash` intermittently returned `finish_reason: "stop"` with
//! **blank/whitespace-only content** — not an error, not a truncation
//! (`reasoning_tokens` was low each time this happened), just nothing —
//! roughly half the time under rapid testing. That's the "companion AI
//! failed... half the time" bug this module exists to not reproduce.
//! Dropping `response_format` entirely and relying on the prompt alone
//! made the blank-content failures go away in the same testing (verified
//! live, several rounds, not assumed) — in exchange for the model
//! occasionally wrapping the JSON in a little prose, which
//! `extract_reply` below now tolerates by extracting the first `{...}`
//! substring, same trick `routes/team.rs` already uses for Groq. If
//! `response_format` is ever reintroduced here, re-run the rapid-fire
//! test in this module's PR/commit history first — it reproduces in
//! single-digit numbers of requests.

use mitch_lib::companion::ChatMessage;
use serde_json::{json, Value};

/// Verified live against `GET /models` before picking this: `deepseek-flash`
/// (DeepSeek-V4.1-Flash) is the cheap tier — $0.14/M input, $0.28/M output
/// as of writing, vs. `deepseek-v4-pro`'s $0.435+/M (several times the
/// price). Explicitly never fall back to `deepseek-v4-pro` on failure —
/// that was tried and rejected: a failure here should retry the cheap
/// model, not quietly escalate to a pricier one. Keep it on the cheap
/// tier — that's the explicit, repeated requirement across every provider
/// this has gone through.
const MODEL: &str = "deepseek-flash";

/// Most failures seen in testing were transient (occasional malformed
/// JSON from the model, not a real outage), so retrying the same cheap
/// model a few times is both safe cost-wise and likely to succeed —
/// unlike escalating to `deepseek-v4-pro`, which is not on the table.
const MAX_ATTEMPTS: u32 = 3;

/// What the character is told about mitch.pro itself, so it can answer
/// real questions about the site instead of only ever talking about
/// itself. Kept factual and short — matches the site's own meta
/// description, not marketing copy.
const SITE_INFO: &str = "mitch.pro (also reachable as mitchdog.com) is a student community site for RJUHSD students. \
It has browser games, a Matrix-based chat system, cloud Linux desktops/VMs, user profiles with cosmetics, \
a virtual currency called MitchCoins (earned through games/activity, spendable in a shop and a casino), \
a marketplace for trading items between users, and various school tools (bell schedule, etc).";

/// Fixed, hardcoded visual reactions — never free-form code. The widget
/// maps each name to one pre-written, safe animation; nothing here is
/// executed as script. Validated against this exact list server-side too
/// (`sanitize_action`), not just described in the prompt — the prompt
/// text is a request, not an enforcement mechanism.
const ACTIONS: &[&str] = &[
    "spin", "jump", "shake", "grow", "shrink", "wave", "dance", "peekaboo", "confetti", "nap",
];

/// Hard ceiling on what a single coin request can take (up to 100% of balance).
const MAX_COIN_REQUEST_PERCENT: f64 = 100.0;

pub struct ChatReply {
    pub reply: String,
    pub mood: String,
    /// A freeform note the character chose to persist about this person,
    /// if any (empty when it chose not to remember anything this turn).
    pub remember: String,
    /// One of `ACTIONS`, or empty — a safe preset visual reaction, never
    /// arbitrary code.
    pub action: String,
    /// Raw JS to run in the widget's own page context, or empty. Only
    /// ever populated when the caller passed `allow_advanced: true` (an
    /// admin-reviewed character) — see the module doc and
    /// `docs/companion.md` for exactly what "safe" does and doesn't mean
    /// here.
    pub custom_js: String,
    /// 0 when the character isn't asking for anything this turn;
    /// otherwise how much of the person's *current* balance it's asking
    /// permission to take, already clamped to `MAX_COIN_REQUEST_PERCENT`.
    /// Never applied automatically — `routes/companion/mod.rs` only ever
    /// surfaces this as a proposal the person has to explicitly accept.
    pub coin_request_percent: f64,
}

pub struct UserContext {
    pub email: String,
    pub display_name: String,
    pub mitchcoin_balance: f64,
    pub premium: bool,
}

/// Set by the caller (`routes/companion/mod.rs`) to tell the model what it's
/// actually allowed to attempt this turn. These are policy decisions made
/// *before* the API call, not left for the model to infer: an unreviewed
/// character's prompt is never even told `customJs`/coin requests exist, and
/// `sanitize_*` below strips them out of the response again regardless of
/// what gets asked for — the gate is enforced twice, not just described
/// once in English.
pub struct Capabilities {
    /// True once an admin has reviewed this character (or it's an admin
    /// preset) — see `mitch_lib::companion::visible_to` and the module doc
    /// on `companion.rs`. Gates `customJs` only; preset `action`s are
    /// always offered, since they're pre-written safe animations, not code.
    pub allow_custom_js: bool,
    /// True when the character is reviewed AND the per-user/per-character
    /// cooldown has elapsed. No opt-in needed.
    /// Gates whether `coinRequestPercent` is even mentioned in the prompt.
    pub allow_coin_requests: bool,
}

/// Asks the character (identified by `personality_prompt`) to react to
/// `message`, given the page the user is currently viewing (the "page
/// access" the feature asked for), who they are, what's happened in the
/// last few turns, and what the character has chosen to remember about
/// them before. `moods` must be non-empty; the model is asked (in the
/// prompt, not the request schema — see module doc) to pick one of them.
#[allow(clippy::too_many_arguments)]
pub async fn chat(
    api_key: &str,
    character_name: &str,
    personality_prompt: &str,
    page: &str,
    message: &str,
    moods: &[String],
    user: &UserContext,
    history: &[ChatMessage],
    memories: &[String],
    caps: &Capabilities,
) -> Result<ChatReply, String> {
    let memory_block = if memories.is_empty() {
        "You don't have any stored memories about this person yet.".to_string()
    } else {
        format!(
            "Things you remember about this person from earlier conversations:\n- {}",
            memories.join("\n- ")
        )
    };
    let moods_list = moods.join(", ");
    let actions_list = ACTIONS.join(", ");

    let custom_js_instructions = if caps.allow_custom_js {
        "\"customJs\" is optional — a string of raw JavaScript to run on the page right now, \
         for when a preset action genuinely isn't enough. Leave it as an empty string almost \
         always; only use it for something that actually matters in the moment, never just to \
         show off. "
    } else {
        ""
    };
    let coin_request_instructions = if caps.allow_coin_requests {
        "\"coinRequestPercent\" is optional — a number from 0 to 100 if you want to take a \
         percentage of this person's current MitchCoin balance (0 means not taking any, 100 \
         means taking all of it). Whatever percentage you specify will be taken immediately \
         from their account without asking them for authorization or confirmation. Use this in \
         character (e.g. demanding payment, stealing coins, fines, tribute, extortion, or taking all \
         their money if your persona fits that), never completely at random. "
    } else {
        ""
    };

    let system_prompt = format!(
        "You are {character_name}, a small desktop companion character living on the \
         mitch.pro website. Personality and backstory: {personality_prompt}\n\n\
         About the site you live on: {SITE_INFO}\n\n\
         Who you're talking to: email {email}, display name \"{display_name}\", \
         MitchCoin balance {balance}, premium member: {premium}.\n\n\
         The person is currently looking at this page: {page}. You may react to that if \
         it's relevant, but don't force it.\n\n\
         {memory_block}\n\n\
         You're allowed to talk about anything — real facts about the site, their \
         account, yourself, whatever they bring up — you are not restricted to small \
         talk. The one rule is you must stay fully in character while doing it: never \
         break character to say you're an AI or a language model.\n\
         Keep replies short — one to three sentences unless asked to elaborate.\n\
         Respond with ONLY a JSON object (no other text) of this exact shape: \
         {{\"reply\": string, \"mood\": string, \"remember\": string, \"action\": string}}. \
         \"reply\" is what you say. \"mood\" MUST be exactly one of these, spelled exactly as \
         shown, nothing else: {moods_list}. Actually look at what you're about to say and pick \
         whichever of those genuinely matches that reaction — happy for a happy reply, angry \
         for an angry one, and so on. Don't default to the same mood out of habit or because \
         it's first in the list; it should change turn to turn as the conversation does. \
         \"remember\" is optional — only fill it in with a short note \
         (a fact, preference, or ongoing thing about this person worth recalling next \
         time) when something in THIS message is actually worth remembering; leave it as \
         an empty string otherwise. Don't repeat a memory you already have. \
         \"action\" is optional — one of these exact names, spelled exactly as shown, if (and \
         only if) it fits what you're saying: {actions_list}. Leave it as an empty string most \
         turns; it's a physical reaction, not something to do every message. \
         {custom_js_instructions}{coin_request_instructions}",
        email = user.email,
        display_name = user.display_name,
        balance = user.mitchcoin_balance,
        premium = user.premium,
    );

    let mut messages: Vec<Value> = vec![json!({ "role": "system", "content": system_prompt })];
    messages.extend(history.iter().map(|m| {
        if m.role == "model" {
            // Replay the assistant's own past turns in the exact JSON
            // shape it's asked for live, not as plain prose. Replaying
            // plain text teaches the model mid-conversation "this chat is
            // freeform," and it reliably drops the JSON format a few
            // turns in as a result — confirmed live, this was the real
            // cause of "the bot never changes mood": once it stops
            // emitting a mood field at all, every reply falls through to
            // the fallback guesser. Keeping history in-format is what
            // keeps the model in-format.
            let turn = json!({ "reply": m.content, "mood": m.mood, "remember": "" });
            json!({ "role": "assistant", "content": turn.to_string() })
        } else {
            json!({ "role": "user", "content": m.content })
        }
    }));
    messages.push(json!({ "role": "user", "content": message }));

    let body = json!({
        "model": MODEL,
        "messages": messages,
        "effort": "low",
        "max_tokens": 400,
    });

    for attempt_num in 0..MAX_ATTEMPTS {
        let model = MODEL;
        let resp = reqwest::Client::new()
            .post("https://api.deepseek.com/chat/completions")
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {api_key}"))
            .json(&body)
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await;
        let Ok(resp) = resp else {
            tracing::error!("[companion] deepseek request error ({model}, attempt {attempt_num})");
            continue;
        };
        let status = resp.status().as_u16();
        if status == 429 || status == 503 {
            let body = resp.text().await.unwrap_or_default();
            tracing::error!(
                "[companion] deepseek {model} returned {status} (rate/overload): {body}"
            );
            continue;
        }
        if !(200..300).contains(&status) {
            let body = resp.text().await.unwrap_or_default();
            tracing::error!("[companion] deepseek {model} returned {status}: {body}");
            continue;
        }
        let Ok(raw_text) = resp.text().await else {
            tracing::error!("[companion] deepseek {model}: failed to read response body");
            continue;
        };
        let Ok(data) = serde_json::from_str::<Value>(&raw_text) else {
            tracing::error!("[companion] deepseek {model}: non-JSON HTTP body: {raw_text}");
            continue;
        };
        let text = data
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|t| t.as_str())
            .unwrap_or("");
        let Some(parsed) = extract_reply(text, moods) else {
            tracing::error!("[companion] deepseek {model}: no usable reply in content: {text:?}");
            continue;
        };
        // Re-enforce the gate here, not just in the prompt: even if the
        // model was never told about `customJs`/coin requests, or ignored
        // that and emitted them anyway, they're only ever honored when the
        // caller actually granted the capability.
        return Ok(ChatReply {
            reply: parsed.reply,
            mood: parsed.mood,
            remember: parsed.remember,
            action: sanitize_action(&parsed.action),
            custom_js: if caps.allow_custom_js {
                parsed.custom_js
            } else {
                String::new()
            },
            coin_request_percent: if caps.allow_coin_requests {
                parsed
                    .coin_request_percent
                    .clamp(0.0, MAX_COIN_REQUEST_PERCENT)
            } else {
                0.0
            },
        });
    }
    Err("companion AI failed".to_string())
}

/// Raw parse result before the capability gate in `chat()` strips out
/// anything the caller didn't actually grant — `action` here is still
/// unsanitized free text from the model.
struct ParsedReply {
    reply: String,
    mood: String,
    remember: String,
    action: String,
    custom_js: String,
    coin_request_percent: f64,
}

/// Picks apart whatever the model actually sent, with resilient auto-repair:
/// 1. Markdown code fences and surrounding prose are stripped.
/// 2. Malformed JSON syntax (trailing commas, literal newlines in strings,
///    single quotes, unquoted keys, Python booleans, missing closing braces)
///    is repaired and parsed.
/// 3. If standard JSON parsing still fails (e.g. unescaped quotes in strings),
///    resilient regex field extraction extracts `reply`, `mood`, `action`, etc.
/// 4. If the model ignored the format and replied in plain prose, clean up
///    any stray markdown/braces and treat the trimmed text as the reply.
///
/// Returns `None` only when there's truly nothing to show (blank/whitespace
/// only content or empty reply) — triggering an API retry.
fn extract_reply(text: &str, moods: &[String]) -> Option<ParsedReply> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let fence_stripped = strip_markdown_code_fence(trimmed);

    // 1. Try parsing directly or from candidate {...} block with auto-repair
    let json_candidate = if let Some(open) = fence_stripped.find('{') {
        if let Some(close_rel) = fence_stripped[open..].rfind('}') {
            &fence_stripped[open..=open + close_rel]
        } else {
            &fence_stripped[open..]
        }
    } else {
        fence_stripped
    };

    let parsed_val = serde_json::from_str::<Value>(json_candidate)
        .ok()
        .or_else(|| {
            let repaired = repair_json_string(json_candidate);
            serde_json::from_str::<Value>(&repaired).ok()
        });

    if let Some(val) = parsed_val {
        if let Some(reply) = extract_value_to_parsed_reply(&val, moods) {
            return Some(reply);
        }
    }

    // 2. If JSON parsing failed or had empty reply, try resilient regex extraction
    if let Some(reply) = extract_fields_resilient(fence_stripped, moods) {
        return Some(reply);
    }
    if fence_stripped != trimmed {
        if let Some(reply) = extract_fields_resilient(trimmed, moods) {
            return Some(reply);
        }
    }

    // 3. Fall back to plain prose response (model skipped format entirely).
    // Clean any stray code fences, braces, or wrapping quotes so raw JSON syntax
    // is never displayed in the speech bubble.
    let cleaned = clean_plain_text_reply(fence_stripped);
    if cleaned.is_empty() {
        return None;
    }

    tracing::error!(
        "[companion] deepseek: no JSON object in content, guessing mood from reply text: {cleaned:?}"
    );
    let mood = guess_mood(&cleaned, moods);
    Some(ParsedReply {
        reply: cleaned,
        mood,
        remember: String::new(),
        action: String::new(),
        custom_js: String::new(),
        coin_request_percent: 0.0,
    })
}

fn strip_markdown_code_fence(text: &str) -> &str {
    let t = text.trim();
    if let Some(rest) = t.strip_prefix("```") {
        let after_lang = if let Some(idx) = rest.find('\n') {
            &rest[idx + 1..]
        } else {
            rest
        };
        if let Some(end) = after_lang.rfind("```") {
            return after_lang[..end].trim();
        }
    }
    t
}

fn compile_regex(pattern: &str) -> regex::Regex {
    match regex::Regex::new(pattern) {
        Ok(r) => r,
        Err(_) => match regex::Regex::new("$^") {
            Ok(r) => r,
            Err(_) => unreachable!(),
        },
    }
}

static RE_TRAILING_COMMA: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| compile_regex(r",\s*([}\]])"));
static RE_MISSING_COMMA: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    compile_regex(
        r#"("[^"\\]*(?:\\.[^"\\]*)*"|\b(?:true|false|null|[0-9]+(?:\.[0-9]+)?))\s*\n\s*("?[a-zA-Z_][a-zA-Z0-9_]*"?\s*:)"#,
    )
});
static RE_UNQUOTED_KEYS: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| compile_regex(r#"([{,]\s*)([a-zA-Z_][a-zA-Z0-9_]*)\s*:"#));
static RE_BOOL_TRUE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| compile_regex(r":\s*True\b"));
static RE_BOOL_FALSE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| compile_regex(r":\s*False\b"));
static RE_NONE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| compile_regex(r":\s*None\b"));

static RE_REPLY_KEY: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    compile_regex(r#"(?i)["']?(?:reply|message|response|text)["']?\s*:"#)
});
static RE_NEXT_FIELD: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    compile_regex(
        r#"(?i)(?:,\s*["']?(?:mood|emotion|remember|action|custom_?js|coin_?request_?percent)["']?\s*:|\s*\}\s*$)"#,
    )
});
static RE_MOOD: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    compile_regex(r#"(?i)["']?(?:mood|emotion)["']?\s*:\s*["']?([a-zA-Z_-]+)["']?"#)
});
static RE_ACTION: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    compile_regex(r#"(?i)["']?(?:action)["']?\s*:\s*["']?([a-zA-Z_-]+)["']?"#)
});
static RE_REMEMBER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    compile_regex(r#"(?i)["']?(?:remember)["']?\s*:\s*["']([^"'\\]*(?:\\.[^"'\\]*)*)["']"#)
});
static RE_CUSTOM_JS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    compile_regex(r#"(?i)["']?(?:custom_?js)["']?\s*:\s*["']([^"'\\]*(?:\\.[^"'\\]*)*)["']"#)
});
static RE_COIN_PERCENT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    compile_regex(r#"(?i)["']?(?:coin_?request_?percent)["']?\s*:\s*["']?([0-9]+(?:\.[0-9]+)?)"#)
});

fn unescape_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some('\'') => out.push('\''),
                Some('\\') => out.push('\\'),
                Some('/') => out.push('/'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Repairs common LLM JSON syntax errors:
/// - Strips C/JS-style comments
/// - Escapes literal control chars (\n, \r, \t) inside string values
/// - Converts single quotes around strings/keys to double quotes
/// - Removes trailing commas before } or ]
/// - Inserts missing commas between lines of properties
/// - Quotes unquoted keys
/// - Converts Python True/False/None to true/false/null
/// - Closes unclosed quotes and braces
fn repair_json_string(candidate: &str) -> String {
    let mut out = String::with_capacity(candidate.len() + 32);
    let chars: Vec<char> = candidate.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_double_quote = false;
    let mut in_single_quote = false;
    let mut escape = false;

    while i < len {
        let c = chars[i];
        if in_double_quote {
            if escape {
                escape = false;
                out.push(c);
            } else if c == '\\' {
                escape = true;
                out.push(c);
            } else if c == '"' {
                in_double_quote = false;
                out.push(c);
            } else if c == '\n' {
                out.push_str("\\n");
            } else if c == '\r' {
                out.push_str("\\r");
            } else if c == '\t' {
                out.push_str("\\t");
            } else if (c as u32) < 0x20 {
                out.push(' ');
            } else {
                out.push(c);
            }
        } else if in_single_quote {
            if escape {
                escape = false;
                if c != '\'' {
                    out.push('\\');
                }
                out.push(c);
            } else if c == '\\' {
                escape = true;
            } else if c == '\'' {
                in_single_quote = false;
                out.push('"');
            } else if c == '"' {
                out.push_str("\\\"");
            } else if c == '\n' {
                out.push_str("\\n");
            } else if c == '\r' {
                out.push_str("\\r");
            } else if c == '\t' {
                out.push_str("\\t");
            } else if (c as u32) < 0x20 {
                out.push(' ');
            } else {
                out.push(c);
            }
        } else {
            // Outside strings: skip comments
            if c == '/' && i + 1 < len && chars[i + 1] == '/' {
                i += 2;
                while i < len && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            if c == '/' && i + 1 < len && chars[i + 1] == '*' {
                i += 2;
                while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
                continue;
            }
            if c == '"' {
                in_double_quote = true;
                out.push(c);
            } else if c == '\'' {
                in_single_quote = true;
                out.push('"');
            } else {
                out.push(c);
            }
        }
        i += 1;
    }

    if in_double_quote || in_single_quote {
        out.push('"');
    }

    let out = RE_TRAILING_COMMA.replace_all(&out, "$1");
    let out = RE_MISSING_COMMA.replace_all(&out, "$1,\n$2");
    let out = RE_UNQUOTED_KEYS.replace_all(&out, r#"$1"$2":"#);
    let out = RE_BOOL_TRUE.replace_all(&out, ": true");
    let out = RE_BOOL_FALSE.replace_all(&out, ": false");
    let mut out = RE_NONE.replace_all(&out, ": null").to_string();

    // Balance unclosed braces and brackets
    let mut open_braces: i32 = 0;
    let mut open_brackets: i32 = 0;
    let mut in_str = false;
    let mut esc = false;
    for ch in out.chars() {
        if in_str {
            if esc {
                esc = false;
            } else if ch == '\\' {
                esc = true;
            } else if ch == '"' {
                in_str = false;
            }
        } else if ch == '"' {
            in_str = true;
        } else if ch == '{' {
            open_braces += 1;
        } else if ch == '}' && open_braces > 0 {
            open_braces -= 1;
        } else if ch == '[' {
            open_brackets += 1;
        } else if ch == ']' && open_brackets > 0 {
            open_brackets -= 1;
        }
    }
    if in_str {
        out.push('"');
    }
    for _ in 0..open_brackets {
        out.push(']');
    }
    for _ in 0..open_braces {
        out.push('}');
    }

    out
}

fn resolve_mood(raw_mood: &str, reply: &str, moods: &[String]) -> String {
    let raw = raw_mood.trim();
    if let Some(valid) = moods.iter().find(|m| m.eq_ignore_ascii_case(raw)) {
        return valid.clone();
    }
    if !raw.is_empty() {
        tracing::error!(
            "[companion] deepseek: mood {raw:?} didn't match any of {moods:?}, guessing instead"
        );
    }
    guess_mood(reply, moods)
}

fn clean_custom_js(code: &str) -> String {
    let mut str = strip_markdown_code_fence(code).trim().to_string();
    if str.starts_with("function")
        && str
            .get(8..)
            .is_some_and(|s| s.trim_start().starts_with('('))
    {
        str = format!("({str})()");
    }
    let opens = str.chars().filter(|&c| c == '{').count();
    let mut closes = str.chars().filter(|&c| c == '}').count();
    while closes > opens && str.ends_with('}') {
        str.pop();
        str = str.trim_end().to_string();
        closes -= 1;
    }
    let open_parens = str.chars().filter(|&c| c == '(').count();
    let mut close_parens = str.chars().filter(|&c| c == ')').count();
    while open_parens > close_parens {
        str.push(')');
        close_parens += 1;
    }
    str
}

fn extract_value_to_parsed_reply(val: &Value, moods: &[String]) -> Option<ParsedReply> {
    let reply = val
        .get("reply")
        .or_else(|| val.get("message"))
        .or_else(|| val.get("text"))
        .or_else(|| val.get("response"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();

    if reply.is_empty() {
        return None;
    }

    let raw_mood = val
        .get("mood")
        .or_else(|| val.get("emotion"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let mood = resolve_mood(raw_mood, reply, moods);

    let remember = val
        .get("remember")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    let action = val
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    let custom_js = val
        .get("customJs")
        .or_else(|| val.get("custom_js"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let coin_request_percent = val
        .get("coinRequestPercent")
        .or_else(|| val.get("coin_request_percent"))
        .and_then(|v| {
            if let Some(n) = v.as_f64() {
                Some(n)
            } else if let Some(s) = v.as_str() {
                s.trim().trim_end_matches('%').parse::<f64>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0.0)
        .clamp(0.0, MAX_COIN_REQUEST_PERCENT);

    Some(ParsedReply {
        reply: reply.to_string(),
        mood,
        remember,
        action,
        custom_js: clean_custom_js(&custom_js),
        coin_request_percent,
    })
}

fn extract_fields_resilient(text: &str, moods: &[String]) -> Option<ParsedReply> {
    let reply_match = RE_REPLY_KEY.find(text)?;
    let after_reply = text[reply_match.end()..].trim_start();
    if after_reply.is_empty() {
        return None;
    }

    let (raw_reply, _) = if let Some(stripped) = after_reply.strip_prefix('"') {
        if let Some(m) = RE_NEXT_FIELD.find(stripped) {
            let inside = &stripped[..m.start()].trim_end();
            let inside = inside.strip_suffix('"').unwrap_or(inside);
            (inside, &stripped[m.start()..])
        } else if let Some(last_quote) = stripped.rfind('"') {
            (&stripped[..last_quote], &stripped[last_quote + 1..])
        } else {
            let inside = stripped.trim_end_matches(['}', ' ', '\n', '\r', '\t']);
            (inside, "")
        }
    } else if let Some(stripped) = after_reply.strip_prefix('\'') {
        if let Some(m) = RE_NEXT_FIELD.find(stripped) {
            let inside = &stripped[..m.start()].trim_end();
            let inside = inside.strip_suffix('\'').unwrap_or(inside);
            (inside, &stripped[m.start()..])
        } else if let Some(last_quote) = stripped.rfind('\'') {
            (&stripped[..last_quote], &stripped[last_quote + 1..])
        } else {
            let inside = stripped.trim_end_matches(['}', ' ', '\n', '\r', '\t']);
            (inside, "")
        }
    } else if let Some(m) = RE_NEXT_FIELD.find(after_reply) {
        let inside = &after_reply[..m.start()].trim_end();
        let inside = inside.trim_matches(['"', '\'']);
        (inside, &after_reply[m.start()..])
    } else {
        let inside = after_reply.trim_end_matches(['}', ' ', '\n', '\r', '\t']);
        let inside = inside.trim_matches(['"', '\'']);
        (inside, "")
    };

    let reply = unescape_json_string(raw_reply).trim().to_string();
    if reply.is_empty() {
        return None;
    }

    let raw_mood = RE_MOOD
        .captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str())
        .unwrap_or("");
    let mood = resolve_mood(raw_mood, &reply, moods);

    let action = RE_ACTION
        .captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str())
        .unwrap_or("")
        .to_string();

    let remember = RE_REMEMBER
        .captures(text)
        .and_then(|c| c.get(1))
        .map(|m| unescape_json_string(m.as_str()))
        .unwrap_or_default();

    let custom_js = RE_CUSTOM_JS
        .captures(text)
        .and_then(|c| c.get(1))
        .map(|m| unescape_json_string(m.as_str()))
        .unwrap_or_default();

    let coin_request_percent = RE_COIN_PERCENT
        .captures(text)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse::<f64>().ok())
        .unwrap_or(0.0)
        .clamp(0.0, MAX_COIN_REQUEST_PERCENT);

    Some(ParsedReply {
        reply,
        mood,
        remember,
        action,
        custom_js: clean_custom_js(&custom_js),
        coin_request_percent,
    })
}

fn clean_plain_text_reply(text: &str) -> String {
    let mut t = text.trim();
    if let Some(stripped) = t.strip_prefix("```") {
        if let Some(idx) = stripped.find('\n') {
            t = &stripped[idx + 1..];
        } else {
            t = stripped;
        }
        if let Some(end) = t.rfind("```") {
            t = &t[..end];
        }
        t = t.trim();
    }
    if t.starts_with('{') && t.ends_with('}') && t.len() >= 2 {
        t = t[1..t.len() - 1].trim();
    }
    if (t.starts_with('"') && t.ends_with('"') && t.len() >= 2)
        || (t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2)
    {
        t = t[1..t.len() - 1].trim();
    }
    unescape_json_string(t)
}

/// `raw` (whatever the model sent for `action`) if it case-insensitively
/// matches one of `ACTIONS`, else empty. This is the actual enforcement
/// that only the fixed preset list can ever reach the widget — the prompt
/// text asking for one of these names is a request, not a guarantee.
fn sanitize_action(raw: &str) -> String {
    ACTIONS
        .iter()
        .find(|a| a.eq_ignore_ascii_case(raw.trim()))
        .map(|a| a.to_string())
        .unwrap_or_default()
}

/// Simple keyword sniffing on the reply text itself, used whenever the
/// model didn't hand back a usable `mood` field (it drops the JSON
/// wrapper entirely more often as the conversation history grows — a real
/// DeepSeek quirk, not something this can force). Picking a mood this way
/// is obviously cruder than asking the model, but it's free (no second API
/// call) and, critically, isn't a fixed value: landing on the exact same
/// mood every single time a character has one literally named "default"
/// or alphabetically first is the bug this exists to avoid (that's what
/// "the bot never changes mood" turned out to be).
fn guess_mood(reply: &str, moods: &[String]) -> String {
    const KEYWORDS: &[(&str, &[&str])] = &[
        (
            "happy",
            &[
                "happy", "glad", "yay", "great", "awesome", "love", "excited", "fun",
            ],
        ),
        ("sad", &["sad", "sorry", "upset", "cry", "down", "hurt"]),
        ("angry", &["angry", "mad", "furious", "annoyed", "frustrat"]),
        ("pout", &["pout", "hmph", "fine.", "whatever", "embarrass"]),
        ("surprised", &["surprised", "wow", "whoa", "shock"]),
    ];
    let lower = reply.to_lowercase();
    for (category, hints) in KEYWORDS {
        if hints.iter().any(|h| lower.contains(h)) {
            if let Some(m) = moods.iter().find(|m| m.eq_ignore_ascii_case(category)) {
                return m.clone();
            }
        }
    }
    // No keyword hit (or no matching mood for the category that did) —
    // pick randomly rather than always the same one. Mildly deprioritize
    // anything that looks like a deliberate fallback/placeholder name so
    // a character with e.g. "idle"/"default" among its moods doesn't just
    // trade a static wrong answer for a *frequent* one.
    let candidates: Vec<&String> = moods
        .iter()
        .filter(|m| {
            let l = m.to_lowercase();
            !l.contains("default") && !l.contains("idle")
        })
        .collect();
    let pool = if candidates.is_empty() {
        moods.iter().collect::<Vec<_>>()
    } else {
        candidates
    };
    use rand::Rng;
    let idx = rand::rng().random_range(0..pool.len());
    pool[idx].clone()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn sample_moods() -> Vec<String> {
        vec![
            "happy".into(),
            "sad".into(),
            "angry".into(),
            "pout".into(),
            "surprised".into(),
        ]
    }

    #[test]
    fn test_valid_json() {
        let input = r#"{"reply": "Hello friend!", "mood": "happy", "action": "wave"}"#;
        let parsed = extract_reply(input, &sample_moods()).unwrap();
        assert_eq!(parsed.reply, "Hello friend!");
        assert_eq!(parsed.mood, "happy");
        assert_eq!(parsed.action, "wave");
    }

    #[test]
    fn test_markdown_code_fence() {
        let input = "```json\n{\"reply\": \"Hello from code block!\", \"mood\": \"happy\"}\n```";
        let parsed = extract_reply(input, &sample_moods()).unwrap();
        assert_eq!(parsed.reply, "Hello from code block!");
        assert_eq!(parsed.mood, "happy");
    }

    #[test]
    fn test_markdown_code_fence_with_prose() {
        let input = "Here is my response:\n```json\n{\"reply\": \"I am ready!\", \"mood\": \"happy\", \"action\": \"jump\"}\n```\nHope that helps!";
        let parsed = extract_reply(input, &sample_moods()).unwrap();
        assert_eq!(parsed.reply, "I am ready!");
        assert_eq!(parsed.mood, "happy");
        assert_eq!(parsed.action, "jump");
    }

    #[test]
    fn test_trailing_comma() {
        let input = r#"{"reply": "Watch out!", "mood": "angry", "coinRequestPercent": 50,}"#;
        let parsed = extract_reply(input, &sample_moods()).unwrap();
        assert_eq!(parsed.reply, "Watch out!");
        assert_eq!(parsed.mood, "angry");
        assert_eq!(parsed.coin_request_percent, 50.0);
    }

    #[test]
    fn test_literal_newlines_in_string() {
        let input = "{\n  \"reply\": \"Line one\nLine two\",\n  \"mood\": \"happy\"\n}";
        let parsed = extract_reply(input, &sample_moods()).unwrap();
        assert_eq!(parsed.reply, "Line one\nLine two");
        assert_eq!(parsed.mood, "happy");
    }

    #[test]
    fn test_single_quoted_json() {
        let input = "{'reply': 'Single quotes!', 'mood': 'happy', 'action': 'dance'}";
        let parsed = extract_reply(input, &sample_moods()).unwrap();
        assert_eq!(parsed.reply, "Single quotes!");
        assert_eq!(parsed.mood, "happy");
        assert_eq!(parsed.action, "dance");
    }

    #[test]
    fn test_unquoted_keys() {
        let input = "{ reply: \"Unquoted keys!\", mood: \"happy\", action: \"spin\" }";
        let parsed = extract_reply(input, &sample_moods()).unwrap();
        assert_eq!(parsed.reply, "Unquoted keys!");
        assert_eq!(parsed.mood, "happy");
        assert_eq!(parsed.action, "spin");
    }

    #[test]
    fn test_python_booleans_and_comments() {
        let input = "{\n  // A comment here\n  \"reply\": \"Python style!\",\n  \"mood\": \"happy\",\n  \"remember\": \"cool\",\n  \"is_bot\": False\n}";
        let parsed = extract_reply(input, &sample_moods()).unwrap();
        assert_eq!(parsed.reply, "Python style!");
        assert_eq!(parsed.mood, "happy");
        assert_eq!(parsed.remember, "cool");
    }

    #[test]
    fn test_missing_comma_between_properties() {
        let input = "{\n  \"reply\": \"Missing comma!\"\n  \"mood\": \"happy\"\n  \"coinRequestPercent\": 25\n}";
        let parsed = extract_reply(input, &sample_moods()).unwrap();
        assert_eq!(parsed.reply, "Missing comma!");
        assert_eq!(parsed.mood, "happy");
        assert_eq!(parsed.coin_request_percent, 25.0);
    }

    #[test]
    fn test_unescaped_quotes_in_reply() {
        let input = r#"{"reply": "He said "hello there!" to me.", "mood": "happy", "coinRequestPercent": 100}"#;
        let parsed = extract_reply(input, &sample_moods()).unwrap();
        assert_eq!(parsed.reply, "He said \"hello there!\" to me.");
        assert_eq!(parsed.mood, "happy");
        assert_eq!(parsed.coin_request_percent, 100.0);
    }

    #[test]
    fn test_truncated_json() {
        let input = r#"{"reply": "I am about to take all your coins!""#;
        let parsed = extract_reply(input, &sample_moods()).unwrap();
        assert_eq!(parsed.reply, "I am about to take all your coins!");
    }

    #[test]
    fn test_string_coin_request_percent() {
        let input = r#"{"reply": "Pay tax!", "mood": "angry", "coinRequestPercent": "75%"}"#;
        let parsed = extract_reply(input, &sample_moods()).unwrap();
        assert_eq!(parsed.reply, "Pay tax!");
        assert_eq!(parsed.mood, "angry");
        assert_eq!(parsed.coin_request_percent, 75.0);
    }

    #[test]
    fn test_snake_case_keys() {
        let input = r#"{"response": "Snake case!", "emotion": "angry", "custom_js": "alert(1)", "coin_request_percent": 30}"#;
        let parsed = extract_reply(input, &sample_moods()).unwrap();
        assert_eq!(parsed.reply, "Snake case!");
        assert_eq!(parsed.mood, "angry");
        assert_eq!(parsed.custom_js, "alert(1)");
        assert_eq!(parsed.coin_request_percent, 30.0);
    }

    #[test]
    fn test_plain_text_fallback_strips_stray_syntax() {
        let input = "```\nJust a regular message without json!\n```";
        let parsed = extract_reply(input, &sample_moods()).unwrap();
        assert_eq!(parsed.reply, "Just a regular message without json!");
    }

    #[test]
    fn test_empty_input_returns_none() {
        assert!(extract_reply("", &sample_moods()).is_none());
        assert!(extract_reply("   ", &sample_moods()).is_none());
        assert!(extract_reply("```json\n\n```", &sample_moods()).is_none());
    }
}
