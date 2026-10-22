//! The Groq API call behind `/api/companion/chat`. No `server.js`
//! equivalent — same OpenAI-compatible shape `routes/team.rs`'s support-email
//! assistant already uses (`GROQ_API_KEY`, `api.groq.com/openai/v1/chat/completions`),
//! but with its own model choice and a JSON-schema `response_format` for
//! structured output instead of team.rs's regex-scraped `{...}` extraction.
//!
//! Switched from Gemini to Groq on request — Gemini's free tier requires a
//! prepaid Google Cloud billing account past a certain point, which the
//! owner didn't want to pay into. Groq has a genuinely free tier with no
//! such prepay step.

use mitch_lib::companion::ChatMessage;
use serde_json::{json, Value};

/// Verified live against `GET /openai/v1/models` before picking this:
/// `openai/gpt-oss-20b` is the cheapest Groq model that's actually a
/// general-purpose chat model (not a prompt-guard classifier) and supports
/// `structured_outputs` — $0.075/M input, $0.3/M output tokens as of
/// writing. `openai/gpt-oss-120b` (same feature set, ~2x the price) is the
/// fallback. Do not swap in a bigger/pricier model here to "improve"
/// replies — "cheapest model" was the explicit requirement, same as it was
/// for the Gemini version this replaced.
const MODELS: &[&str] = &["openai/gpt-oss-20b", "openai/gpt-oss-120b"];

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
/// them before. `moods` must be non-empty; the model is constrained to
/// pick one of them via the JSON schema's `enum`.
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
         Respond with a JSON object: \"reply\" is what you say, \"mood\" is exactly one \
         of your available moods (whichever best matches your reaction), and \"remember\" \
         is optional — only fill it in with a short note (a fact, preference, or ongoing \
         thing about this person worth recalling next time) when something in THIS \
         message is actually worth remembering; leave it empty otherwise. Don't repeat a \
         memory you already have.",
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
        "reasoning_effort": "low",
        "max_tokens": 400,
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "companion_reply",
                "schema": {
                    "type": "object",
                    "properties": {
                        "reply": { "type": "string" },
                        "mood": { "type": "string", "enum": moods },
                        "remember": { "type": "string" }
                    },
                    "required": ["reply", "mood", "remember"],
                    "additionalProperties": false
                }
            }
        }
    });

    for model in MODELS {
        let mut attempt = body.clone();
        attempt["model"] = json!(model);
        // User-Agent spoof matches `team.rs`'s existing Groq caller — Groq
        // (or something in front of it) 403s reqwest's default UA string.
        let resp = reqwest::Client::new()
            .post("https://api.groq.com/openai/v1/chat/completions")
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {api_key}"))
            .header("User-Agent", "python-requests/2.31.0")
            .json(&attempt)
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await;
        let Ok(resp) = resp else {
            tracing::error!("[companion] groq request error ({model})");
            continue;
        };
        let status = resp.status().as_u16();
        if status == 429 || status == 503 {
            continue;
        }
        if !(200..300).contains(&status) {
            let body = resp.text().await.unwrap_or_default();
            tracing::error!("[companion] groq {model} returned {status}: {body}");
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
