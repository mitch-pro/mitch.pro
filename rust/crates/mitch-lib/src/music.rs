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
    /// Whether the timing has been checked against the video's captions (and
    /// shifted if it was off). Lyrics fetched in the background aren't until
    /// the first real play.
    pub aligned: bool,
    pub body: String,
}

/// SQL that is true for a cache row whose song is in some playlist. Cache keys
/// start with the 11-character video id.
const PINNED: &str = "substr(cache_key, 1, 11) IN \
     (SELECT video_id FROM music_playlist_tracks WHERE video_id != '')";

/// Cached lyrics for a key, if still good. Lyrics found for a song that is in a
/// playlist never expire (they stay until the playlist is deleted, like its
/// stream link); other hits live `HIT_TTL_MS`, and a "nothing found" result of
/// any song is retried after `MISS_TTL_MS`.
pub fn get_cached_lyrics(store: &DataStore, cache_key: &str) -> Option<CachedLyrics> {
    let conn = store.conn();
    let (row, fetched_at, pinned): (CachedLyrics, i64, bool) = conn
        .query_row(
            &format!(
                "SELECT source, sync, estimated, body, fetched_at, approximate, ({PINNED}), aligned \
                 FROM music_lyrics_cache WHERE cache_key = ?1"
            ),
            rusqlite::params![cache_key],
            |r| {
                Ok((
                    CachedLyrics {
                        source: r.get(0)?,
                        sync: r.get(1)?,
                        estimated: r.get::<_, i64>(2)? != 0,
                        body: r.get(3)?,
                        approximate: r.get::<_, i64>(5)? != 0,
                        aligned: r.get::<_, i64>(7)? != 0,
                    },
                    r.get(4)?,
                    r.get::<_, i64>(6)? != 0,
                ))
            },
        )
        .ok()?;
    if row.sync == "miss" {
        return (now_millis() - fetched_at < MISS_TTL_MS).then_some(row);
    }
    (pinned || now_millis() - fetched_at < HIT_TTL_MS).then_some(row)
}

/// The key lyrics are cached under: the video id plus a hash of the title and
/// artist the lookup used. The lookup's title and artist come from the client,
/// so keying by video id alone would let one user store the wrong song's lyrics
/// for everyone. With this key a different title simply gets its own entry, and
/// the usual case (the title the server's own search produced) is shared.
pub fn lyrics_cache_key(video_id: &str, title: &str, artist: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(crate::music_lyrics::normalize(title).as_bytes());
    h.update(b"\x1f");
    h.update(crate::music_lyrics::normalize(artist).as_bytes());
    let digest = h.finalize();
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("{video_id}|{hex}")
}

/// Rows kept at most (the oldest go first) and the largest body cached. Rows
/// for songs in playlists are not counted against `MAX_LYRICS_ROWS` (the first
/// to go would otherwise be a playlist's), but everything together is still
/// bounded by `MAX_LYRICS_ROWS_TOTAL`.
pub const MAX_LYRICS_ROWS: i64 = 5000;
pub const MAX_LYRICS_ROWS_TOTAL: i64 = 30_000;
const MAX_LYRICS_BODY_BYTES: usize = 120 * 1024;

pub fn put_cached_lyrics(store: &DataStore, cache_key: &str, row: &CachedLyrics) {
    if row.body.len() > MAX_LYRICS_BODY_BYTES {
        return;
    }
    let conn = store.conn();
    let _ = conn.execute(
        "INSERT INTO music_lyrics_cache (cache_key, source, sync, estimated, body, fetched_at, approximate, aligned) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(cache_key) DO UPDATE SET source = ?2, sync = ?3, estimated = ?4, \
         body = ?5, fetched_at = ?6, approximate = ?7, aligned = ?8",
        rusqlite::params![
            cache_key,
            row.source,
            row.sync,
            i64::from(row.estimated),
            row.body,
            now_millis(),
            i64::from(row.approximate),
            i64::from(row.aligned)
        ],
    );
    // Cap the table: keep the newest MAX_LYRICS_ROWS rows that aren't for a
    // playlist song, then bound the whole table.
    let _ = conn.execute(
        &format!(
            "DELETE FROM music_lyrics_cache WHERE cache_key IN \
             (SELECT cache_key FROM music_lyrics_cache WHERE NOT ({PINNED}) \
              ORDER BY fetched_at DESC LIMIT -1 OFFSET ?1)"
        ),
        rusqlite::params![MAX_LYRICS_ROWS],
    );
    let _ = conn.execute(
        "DELETE FROM music_lyrics_cache WHERE cache_key IN \
         (SELECT cache_key FROM music_lyrics_cache ORDER BY fetched_at DESC LIMIT -1 OFFSET ?1)",
        rusqlite::params![MAX_LYRICS_ROWS_TOTAL],
    );
}

/// Removes cached lyrics past their time to live (the worker calls this).
/// Found lyrics of songs that are in a playlist are kept.
pub fn prune_lyrics_cache(store: &DataStore) -> usize {
    let now = now_millis();
    store
        .conn()
        .execute(
            &format!(
                "DELETE FROM music_lyrics_cache WHERE \
                 (sync = 'miss' AND fetched_at < ?1) OR (fetched_at < ?2 AND NOT ({PINNED}))"
            ),
            rusqlite::params![now - MISS_TTL_MS, now - HIT_TTL_MS],
        )
        .unwrap_or(0)
}

/// Songs that are in a playlist, as `(video id, title, artist, seconds)`: the
/// wording a player sends when it asks for their lyrics, so the cache key
/// computed from it is the one a play will look up. At most `limit` rows.
pub fn playlist_lyrics_candidates(
    store: &DataStore,
    limit: i64,
) -> Vec<(String, String, String, i64)> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(
        "SELECT video_id, title, artist, MAX(duration_s) FROM music_playlist_tracks \
         WHERE video_id != '' GROUP BY video_id, title, artist LIMIT ?1",
    ) else {
        return Vec::new();
    };
    let rows = stmt.query_map(rusqlite::params![limit], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
    });
    rows.map(|r| r.filter_map(Result::ok).collect())
        .unwrap_or_default()
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

/// One cached search result row.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchRow {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub duration: i64,
}

/// Most distinct queries kept; the oldest are dropped past this.
pub const MAX_SEARCH_QUERIES: i64 = 3000;
/// Queries not refreshed for this long are dropped (milliseconds).
pub const SEARCH_MAX_AGE_MS: i64 = 30 * 24 * 3600 * 1000;

/// The stored results for a (normalized) query and when they were fetched
/// (unix ms). Age is the caller's call: stale results are still returned so
/// they can be shown while a refresh runs.
pub fn get_cached_search(store: &DataStore, query: &str) -> Option<(i64, Vec<SearchRow>)> {
    let conn = store.conn();
    let fetched: i64 = conn
        .query_row(
            "SELECT fetched_at FROM music_search_queries WHERE query = ?1",
            rusqlite::params![query],
            |r| r.get(0),
        )
        .ok()?;
    let mut stmt = conn
        .prepare(
            "SELECT video_id, title, artist, duration FROM music_search_results \
             WHERE query = ?1 ORDER BY position",
        )
        .ok()?;
    let rows: Vec<SearchRow> = stmt
        .query_map(rusqlite::params![query], |r| {
            Ok(SearchRow {
                id: r.get(0)?,
                title: r.get(1)?,
                artist: r.get(2)?,
                duration: r.get(3)?,
            })
        })
        .ok()?
        .filter_map(Result::ok)
        .collect();
    Some((fetched, rows))
}

/// Replaces the stored results for a query (an empty result isn't stored, so a
/// blip never hides a real answer).
pub fn put_cached_search(store: &DataStore, query: &str, rows: &[SearchRow]) {
    if rows.is_empty() || query.is_empty() || query.len() > 400 {
        return;
    }
    let conn = store.conn();
    let _ = conn.execute_batch("BEGIN");
    let ok = (|| -> rusqlite::Result<()> {
        conn.execute(
            "DELETE FROM music_search_results WHERE query = ?1",
            rusqlite::params![query],
        )?;
        for (i, t) in rows.iter().enumerate() {
            if !valid_video_id(&t.id) {
                continue;
            }
            conn.execute(
                "INSERT INTO music_search_results (query, position, video_id, title, artist, duration) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![query, i as i64, t.id, t.title, t.artist, t.duration],
            )?;
        }
        conn.execute(
            "INSERT INTO music_search_queries (query, fetched_at) VALUES (?1, ?2) \
             ON CONFLICT(query) DO UPDATE SET fetched_at = ?2",
            rusqlite::params![query, now_millis()],
        )?;
        Ok(())
    })();
    let _ = conn.execute_batch(if ok.is_ok() { "COMMIT" } else { "ROLLBACK" });
}

/// Drops queries past their age limit, then the oldest beyond the count cap.
pub fn prune_search_cache(store: &DataStore) -> usize {
    let conn = store.conn();
    let _ = conn.execute(
        "DELETE FROM music_search_queries WHERE fetched_at < ?1",
        rusqlite::params![now_millis() - SEARCH_MAX_AGE_MS],
    );
    let _ = conn.execute(
        "DELETE FROM music_search_queries WHERE query IN (SELECT query FROM music_search_queries \
         ORDER BY fetched_at DESC LIMIT -1 OFFSET ?1)",
        rusqlite::params![MAX_SEARCH_QUERIES],
    );
    conn.execute(
        "DELETE FROM music_search_results WHERE query NOT IN (SELECT query FROM music_search_queries)",
        [],
    )
    .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_results_are_stored_replaced_and_pruned() {
        let dir = std::env::temp_dir().join(format!(
            "music-search-{}",
            crate::crypto::random_bytes_hex(6)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let s = DataStore::open(&dir, &dir).unwrap();
        let row = |id: &str, t: &str| SearchRow {
            id: id.into(),
            title: t.into(),
            artist: "A".into(),
            duration: 200,
        };
        assert!(get_cached_search(&s, "queen").is_none());
        put_cached_search(
            &s,
            "queen",
            &[row("aaaaaaaaaaa", "One"), row("bbbbbbbbbbb", "Two")],
        );
        let (at, rows) = get_cached_search(&s, "queen").unwrap();
        assert!(at > 0);
        assert_eq!(
            rows.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
            ["One", "Two"]
        );
        // A refresh replaces the list; an empty answer never wipes it.
        put_cached_search(&s, "queen", &[row("ccccccccccc", "Three")]);
        put_cached_search(&s, "queen", &[]);
        assert_eq!(get_cached_search(&s, "queen").unwrap().1.len(), 1);
        // Old queries are pruned along with their rows.
        s.conn()
            .execute("UPDATE music_search_queries SET fetched_at = 1", [])
            .unwrap();
        prune_search_cache(&s);
        assert!(get_cached_search(&s, "queen").is_none());
        let left: i64 = s
            .conn()
            .query_row("SELECT COUNT(*) FROM music_search_results", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(left, 0);
    }

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
    fn lyrics_cache_is_keyed_by_title_too_and_bounded() {
        let dir =
            std::env::temp_dir().join(format!("music-lyr-{}", crate::crypto::random_bytes_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        let s = DataStore::open(&dir, &dir).unwrap();
        // The same song under decorated titles shares an entry; a different title doesn't.
        let a = lyrics_cache_key("aaaaaaaaaaa", "Song (Remastered 2011)", "The Artist");
        let b = lyrics_cache_key("aaaaaaaaaaa", "song", "the artist");
        let other = lyrics_cache_key("aaaaaaaaaaa", "A Different Song", "The Artist");
        assert_eq!(a, b);
        assert_ne!(a, other, "a wrong title can't overwrite the real entry");
        let row = |body: &str| CachedLyrics {
            source: "x".into(),
            sync: "line".into(),
            estimated: false,
            approximate: false,
            aligned: true,
            body: body.into(),
        };
        put_cached_lyrics(&s, &a, &row("real"));
        put_cached_lyrics(&s, &other, &row("poison"));
        assert_eq!(get_cached_lyrics(&s, &a).unwrap().body, "real");
        // Oversized bodies are not cached.
        put_cached_lyrics(&s, "big", &row(&"x".repeat(MAX_LYRICS_BODY_BYTES + 1)));
        assert!(get_cached_lyrics(&s, "big").is_none());
        // The table keeps only the newest MAX_LYRICS_ROWS.
        {
            let conn = s.conn();
            let tx = conn.unchecked_transaction().unwrap();
            for i in 0..(MAX_LYRICS_ROWS + 50) {
                tx.execute("INSERT INTO music_lyrics_cache (cache_key, source, sync, body, fetched_at) VALUES (?1, 'x', 'line', '', ?2)", rusqlite::params![format!("k{i}"), i]).unwrap();
            }
            tx.commit().unwrap();
        }
        put_cached_lyrics(&s, "newest", &row("n"));
        let count: i64 = s
            .conn()
            .query_row("SELECT COUNT(*) FROM music_lyrics_cache", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, MAX_LYRICS_ROWS);
        assert!(get_cached_lyrics(&s, "newest").is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn playlist_songs_keep_their_lyrics_until_the_playlist_is_deleted() {
        let dir =
            std::env::temp_dir().join(format!("music-pin-{}", crate::crypto::random_bytes_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        let s = DataStore::open(&dir, &dir).unwrap();
        let now = now_millis();
        let row = |body: &str| CachedLyrics {
            source: "x".into(),
            sync: "line".into(),
            estimated: false,
            approximate: false,
            aligned: true,
            body: body.into(),
        };
        let miss = CachedLyrics {
            source: String::new(),
            sync: "miss".into(),
            estimated: false,
            approximate: false,
            aligned: true,
            body: String::new(),
        };
        let kept = lyrics_cache_key("kkkkkkkkkkk", "Held", "A");
        let other = lyrics_cache_key("ooooooooooo", "Loose", "A");
        let kept_miss = lyrics_cache_key("mmmmmmmmmmm", "Held Miss", "A");
        put_cached_lyrics(&s, &kept, &row("held"));
        put_cached_lyrics(&s, &other, &row("loose"));
        put_cached_lyrics(&s, &kept_miss, &miss);
        let pl = crate::music_playlists::create(&s, "o@x.test", "P", false, "").unwrap();
        let track = |id: &str| crate::music_playlists::PlaylistTrack {
            id: id.into(),
            title: "t".into(),
            artist: "a".into(),
            duration: 1,
            spotify_id: String::new(),
        };
        crate::music_playlists::add_tracks(
            &s,
            &pl,
            vec![track("kkkkkkkkkkk"), track("mmmmmmmmmmm")],
        )
        .unwrap();
        // Age everything past both time limits.
        s.conn()
            .execute(
                "UPDATE music_lyrics_cache SET fetched_at = ?1",
                [now - HIT_TTL_MS - 1000],
            )
            .unwrap();
        assert!(
            get_cached_lyrics(&s, &kept).is_some(),
            "found lyrics of a playlist song never expire"
        );
        assert!(
            get_cached_lyrics(&s, &other).is_none(),
            "an unheld hit expires after its time to live"
        );
        assert!(
            get_cached_lyrics(&s, &kept_miss).is_none(),
            "a miss is retried even for a playlist song"
        );
        prune_lyrics_cache(&s);
        let rows: i64 = s
            .conn()
            .query_row("SELECT COUNT(*) FROM music_lyrics_cache", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "pruning keeps only the playlist song's lyrics");
        // The playlist's songs are the candidates the worker fills in.
        assert_eq!(playlist_lyrics_candidates(&s, 10).len(), 2);
        // Deleting the playlist releases them.
        crate::music_playlists::delete(&s, &pl);
        assert!(
            get_cached_lyrics(&s, &kept).is_none(),
            "the lyrics go with the playlist"
        );
        assert!(playlist_lyrics_candidates(&s, 10).is_empty());
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
