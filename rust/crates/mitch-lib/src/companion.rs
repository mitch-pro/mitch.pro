//! Desktop companion characters — data layer for `companion_characters`,
//! `companion_character_images`, `companion_user_settings`,
//! `companion_chat_messages`, `companion_memories`,
//! `companion_subscriptions`, and `companion_character_unlocks`. New
//! subsystem, no `server.js` equivalent: real columns per AGENTS.md's
//! "Data storage" rule rather than a JSON blob.
//!
//! Phase 2 economy, in one place since it's all load-bearing for
//! `AGENTS.md`'s economy-integrity rule:
//! - Using the companion at all requires an active subscription
//!   (`companion_subscriptions`): 750 coins/week or 1500/month, strictly
//!   additive (buying more time always extends from `max(now, current
//!   expiry)` — never shrinks, never double-grants).
//! - Individual characters can cost extra on top of that: `price_coins`
//!   on `companion_characters`, unlocked once per (email, character) via
//!   `companion_character_unlocks` — pay once, keep forever, no
//!   re-charging if the price changes later. Free characters
//!   (`price_coins == 0`) need no unlock row. A character's own creator
//!   always has access to it without paying (see `has_unlocked`).
//! - Any user can create a custom character for a flat 500-coin fee
//!   (pure sink — nobody receives it) and set their own `price_coins` for
//!   others to unlock it. The creator gets a 25% cut of each unlock;
//!   the remaining 75% is also a sink, not credited to anyone — this
//!   caps how much coin-recycling two colluding accounts could pull off
//!   (every cycle is strictly loss-making, see `routes/companion/mod.rs`
//!   for the walk-through).
//! - Custom characters are NOT visible/usable by anyone but their own
//!   creator (and admins) while `needs_review` is set — admins get
//!   notified on creation and can pull up the existing preset-authoring
//!   tool to inspect the prompt, mark it reviewed (making it visible to
//!   everyone), or delete it. This gate is what makes it safe for
//!   reviewed characters to carry real capability (custom JS execution,
//!   coin requests — see `routes/companion/mod.rs` and `deepseek.rs`):
//!   nobody but the creator is exposed to an unreviewed prompt's behavior.
//! - Coin requests: a reviewed character may take up to 100% of the
//!   asker's *current* balance (`deepseek.rs::MAX_COIN_REQUEST_PERCENT`).
//!   No user authorization or opt-in toggle required. Rate-limited per
//!   (email, character) by `COIN_REQUEST_COOLDOWN_MILLIS`. The deduction is
//!   applied directly on the server during the chat turn. Pure sink: paying a
//!   creator out of this would turn "write a manipulative prompt" into a
//!   revenue strategy.

use crate::auth::normalize_email;
use crate::crypto::random_bytes_hex;
use crate::data::DataStore;
use crate::school::now_millis;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Character {
    pub id: String,
    pub name: String,
    pub personality_prompt: String,
    pub is_preset: bool,
    pub is_default: bool,
    pub needs_review: bool,
    pub creator_email: String,
    pub price_coins: f64,
    pub moods: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserSettings {
    pub enabled: bool,
    pub active_character_id: String,
    /// Opt-in, default off: whether this user wants characters to be able
    /// to propose taking a cut of their MitchCoin balance at all. Even
    /// when true, nothing is ever deducted without a separate explicit
    /// accept — see `routes/companion/economy.rs`'s coin-request accept
    /// endpoint and `COIN_REQUEST_COOLDOWN_MILLIS` below.
    pub allow_coin_requests: bool,
}

fn moods_for(store: &DataStore, character_id: &str) -> Vec<String> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(
        "SELECT mood FROM companion_character_images WHERE character_id = ?1 ORDER BY mood",
    ) else {
        return Vec::new();
    };
    let rows = stmt.query_map(rusqlite::params![character_id], |row| {
        row.get::<_, String>(0)
    });
    rows.map(|r| r.filter_map(|m| m.ok()).collect())
        .unwrap_or_default()
}

/// Every character (presets first, then user-made, newest first within
/// each group) — the public `/api/companion/characters` listing.
type CharacterRow = (String, String, String, i64, i64, i64, String, f64);

fn row_to_character(store: &DataStore, row: CharacterRow) -> Character {
    let (
        id,
        name,
        personality_prompt,
        is_preset,
        is_default,
        needs_review,
        creator_email,
        price_coins,
    ) = row;
    let moods = moods_for(store, &id);
    Character {
        id,
        name,
        personality_prompt,
        is_preset: is_preset != 0,
        is_default: is_default != 0,
        needs_review: needs_review != 0,
        creator_email,
        price_coins,
        moods,
    }
}

const CHARACTER_COLUMNS: &str =
    "id, name, personality_prompt, is_preset, is_default, needs_review, creator_email, price_coins";

fn map_character_row(row: &rusqlite::Row) -> rusqlite::Result<CharacterRow> {
    Ok((
        row.get::<_, String>(0)?,
        row.get::<_, String>(1)?,
        row.get::<_, String>(2)?,
        row.get::<_, i64>(3)?,
        row.get::<_, i64>(4)?,
        row.get::<_, i64>(5)?,
        row.get::<_, String>(6)?,
        row.get::<_, f64>(7)?,
    ))
}

pub fn list_characters(store: &DataStore) -> Vec<Character> {
    let rows: Vec<CharacterRow> = {
        let conn = store.conn();
        let Ok(mut stmt) = conn.prepare(&format!(
            "SELECT {CHARACTER_COLUMNS} FROM companion_characters ORDER BY is_preset DESC, created_at DESC"
        )) else {
            return Vec::new();
        };
        let Ok(mapped) = stmt.query_map([], map_character_row) else {
            return Vec::new();
        };
        mapped.filter_map(|r| r.ok()).collect()
    };
    rows.into_iter()
        .map(|row| row_to_character(store, row))
        .collect()
}

/// `list_characters`, filtered down to what `viewer_email` is actually
/// allowed to see: an unreviewed custom character (`needs_review`) is
/// invisible to everyone except the person who created it — that's the
/// actual enforcement of "models aren't released until admins review."
/// Admin presets (`creator_email` empty) are never gated this way; only
/// user-made characters ever carry `needs_review`.
pub fn list_visible_characters(store: &DataStore, viewer_email: &str) -> Vec<Character> {
    let norm = normalize_email(viewer_email);
    list_characters(store)
        .into_iter()
        .filter(|c| !c.needs_review || c.creator_email == norm)
        .collect()
}

/// Every character created by this email, newest first — for the "my
/// characters" self-service management list.
pub fn list_characters_by_creator(store: &DataStore, creator_email: &str) -> Vec<Character> {
    let norm = normalize_email(creator_email);
    let rows: Vec<CharacterRow> = {
        let conn = store.conn();
        let Ok(mut stmt) = conn.prepare(&format!(
            "SELECT {CHARACTER_COLUMNS} FROM companion_characters WHERE creator_email = ?1 ORDER BY created_at DESC"
        )) else {
            return Vec::new();
        };
        let Ok(mapped) = stmt.query_map(rusqlite::params![norm], map_character_row) else {
            return Vec::new();
        };
        mapped.filter_map(|r| r.ok()).collect()
    };
    rows.into_iter()
        .map(|row| row_to_character(store, row))
        .collect()
}

pub fn get_character(store: &DataStore, id: &str) -> Option<Character> {
    let conn = store.conn();
    let row: Option<CharacterRow> = conn
        .query_row(
            &format!("SELECT {CHARACTER_COLUMNS} FROM companion_characters WHERE id = ?1"),
            rusqlite::params![id],
            map_character_row,
        )
        .ok();
    drop(conn);
    row.map(|row| row_to_character(store, row))
}

/// Creates a preset character (admin tool). Returns the new id.
pub fn create_preset_character(
    store: &DataStore,
    name: &str,
    personality_prompt: &str,
    price_coins: f64,
) -> String {
    let id = random_bytes_hex(8);
    let now = now_millis();
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO companion_characters \
         (id, name, personality_prompt, is_preset, creator_email, price_coins, created_at, updated_at) \
         VALUES (?1, ?2, ?3, 1, '', ?4, ?5, ?5)",
        rusqlite::params![id, name, personality_prompt, price_coins, now],
    );
    id
}

/// Creates a user-made custom character. Returns the new id. Flagged
/// `needs_review` immediately — visible right away (no pre-publish gate,
/// by explicit choice), but surfaced to admins for after-the-fact review.
pub fn create_custom_character(
    store: &DataStore,
    creator_email: &str,
    name: &str,
    personality_prompt: &str,
    price_coins: f64,
) -> String {
    let norm = normalize_email(creator_email);
    let id = random_bytes_hex(8);
    let now = now_millis();
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO companion_characters \
         (id, name, personality_prompt, is_preset, is_default, needs_review, creator_email, price_coins, created_at, updated_at) \
         VALUES (?1, ?2, ?3, 0, 0, 1, ?4, ?5, ?6, ?6)",
        rusqlite::params![id, name, personality_prompt, norm, price_coins, now],
    );
    id
}

/// Updates a character's name/personality prompt/price. Doesn't touch
/// `needs_review` itself — callers decide (admin review vs. a creator
/// editing their own character, which should re-flag it; see
/// `flag_needs_review`).
pub fn update_character(
    store: &DataStore,
    id: &str,
    name: &str,
    personality_prompt: &str,
    price_coins: f64,
) -> bool {
    let now = now_millis();
    let conn = store.conn();
    conn.execute(
        "UPDATE companion_characters SET name = ?1, personality_prompt = ?2, price_coins = ?3, updated_at = ?4 WHERE id = ?5",
        rusqlite::params![name, personality_prompt, price_coins, now, id],
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

pub fn flag_needs_review(store: &DataStore, id: &str) {
    let now = now_millis();
    let conn = store.conn();
    let _ = conn.execute(
        "UPDATE companion_characters SET needs_review = 1, updated_at = ?1 WHERE id = ?2",
        rusqlite::params![now, id],
    );
}

pub fn mark_reviewed(store: &DataStore, id: &str) -> bool {
    let now = now_millis();
    let conn = store.conn();
    conn.execute(
        "UPDATE companion_characters SET needs_review = 0, updated_at = ?1 WHERE id = ?2",
        rusqlite::params![now, id],
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// Marks exactly one character as the default (pre-selected for anyone who
/// hasn't picked one yet) — clears the flag off every other character
/// first so there's never more than one.
pub fn set_default_character(store: &DataStore, id: &str) -> bool {
    let now = now_millis();
    let conn = store.conn();
    let _ = conn.execute(
        "UPDATE companion_characters SET is_default = 0, updated_at = ?1 WHERE is_default = 1",
        rusqlite::params![now],
    );
    conn.execute(
        "UPDATE companion_characters SET is_default = 1, updated_at = ?1 WHERE id = ?2",
        rusqlite::params![now, id],
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

pub fn delete_character(store: &DataStore, id: &str) -> bool {
    let conn = store.conn();
    conn.execute(
        "DELETE FROM companion_characters WHERE id = ?1",
        rusqlite::params![id],
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// Records (or replaces) the stored image filename for one mood on one
/// character. `file` is the on-disk filename under
/// `data/companion_images/<character_id>/`, not a path.
pub fn set_character_image(store: &DataStore, character_id: &str, mood: &str, file: &str) {
    let now = now_millis();
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO companion_character_images (character_id, mood, file, updated_at) \
         VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(character_id, mood) DO UPDATE SET file = excluded.file, updated_at = excluded.updated_at",
        rusqlite::params![character_id, mood, file, now],
    );
}

pub fn remove_character_image(store: &DataStore, character_id: &str, mood: &str) {
    let conn = store.conn();
    let _ = conn.execute(
        "DELETE FROM companion_character_images WHERE character_id = ?1 AND mood = ?2",
        rusqlite::params![character_id, mood],
    );
}

/// The on-disk filename for one character/mood pair, for serving the image.
pub fn character_image_file(store: &DataStore, character_id: &str, mood: &str) -> Option<String> {
    let conn = store.conn();
    conn.query_row(
        "SELECT file FROM companion_character_images WHERE character_id = ?1 AND mood = ?2",
        rusqlite::params![character_id, mood],
        |row| row.get::<_, String>(0),
    )
    .ok()
}

/// Defaults to disabled with no character picked — opt-in, matching the
/// "site-wide, only if they have companion on" requirement.
pub fn get_user_settings(store: &DataStore, email: &str) -> UserSettings {
    let norm = normalize_email(email);
    let conn = store.conn();
    conn.query_row(
        "SELECT enabled, active_character_id, allow_coin_requests FROM companion_user_settings WHERE email = ?1",
        rusqlite::params![norm],
        |row| {
            Ok(UserSettings {
                enabled: row.get::<_, i64>(0)? != 0,
                active_character_id: row.get::<_, String>(1)?,
                allow_coin_requests: row.get::<_, i64>(2)? != 0,
            })
        },
    )
    .unwrap_or(UserSettings {
        enabled: false,
        active_character_id: String::new(),
        allow_coin_requests: true,
    })
}

pub fn set_user_settings(store: &DataStore, email: &str, enabled: bool, active_character_id: &str) {
    let norm = normalize_email(email);
    let now = now_millis();
    // Preserves the existing allow_coin_requests value — this function is
    // the enable/character-picker save, not the coin-requests toggle (see
    // `set_allow_coin_requests`), so it must never silently reset it.
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO companion_user_settings (email, enabled, active_character_id, updated_at) \
         VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(email) DO UPDATE SET enabled = excluded.enabled, \
         active_character_id = excluded.active_character_id, updated_at = excluded.updated_at",
        rusqlite::params![norm, enabled as i64, active_character_id, now],
    );
}

pub fn set_allow_coin_requests(store: &DataStore, email: &str, allow: bool) {
    let norm = normalize_email(email);
    let now = now_millis();
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO companion_user_settings (email, allow_coin_requests, updated_at) \
         VALUES (?1, ?2, ?3) \
         ON CONFLICT(email) DO UPDATE SET allow_coin_requests = excluded.allow_coin_requests, updated_at = excluded.updated_at",
        rusqlite::params![norm, allow as i64, now],
    );
}

/// Minimum time between one character asking this person for coins again
/// — keeps "sparingly" (as instructed in the prompt) from being the only
/// thing standing between a user and a character that asks every single
/// turn. Prompt-following is not a security boundary; this is.
pub const COIN_REQUEST_COOLDOWN_MILLIS: i64 = 6 * 60 * 60 * 1000;

/// Whether enough time has passed since this (email, character) pair's
/// last coin request for another one to be offered. True if none has ever
/// been recorded.
pub fn coin_request_cooldown_elapsed(store: &DataStore, email: &str, character_id: &str) -> bool {
    let norm = normalize_email(email);
    let conn = store.conn();
    let last: i64 = conn
        .query_row(
            "SELECT asked_at FROM companion_coin_requests WHERE email = ?1 AND character_id = ?2",
            rusqlite::params![norm, character_id],
            |row| row.get(0),
        )
        .unwrap_or(0);
    now_millis() - last >= COIN_REQUEST_COOLDOWN_MILLIS
}

/// Records that a character just proposed taking `percent` of this
/// person's balance, starting the cooldown and remembering `percent` so
/// `consume_coin_request` can later verify an accept isn't asking for more
/// than what was actually offered — called when the proposal is first
/// surfaced to the user (not only on accept), so a declined or ignored
/// request still can't be immediately retried.
pub fn record_coin_request(store: &DataStore, email: &str, character_id: &str, percent: f64) {
    let norm = normalize_email(email);
    let now = now_millis();
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO companion_coin_requests (email, character_id, asked_at, percent) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(email, character_id) DO UPDATE SET asked_at = excluded.asked_at, percent = excluded.percent",
        rusqlite::params![norm, character_id, now, percent],
    );
}

/// Atomically reads and clears the pending offer for (email, character):
/// returns the percent that was actually offered (0 if there is none
/// pending, including an already-consumed one), and zeroes it out in the
/// same call so a second accept on the same offer can't double-charge.
/// The caller still re-checks this percent against the *current* balance
/// before touching coins — this only answers "was this actually offered,"
/// not "is it still affordable."
pub fn consume_coin_request(store: &DataStore, email: &str, character_id: &str) -> f64 {
    let norm = normalize_email(email);
    let conn = store.conn();
    let percent: f64 = conn
        .query_row(
            "SELECT percent FROM companion_coin_requests WHERE email = ?1 AND character_id = ?2",
            rusqlite::params![norm, character_id],
            |row| row.get(0),
        )
        .unwrap_or(0.0);
    if percent > 0.0 {
        let _ = conn.execute(
            "UPDATE companion_coin_requests SET percent = 0 WHERE email = ?1 AND character_id = ?2",
            rusqlite::params![norm, character_id],
        );
    }
    percent
}

/// One turn of chat history. `role` is `"user"` or `"model"`. `mood` is
/// empty for user turns; for model turns it's what that reply actually
/// used, so the provider module can replay history in the *same* JSON
/// shape it asks for live — see the module doc on why that matters.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    pub mood: String,
}

pub fn add_chat_message(
    store: &DataStore,
    email: &str,
    character_id: &str,
    role: &str,
    content: &str,
    mood: &str,
) {
    let norm = normalize_email(email);
    let now = now_millis();
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO companion_chat_messages (email, character_id, role, content, mood, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![norm, character_id, role, content, mood, now],
    );
}

/// The last `limit` messages (both roles) for this user/character, oldest
/// first — ready to hand straight to the chat provider's message history
/// as prior turns.
pub fn recent_chat_messages(
    store: &DataStore,
    email: &str,
    character_id: &str,
    limit: u32,
) -> Vec<ChatMessage> {
    let norm = normalize_email(email);
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(
        "SELECT role, content, mood FROM companion_chat_messages \
         WHERE email = ?1 AND character_id = ?2 ORDER BY created_at DESC, id DESC LIMIT ?3",
    ) else {
        return Vec::new();
    };
    let Ok(mapped) = stmt.query_map(rusqlite::params![norm, character_id, limit], |row| {
        Ok(ChatMessage {
            role: row.get::<_, String>(0)?,
            content: row.get::<_, String>(1)?,
            mood: row.get::<_, String>(2)?,
        })
    }) else {
        return Vec::new();
    };
    let mut rows: Vec<ChatMessage> = mapped.filter_map(|r| r.ok()).collect();
    rows.reverse();
    rows
}

/// Keeps at most this many memories per user/character — "some" memory,
/// not an unbounded log the prompt would eventually choke on.
const MAX_MEMORIES_PER_CHARACTER: i64 = 50;

/// Stores one freeform note the character chose to remember about this
/// person, trimming older notes beyond `MAX_MEMORIES_PER_CHARACTER` for the
/// same (email, character) pair.
pub fn add_memory(store: &DataStore, email: &str, character_id: &str, content: &str) {
    let norm = normalize_email(email);
    let now = now_millis();
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO companion_memories (email, character_id, content, created_at) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![norm, character_id, content, now],
    );
    let _ = conn.execute(
        "DELETE FROM companion_memories WHERE id IN ( \
           SELECT id FROM companion_memories WHERE email = ?1 AND character_id = ?2 \
           ORDER BY created_at DESC LIMIT -1 OFFSET ?3 \
         )",
        rusqlite::params![norm, character_id, MAX_MEMORIES_PER_CHARACTER],
    );
}

/// Every stored memory for this user/character, oldest first.
pub fn recent_memories(store: &DataStore, email: &str, character_id: &str) -> Vec<String> {
    let norm = normalize_email(email);
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(
        "SELECT content FROM companion_memories WHERE email = ?1 AND character_id = ?2 ORDER BY created_at ASC",
    ) else {
        return Vec::new();
    };
    let Ok(mapped) = stmt.query_map(rusqlite::params![norm, character_id], |row| {
        row.get::<_, String>(0)
    }) else {
        return Vec::new();
    };
    mapped.filter_map(|r| r.ok()).collect()
}

/// 0 when the user has never subscribed.
pub fn subscription_expires_at(store: &DataStore, email: &str) -> i64 {
    let norm = normalize_email(email);
    let conn = store.conn();
    conn.query_row(
        "SELECT expires_at FROM companion_subscriptions WHERE email = ?1",
        rusqlite::params![norm],
        |row| row.get::<_, i64>(0),
    )
    .unwrap_or(0)
}

pub fn is_subscription_active(store: &DataStore, email: &str) -> bool {
    subscription_expires_at(store, email) > now_millis()
}

/// Extends the subscription by `extra_millis` from `max(now, current
/// expiry)` — never from `now` alone, so buying more time while already
/// subscribed stacks on top instead of wasting the remainder; never from
/// a stale past expiry either, so there's no way to construct a cheaper
/// "renewal" that back-dates. Returns the new expiry.
pub fn extend_subscription(store: &DataStore, email: &str, extra_millis: i64) -> i64 {
    let norm = normalize_email(email);
    let now = now_millis();
    let conn = store.conn();
    let current = conn
        .query_row(
            "SELECT expires_at FROM companion_subscriptions WHERE email = ?1",
            rusqlite::params![norm],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0);
    let new_expiry = current.max(now) + extra_millis;
    let _ = conn.execute(
        "INSERT INTO companion_subscriptions (email, expires_at, updated_at) VALUES (?1, ?2, ?3) \
         ON CONFLICT(email) DO UPDATE SET expires_at = excluded.expires_at, updated_at = excluded.updated_at",
        rusqlite::params![norm, new_expiry, now],
    );
    new_expiry
}

/// True if `email` is allowed to even know this character exists —
/// everyone, once reviewed; only its own creator (or an admin, who should
/// be checking this separately) before that. Gates discovery/unlock, not
/// continued use by someone who already unlocked it before a re-edit
/// pulled it back under review — see the module doc.
pub fn visible_to(character: &Character, email: &str) -> bool {
    !character.needs_review || character.creator_email == normalize_email(email)
}

/// True if this character needs no separate purchase: it's free, the
/// caller made it themselves, or they've already unlocked it before.
pub fn has_unlocked(store: &DataStore, email: &str, character: &Character) -> bool {
    if character.price_coins <= 0.0 {
        return true;
    }
    let norm = normalize_email(email);
    if !character.creator_email.is_empty() && character.creator_email == norm {
        return true;
    }
    let conn = store.conn();
    conn.query_row(
        "SELECT 1 FROM companion_character_unlocks WHERE email = ?1 AND character_id = ?2",
        rusqlite::params![norm, character.id],
        |_| Ok(()),
    )
    .is_ok()
}

/// Records a one-time unlock. Idempotent: calling it again for an
/// already-unlocked pair is a harmless no-op (callers still gate the
/// actual coin charge on `has_unlocked` being false beforehand — this
/// just makes the storage side safe to call more than once regardless).
pub fn unlock_character(store: &DataStore, email: &str, character_id: &str) {
    let norm = normalize_email(email);
    let now = now_millis();
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO companion_character_unlocks (email, character_id, unlocked_at) VALUES (?1, ?2, ?3) \
         ON CONFLICT(email, character_id) DO NOTHING",
        rusqlite::params![norm, character_id, now],
    );
}
