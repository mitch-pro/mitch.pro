//! Music player data layer: each user's saved tracks (`music_favorites`) and
//! the shared lyrics cache (`music_lyrics_cache`). See `docs/music.md`.
//!
//! Real tables, no JSON blobs. Nothing here records what a user has listened
//! to: only tracks they chose to save are stored, per `AGENTS.md`'s privacy
//! rule on viewing habits. Both tables are keyed by a YouTube video id; the
//! favorites table is erased with the account (`account_deletion.rs`).

use crate::auth::normalize_email;
use crate::data::DataStore;
use crate::school::now_millis;
use serde::Serialize;

/// Saved tracks per user. Keeps one account from filling the table.
pub const MAX_FAVORITES: i64 = 500;
/// Found lyrics are re-fetched after this long (a provider may have fixed or
/// added timing since).
const HIT_TTL_MS: i64 = 30 * 24 * 3600 * 1000;
/// A search that found nothing is retried after this long.
const MISS_TTL_MS: i64 = 6 * 3600 * 1000;

#[derive(Debug, Clone, Serialize)]
pub struct Favorite {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub duration: i64,
}

/// A YouTube video id: 11 characters of `[A-Za-z0-9_-]`. Everything that
/// reaches yt-dlp or a URL is checked with this first.
pub fn valid_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub fn list_favorites(store: &DataStore, email: &str) -> Vec<Favorite> {
    let email = normalize_email(email);
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(
        "SELECT video_id, title, artist, duration_s FROM music_favorites \
         WHERE email = ?1 ORDER BY added_at DESC",
    ) else {
        return Vec::new();
    };
    let rows = stmt.query_map(rusqlite::params![email], |r| {
        Ok(Favorite {
            id: r.get(0)?,
            title: r.get(1)?,
            artist: r.get(2)?,
            duration: r.get(3)?,
        })
    });
    rows.map(|r| r.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// Saves a track. Err carries a message for the client.
pub fn add_favorite(store: &DataStore, email: &str, fav: &Favorite) -> Result<(), &'static str> {
    if !valid_video_id(&fav.id) {
        return Err("bad track id");
    }
    let email = normalize_email(email);
    let conn = store.conn();
    let already: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM music_favorites WHERE email = ?1 AND video_id = ?2",
            rusqlite::params![email, fav.id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if already == 0 {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM music_favorites WHERE email = ?1",
                rusqlite::params![email],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if count >= MAX_FAVORITES {
            return Err("library is full");
        }
    }
    let title: String = fav.title.chars().take(200).collect();
    let artist: String = fav.artist.chars().take(200).collect();
    conn.execute(
        "INSERT INTO music_favorites (email, video_id, title, artist, duration_s, added_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT(email, video_id) DO UPDATE SET title = ?3, artist = ?4, duration_s = ?5",
        rusqlite::params![
            email,
            fav.id,
            title,
            artist,
            fav.duration.clamp(0, 86_400),
            now_millis()
        ],
    )
    .map(|_| ())
    .map_err(|_| "could not save")
}

pub fn remove_favorite(store: &DataStore, email: &str, video_id: &str) {
    let email = normalize_email(email);
    let _ = store.conn().execute(
        "DELETE FROM music_favorites WHERE email = ?1 AND video_id = ?2",
        rusqlite::params![email, video_id],
    );
}

/// A cached lyrics lookup. `sync` is `word`, `line`, `none` (plain text) or
/// `miss` (nothing found; `body` empty).
#[derive(Debug, Clone)]
pub struct CachedLyrics {
    pub source: String,
    pub sync: String,
    pub estimated: bool,
    pub body: String,
}

pub fn get_cached_lyrics(store: &DataStore, video_id: &str) -> Option<CachedLyrics> {
    let conn = store.conn();
    let (row, fetched_at): (CachedLyrics, i64) = conn
        .query_row(
            "SELECT source, sync, estimated, body, fetched_at FROM music_lyrics_cache \
             WHERE video_id = ?1",
            rusqlite::params![video_id],
            |r| {
                Ok((
                    CachedLyrics {
                        source: r.get(0)?,
                        sync: r.get(1)?,
                        estimated: r.get::<_, i64>(2)? != 0,
                        body: r.get(3)?,
                    },
                    r.get(4)?,
                ))
            },
        )
        .ok()?;
    let ttl = if row.sync == "miss" {
        MISS_TTL_MS
    } else {
        HIT_TTL_MS
    };
    (now_millis() - fetched_at < ttl).then_some(row)
}

pub fn put_cached_lyrics(store: &DataStore, video_id: &str, row: &CachedLyrics) {
    let _ = store.conn().execute(
        "INSERT INTO music_lyrics_cache (video_id, source, sync, estimated, body, fetched_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT(video_id) DO UPDATE SET source = ?2, sync = ?3, estimated = ?4, \
         body = ?5, fetched_at = ?6",
        rusqlite::params![
            video_id,
            row.source,
            row.sync,
            i64::from(row.estimated),
            row.body,
            now_millis()
        ],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_ids_are_strictly_checked() {
        assert!(valid_video_id("fJ9rUzIMcZQ"));
        assert!(valid_video_id("a-_1234567B"));
        assert!(!valid_video_id("short"));
        assert!(!valid_video_id("fJ9rUzIMcZQ "));
        assert!(!valid_video_id("fJ9rUzIMc;Q"));
        assert!(!valid_video_id("../etc/pass"));
    }
}
