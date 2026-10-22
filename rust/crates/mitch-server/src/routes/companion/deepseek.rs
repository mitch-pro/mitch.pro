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
//! `enum` to constrain `mood` the way Gemini/Groq could. Only
//! `{"type": "json_object"}` (valid-JSON-but-unconstrained-shape) is
//! available, so the valid moods are spelled out in the prompt text
//! instead, and the existing post-parse validation (reject if the
//! returned mood isn't in `moods`, try the next model) is now the *only*
//! enforcement, not a backstop — don't remove it.

use mitch_lib::companion::ChatMessage;
use serde_json::{json, Value};

/// Verified live against `GET /models` before picking this: `deepseek-flash`
/// (DeepSeek-V4.1-Flash) is the cheap tier — $0.14/M input, $0.28/M output
/// as of writing, vs. `deepseek-v4-pro`'s $0.435+/M — and still a capable
/// general chat model, just the budget one. `deepseek-v4-pro` is the
/// fallback. Keep it on the cheap tier — that's the explicit, repeated
/// requirement across every provider this has gone through.
const MODELS: &[&str] = &["deepseek-flash", "deepseek-v4-pro"];

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
        "model": "", // set per attempt below
        "messages": messages,
        "effort": "low",
        "max_tokens": 400,
        "response_format": { "type": "json_object" }
    });

    for model in MODELS {
        let mut attempt = body.clone();
        attempt["model"] = json!(model);
        let resp = reqwest::Client::new()
            .post("https://api.deepseek.com/chat/completions")
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {api_key}"))
            .json(&attempt)
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await;
        let Ok(resp) = resp else {
            tracing::error!("[companion] deepseek request error ({model})");
            continue;
        };
        let status = resp.status().as_u16();
        if status == 429 || status == 503 {
            continue;
        }
        if !(200..300).contains(&status) {
            let body = resp.text().await.unwrap_or_default();
            tracing::error!("[companion] deepseek {model} returned {status}: {body}");
            continue;
        }
        let Ok(data) = resp.json::<Value>().await else {
            continue;
        };
        let text = data
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|t| t.as_str())
            .unwrap_or("");
        let Ok(parsed) = serde_json::from_str::<Value>(text) else {
            continue;
        };
        let reply = parsed
            .get("reply")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let mood = parsed
            .get("mood")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if reply.is_empty() || !moods.iter().any(|m| m == &mood) {
            continue;
        }
        let remember = parsed
            .get("remember")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        return Ok(ChatReply {
            reply,
            mood,
            remember,
        });
    }
    Err("companion AI failed".to_string())
}
