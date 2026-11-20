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
    /// Matched by relaxed length (the full song for a TV-size upload): the
    /// timing belongs to a longer recording.
    pub approximate: bool,
    pub body: String,
}

pub fn get_cached_lyrics(store: &DataStore, video_id: &str) -> Option<CachedLyrics> {
    let conn = store.conn();
    let (row, fetched_at): (CachedLyrics, i64) = conn
        .query_row(
            "SELECT source, sync, estimated, body, fetched_at, approximate FROM music_lyrics_cache \
             WHERE video_id = ?1",
            rusqlite::params![video_id],
            |r| {
                Ok((
                    CachedLyrics {
                        source: r.get(0)?,
                        sync: r.get(1)?,
                        estimated: r.get::<_, i64>(2)? != 0,
                        body: r.get(3)?,
                        approximate: r.get::<_, i64>(5)? != 0,
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
        "INSERT INTO music_lyrics_cache (video_id, source, sync, estimated, body, fetched_at, approximate) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(video_id) DO UPDATE SET source = ?2, sync = ?3, estimated = ?4, \
         body = ?5, fetched_at = ?6, approximate = ?7",
        rusqlite::params![
            video_id,
            row.source,
            row.sync,
            i64::from(row.estimated),
            row.body,
            now_millis(),
            i64::from(row.approximate)
        ],
    );
}

// ------------------------------------------------------- stream links ----

/// A resolved audio stream URL kept in the database so playback starts without
/// waiting for yt-dlp, and survives a restart. Google's URLs last about six
/// hours and are tied to the IP that asked, so "kept" means kept fresh:
/// links for songs that are in a playlist stay in the table and the background
/// worker re-resolves them before they expire; they are removed when no
/// playlist holds the song any more.
#[derive(Debug, Clone)]
pub struct StreamLink {
    pub url: String,
    pub mime: String,
    /// Unix milliseconds.
    pub expires_at: i64,
}

/// A link with at least `min_left_ms` of life left.
pub fn get_stream_link(store: &DataStore, video_id: &str, min_left_ms: i64) -> Option<StreamLink> {
    let link: StreamLink = store
        .conn()
        .query_row(
            "SELECT url, mime, expires_at FROM music_stream_links WHERE video_id = ?1",
            rusqlite::params![video_id],
            |r| {
                Ok(StreamLink {
                    url: r.get(0)?,
                    mime: r.get(1)?,
                    expires_at: r.get(2)?,
                })
            },
        )
        .ok()?;
    (link.expires_at - now_millis() >= min_left_ms).then_some(link)
}

pub fn put_stream_link(store: &DataStore, video_id: &str, link: &StreamLink) {
    if !valid_video_id(video_id) {
        return;
    }
    let _ = store.conn().execute(
        "INSERT INTO music_stream_links (video_id, url, mime, expires_at) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(video_id) DO UPDATE SET url = ?2, mime = ?3, expires_at = ?4",
        rusqlite::params![video_id, link.url, link.mime, link.expires_at],
    );
}

pub fn delete_stream_link(store: &DataStore, video_id: &str) {
    let _ = store.conn().execute(
        "DELETE FROM music_stream_links WHERE video_id = ?1",
        rusqlite::params![video_id],
    );
}

/// Videos that are in some playlist and have no link, or one that expires
/// before `refresh_before_ms` (unix ms), soonest-to-lapse first.
pub fn playlist_videos_needing_link(
    store: &DataStore,
    refresh_before_ms: i64,
    limit: i64,
) -> Vec<String> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(
        "SELECT t.video_id FROM music_playlist_tracks t \
         LEFT JOIN music_stream_links l ON l.video_id = t.video_id \
         WHERE t.video_id != '' AND (l.video_id IS NULL OR l.expires_at < ?1) \
         GROUP BY t.video_id ORDER BY MIN(COALESCE(l.expires_at, 0)) LIMIT ?2",
    ) else {
        return Vec::new();
    };
    let rows = stmt.query_map(rusqlite::params![refresh_before_ms, limit], |r| r.get(0));
    rows.map(|r| r.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// Removes links nothing needs: not in any playlist and already expired.
pub fn prune_stream_links(store: &DataStore) -> usize {
    store
        .conn()
        .execute(
            "DELETE FROM music_stream_links WHERE expires_at < ?1 AND video_id NOT IN \
             (SELECT video_id FROM music_playlist_tracks WHERE video_id != '')",
            rusqlite::params![now_millis()],
        )
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_links_expire_and_are_kept_for_playlist_songs() {
        let dir = std::env::temp_dir().join(format!(
            "music-links-{}",
            crate::crypto::random_bytes_hex(6)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let s = DataStore::open(&dir, &dir).unwrap();
        let now = now_millis();
        let link = |left: i64| StreamLink {
            url: "https://a.googlevideo.com/x".into(),
            mime: "audio/mp4".into(),
            expires_at: now + left,
        };
        put_stream_link(&s, "aaaaaaaaaaa", &link(3_600_000));
        put_stream_link(&s, "bbbbbbbbbbb", &link(-1000));
        put_stream_link(&s, "ccccccccccc", &link(-1000));
        put_stream_link(&s, "bad", &link(3_600_000));
        assert!(get_stream_link(&s, "aaaaaaaaaaa", 600_000).is_some());
        assert!(
            get_stream_link(&s, "aaaaaaaaaaa", 7_200_000).is_none(),
            "too little life left"
        );
        assert!(get_stream_link(&s, "bbbbbbbbbbb", 0).is_none(), "expired");
        assert!(
            get_stream_link(&s, "bad", 0).is_none(),
            "invalid ids are never stored"
        );
        // Only "b" is in a playlist.
        let id = crate::music_playlists::create(&s, "o@x.test", "P", false, "").unwrap();
        crate::music_playlists::add_tracks(
            &s,
            &id,
            vec![crate::music_playlists::PlaylistTrack {
                id: "bbbbbbbbbbb".into(),
                title: "t".into(),
                artist: "a".into(),
                duration: 1,
                spotify_id: String::new(),
            }],
        )
        .unwrap();
        assert_eq!(
            prune_stream_links(&s),
            1,
            "the expired link nobody needs goes"
        );
        assert!(
            s.conn()
                .query_row(
                    "SELECT COUNT(*) FROM music_stream_links WHERE video_id='bbbbbbbbbbb'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap()
                == 1,
            "expired playlist link stays to be refreshed"
        );
        assert_eq!(
            playlist_videos_needing_link(&s, now + 1_800_000, 10),
            vec!["bbbbbbbbbbb".to_string()]
        );
        put_stream_link(&s, "bbbbbbbbbbb", &link(3_600_000));
        assert!(playlist_videos_needing_link(&s, now + 1_800_000, 10).is_empty());
        crate::music_playlists::delete(&s, &id);
        assert!(
            get_stream_link(&s, "bbbbbbbbbbb", 0).is_none(),
            "deleting the playlist drops its links"
        );
        assert!(
            get_stream_link(&s, "aaaaaaaaaaa", 0).is_some(),
            "other cached links are untouched"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

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
