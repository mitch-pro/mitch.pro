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

pub struct ChatReply {
    pub reply: String,
    pub mood: String,
    /// A freeform note the character chose to persist about this person,
    /// if any (empty when it chose not to remember anything this turn).
    pub remember: String,
}

pub struct UserContext {
    pub email: String,
    pub display_name: String,
    pub mitchcoin_balance: f64,
    pub premium: bool,
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
         {{\"reply\": string, \"mood\": string, \"remember\": string}}. \"reply\" is what \
         you say. \"mood\" MUST be exactly one of these, spelled exactly as shown, nothing \
         else: {moods_list}. \"remember\" is optional — only fill it in with a short note \
         (a fact, preference, or ongoing thing about this person worth recalling next \
         time) when something in THIS message is actually worth remembering; leave it as \
         an empty string otherwise. Don't repeat a memory you already have.",
        email = user.email,
        display_name = user.display_name,
        balance = user.mitchcoin_balance,
        premium = user.premium,
    );

    let mut messages: Vec<Value> = vec![json!({ "role": "system", "content": system_prompt })];
    messages.extend(history.iter().map(|m| {
        let role = if m.role == "model" {
            "assistant"
        } else {
            "user"
        };
        json!({ "role": role, "content": m.content })
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
        let Some((reply, mood, remember)) = extract_reply(text, moods) else {
            tracing::error!("[companion] deepseek {model}: no usable reply in content: {text:?}");
            continue;
        };
        return Ok(ChatReply {
            reply,
            mood,
            remember,
        });
    }
    Err("companion AI failed".to_string())
}

/// Picks apart whatever the model actually sent, most-strict-first:
/// 1. The whole trimmed text is the JSON object we asked for.
/// 2. The model wrapped the JSON in a little prose — same `{...}`
///    substring trick `routes/team.rs` uses for Groq.
/// 3. The model ignored the format and just replied in plain prose —
///    treat the whole trimmed text as the reply rather than discarding a
///    perfectly good in-character response over a formatting slip.
///
/// An invalid or missing `mood` in any of these falls back to a default
/// rather than discarding an otherwise-good reply — getting the sprite
/// wrong is a much smaller problem than throwing the reply away.
/// Returns `None` only when there's truly nothing to show (blank/
/// whitespace-only content) — the one case still worth retrying for.
fn extract_reply(text: &str, moods: &[String]) -> Option<(String, String, String)> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(parsed) = serde_json::from_str::<Value>(trimmed)
        .ok()
        .or_else(|| extract_json_object(trimmed))
    {
        let reply = parsed
            .get("reply")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if !reply.is_empty() {
            let raw_mood = parsed.get("mood").and_then(|v| v.as_str()).unwrap_or("");
            // Case/whitespace-insensitive match: the model reliably picks
            // one of the moods it was given, just not always byte-for-byte
            // (e.g. "Happy" instead of "happy") — the canonical spelling
            // from `moods` is what gets returned either way, since that's
            // what the image lookup and the DB row are keyed on.
            let mood = moods
                .iter()
                .find(|valid| valid.eq_ignore_ascii_case(raw_mood.trim()))
                .cloned()
                .unwrap_or_else(|| {
                    if !raw_mood.is_empty() {
                        tracing::error!(
                            "[companion] deepseek: mood {raw_mood:?} didn't match any of {moods:?}, guessing instead"
                        );
                    }
                    guess_mood(reply, moods)
                });
            let remember = parsed
                .get("remember")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            return Some((reply.to_string(), mood, remember));
        }
    }
    // No JSON object at all, or one with an empty "reply" — the model
    // ignored the format. Use the raw text itself rather than failing.
    let mood = guess_mood(trimmed, moods);
    Some((trimmed.to_string(), mood, String::new()))
}

/// The first `{` through its matching last `}`, parsed as JSON — `None`
/// if there's no `{` at all or that substring isn't valid JSON.
fn extract_json_object(text: &str) -> Option<Value> {
    let open = text.find('{')?;
    let close_rel = text[open..].rfind('}')?;
    serde_json::from_str(&text[open..=open + close_rel]).ok()
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
