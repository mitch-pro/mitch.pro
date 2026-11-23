//! Music playlists: `music_playlists`, `music_playlist_tracks` and the shared
//! Spotify-track match cache `music_track_matches`. See `docs/music.md`.
//!
//! Access model, enforced by [`can_view`] and the owner checks in the routes:
//! - **public**: any signed-in user can open and copy it, and it is listed in
//!   the Public tab.
//! - **private**: only the owner, unless the owner has made a share link. The
//!   link carries a random token (`share_token`); anyone signed in who has the
//!   link can open and copy it. Clearing the token revokes every old link.
//!
//! Only the owner ever edits, deletes or sees the token.
//!
//! A track is a YouTube video id plus display text. Tracks imported from
//! Spotify have no video id (`id` empty) and a `spotify_id`; the player finds a
//! YouTube match the first time one is played and remembers it in
//! `music_track_matches`, so every user benefits from one lookup.

use crate::auth::normalize_email;
use crate::crypto::random_bytes_hex;
use crate::data::DataStore;
use crate::music::valid_video_id;
use crate::school::now_millis;

pub const MAX_PLAYLISTS: i64 = 100;
pub const MAX_TRACKS: usize = 500;
pub const MAX_NAME_CHARS: usize = 80;

#[derive(Debug, Clone)]
pub struct Playlist {
    pub id: String,
    pub owner_email: String,
    pub name: String,
    pub public: bool,
    pub share_token: String,
    pub spotify_id: String,
    pub spotify_synced_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PlaylistTrack {
    /// YouTube video id, or empty for a Spotify track not matched yet.
    pub id: String,
    pub title: String,
    pub artist: String,
    pub duration: i64,
    #[serde(rename = "spotifyId")]
    pub spotify_id: String,
}

impl PlaylistTrack {
    /// Identity used to skip duplicates when adding.
    fn key(&self) -> String {
        if self.id.is_empty() {
            format!("s:{}", self.spotify_id)
        } else {
            format!("y:{}", self.id)
        }
    }

    /// A track is storable when it has a valid video id, or a Spotify id.
    fn valid(&self) -> bool {
        (valid_video_id(&self.id) && self.spotify_id.len() <= 40)
            || (self.id.is_empty() && !self.spotify_id.is_empty() && self.spotify_id.len() <= 40)
    }

    fn clipped(mut self) -> Self {
        self.title = self.title.chars().take(200).collect();
        self.artist = self.artist.chars().take(200).collect();
        self.duration = self.duration.clamp(0, 86_400);
        self
    }
}

/// Trims and bounds a playlist name. `None` if nothing is left.
pub fn clean_name(name: &str) -> Option<String> {
    let n: String = name
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_NAME_CHARS)
        .collect();
    (!n.is_empty()).then_some(n)
}

const COLS: &str =
    "id, owner_email, name, public, share_token, spotify_id, spotify_synced_at, updated_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Playlist> {
    Ok(Playlist {
        id: r.get(0)?,
        owner_email: r.get(1)?,
        name: r.get(2)?,
        public: r.get::<_, i64>(3)? != 0,
        share_token: r.get(4)?,
        spotify_id: r.get(5)?,
        spotify_synced_at: r.get(6)?,
        updated_at: r.get(7)?,
    })
}

pub fn create(
    store: &DataStore,
    email: &str,
    name: &str,
    public: bool,
    spotify_id: &str,
) -> Result<String, &'static str> {
    let Some(name) = clean_name(name) else {
        return Err("give the playlist a name");
    };
    let email = normalize_email(email);
    let conn = store.conn();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM music_playlists WHERE owner_email = ?1",
            rusqlite::params![email],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if count >= MAX_PLAYLISTS {
        return Err("too many playlists");
    }
    let id = random_bytes_hex(6);
    let now = now_millis();
    conn.execute(
        "INSERT INTO music_playlists \
         (id, owner_email, name, public, share_token, spotify_id, spotify_synced_at, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, '', ?5, ?6, ?6, ?6)",
        rusqlite::params![
            id,
            email,
            name,
            i64::from(public),
            spotify_id,
            if spotify_id.is_empty() { 0 } else { now }
        ],
    )
    .map_err(|_| "could not create the playlist")?;
    Ok(id)
}

pub fn get(store: &DataStore, id: &str) -> Option<Playlist> {
    store
        .conn()
        .query_row(
            &format!("SELECT {COLS} FROM music_playlists WHERE id = ?1"),
            rusqlite::params![id],
            row,
        )
        .ok()
}

/// A playlist with its track count, for listings.
pub type Listed = (Playlist, i64);

fn list_where(store: &DataStore, clause: &str, params: &[&dyn rusqlite::ToSql]) -> Vec<Listed> {
    let conn = store.conn();
    let sql = format!(
        "SELECT {COLS}, (SELECT COUNT(*) FROM music_playlist_tracks t WHERE t.playlist_id = p.id) \
         FROM music_playlists p WHERE {clause}"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let rows = stmt.query_map(params, |r| Ok((row(r)?, r.get::<_, i64>(8)?)));
    rows.map(|r| r.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

pub fn list_mine(store: &DataStore, email: &str) -> Vec<Listed> {
    let email = normalize_email(email);
    list_where(
        store,
        "owner_email = ?1 ORDER BY updated_at DESC",
        &[&email],
    )
}

/// Public playlists, newest first. `q` filters by name (case-insensitive).
pub fn list_public(store: &DataStore, q: &str, limit: i64) -> Vec<Listed> {
    let like = format!(
        "%{}%",
        q.trim()
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    list_where(
        store,
        "public = 1 AND name LIKE ?1 ESCAPE '\\' ORDER BY updated_at DESC LIMIT ?2",
        &[&like, &limit.clamp(1, 100)],
    )
    .into_iter()
    .filter(|(_, n)| *n > 0)
    .collect()
}

/// Video ids of a playlist's first `n` matched songs, for its cover mosaic.
pub fn cover_ids(store: &DataStore, id: &str, n: i64) -> Vec<String> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(
        "SELECT video_id FROM music_playlist_tracks WHERE playlist_id = ?1 AND video_id != '' \
         ORDER BY position LIMIT ?2",
    ) else {
        return Vec::new();
    };
    let rows = stmt.query_map(rusqlite::params![id, n], |r| r.get(0));
    rows.map(|r| r.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

pub fn tracks(store: &DataStore, id: &str) -> Vec<PlaylistTrack> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(
        "SELECT video_id, title, artist, duration_s, spotify_id FROM music_playlist_tracks \
         WHERE playlist_id = ?1 ORDER BY position",
    ) else {
        return Vec::new();
    };
    let rows = stmt.query_map(rusqlite::params![id], |r| {
        Ok(PlaylistTrack {
            id: r.get(0)?,
            title: r.get(1)?,
            artist: r.get(2)?,
            duration: r.get(3)?,
            spotify_id: r.get(4)?,
        })
    });
    rows.map(|r| r.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

pub fn is_owner(pl: &Playlist, email: &str) -> bool {
    pl.owner_email == normalize_email(email)
}

/// Whether `viewer` may open the playlist: the owner, anyone for a public
/// playlist, or anyone holding the current share token.
pub fn can_view(pl: &Playlist, viewer: &str, token: &str) -> bool {
    is_owner(pl, viewer)
        || pl.public
        || (!pl.share_token.is_empty() && same_secret(&pl.share_token, token))
}

pub fn update(
    store: &DataStore,
    id: &str,
    name: Option<&str>,
    public: Option<bool>,
) -> Result<(), &'static str> {
    let conn = store.conn();
    let now = now_millis();
    if let Some(n) = name {
        let Some(n) = clean_name(n) else {
            return Err("give the playlist a name");
        };
        let _ = conn.execute(
            "UPDATE music_playlists SET name = ?2, updated_at = ?3 WHERE id = ?1",
            rusqlite::params![id, n, now],
        );
    }
    if let Some(p) = public {
        let _ = conn.execute(
            "UPDATE music_playlists SET public = ?2, updated_at = ?3 WHERE id = ?1",
            rusqlite::params![id, i64::from(p), now],
        );
    }
    Ok(())
}

pub fn delete(store: &DataStore, id: &str) {
    let ids: Vec<String> = tracks(store, id)
        .into_iter()
        .map(|t| t.id)
        .filter(|v| !v.is_empty())
        .collect();
    let conn = store.conn();
    let _ = conn.execute(
        "DELETE FROM music_playlist_tracks WHERE playlist_id = ?1",
        rusqlite::params![id],
    );
    let _ = conn.execute(
        "DELETE FROM music_playlists WHERE id = ?1",
        rusqlite::params![id],
    );
    // The stored stream links and lyrics of songs no other playlist holds go
    // with it.
    for v in ids {
        let _ = conn.execute(
            "DELETE FROM music_stream_links WHERE video_id = ?1 AND video_id NOT IN \
             (SELECT video_id FROM music_playlist_tracks WHERE video_id != '')",
            rusqlite::params![v],
        );
        let _ = conn.execute(
            "DELETE FROM music_lyrics_cache WHERE substr(cache_key, 1, 11) = ?1 AND ?1 NOT IN \
             (SELECT video_id FROM music_playlist_tracks WHERE video_id != '')",
            rusqlite::params![v],
        );
    }
}

/// Spotify tracks in playlists that have no YouTube video yet:
/// (spotify id, title, artist, duration seconds), one per Spotify track.
pub fn unmatched_spotify_tracks(
    store: &DataStore,
    limit: i64,
) -> Vec<(String, String, String, i64)> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(
        "SELECT spotify_id, title, artist, MAX(duration_s) FROM music_playlist_tracks \
         WHERE video_id = '' AND spotify_id != '' GROUP BY spotify_id LIMIT ?1",
    ) else {
        return Vec::new();
    };
    let rows = stmt.query_map(rusqlite::params![limit], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
    });
    rows.map(|r| r.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// Records the YouTube video found for a Spotify track in every playlist row
/// that holds it, so those songs play without a lookup.
pub fn set_spotify_match(store: &DataStore, spotify_id: &str, video_id: &str) {
    if !valid_video_id(video_id) {
        return;
    }
    let _ = store.conn().execute(
        "UPDATE music_playlist_tracks SET video_id = ?2 WHERE spotify_id = ?1 AND video_id = ''",
        rusqlite::params![spotify_id, video_id],
    );
}

/// Creates (`true`) or revokes (`false`) the share link token. Returns the
/// new token when one was created.
pub fn set_share(store: &DataStore, id: &str, enable: bool) -> String {
    let token = if enable {
        random_bytes_hex(12)
    } else {
        String::new()
    };
    let _ = store.conn().execute(
        "UPDATE music_playlists SET share_token = ?2 WHERE id = ?1",
        rusqlite::params![id, token],
    );
    token
}

/// Replaces all tracks in one transaction, renumbering positions.
fn rewrite(store: &DataStore, id: &str, tracks: &[PlaylistTrack]) -> Result<(), &'static str> {
    let conn = store.conn();
    let tx = conn.unchecked_transaction().map_err(|_| "could not save")?;
    tx.execute(
        "DELETE FROM music_playlist_tracks WHERE playlist_id = ?1",
        rusqlite::params![id],
    )
    .map_err(|_| "could not save")?;
    for (i, t) in tracks.iter().enumerate() {
        tx.execute(
            "INSERT INTO music_playlist_tracks \
             (playlist_id, position, video_id, title, artist, duration_s, spotify_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                id,
                i as i64,
                t.id,
                t.title,
                t.artist,
                t.duration,
                t.spotify_id
            ],
        )
        .map_err(|_| "could not save")?;
    }
    tx.execute(
        "UPDATE music_playlists SET updated_at = ?2 WHERE id = ?1",
        rusqlite::params![id, now_millis()],
    )
    .map_err(|_| "could not save")?;
    tx.commit().map_err(|_| "could not save")
}

/// Appends tracks, skipping invalid ones and ones already in the playlist.
/// Returns how many were added.
pub fn add_tracks(
    store: &DataStore,
    id: &str,
    new: Vec<PlaylistTrack>,
) -> Result<usize, &'static str> {
    let mut all = tracks(store, id);
    let mut seen: std::collections::HashSet<String> = all.iter().map(PlaylistTrack::key).collect();
    let before = all.len();
    for t in new {
        let t = t.clipped();
        if !t.valid() || !seen.insert(t.key()) {
            continue;
        }
        if all.len() >= MAX_TRACKS {
            return if all.len() > before {
                rewrite(store, id, &all).map(|_| all.len() - before)
            } else {
                Err("playlist is full")
            };
        }
        all.push(t);
    }
    let added = all.len() - before;
    if added > 0 {
        rewrite(store, id, &all)?;
    }
    Ok(added)
}

pub fn remove_track(store: &DataStore, id: &str, position: usize) -> Result<(), &'static str> {
    let mut all = tracks(store, id);
    if position >= all.len() {
        return Err("no such track");
    }
    all.remove(position);
    rewrite(store, id, &all)
}

pub fn move_track(store: &DataStore, id: &str, from: usize, to: usize) -> Result<(), &'static str> {
    let mut all = tracks(store, id);
    if from >= all.len() || to >= all.len() {
        return Err("no such track");
    }
    let t = all.remove(from);
    all.insert(to, t);
    rewrite(store, id, &all)
}

/// Makes the Spotify-origin part of the playlist equal to `from_spotify`
/// (in Spotify's order), then keeps tracks the owner added by hand after it.
/// Returns (added, removed) counts of Spotify tracks.
pub fn sync_spotify(
    store: &DataStore,
    id: &str,
    from_spotify: Vec<PlaylistTrack>,
) -> Result<(usize, usize), &'static str> {
    let current = tracks(store, id);
    let old: std::collections::HashSet<&str> = current
        .iter()
        .filter(|t| !t.spotify_id.is_empty())
        .map(|t| t.spotify_id.as_str())
        .collect();
    let mut next: Vec<PlaylistTrack> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for t in from_spotify {
        let t = t.clipped();
        if t.spotify_id.is_empty() || !seen.insert(t.spotify_id.clone()) {
            continue;
        }
        // Keep a YouTube match we already have for this Spotify track.
        let known = current.iter().find(|c| c.spotify_id == t.spotify_id);
        next.push(PlaylistTrack {
            id: known.map(|c| c.id.clone()).unwrap_or_default(),
            ..t
        });
        if next.len() >= MAX_TRACKS {
            break;
        }
    }
    let added = next
        .iter()
        .filter(|t| !old.contains(t.spotify_id.as_str()))
        .count();
    let removed = old.len()
        - next
            .iter()
            .filter(|t| old.contains(t.spotify_id.as_str()))
            .count();
    for c in current.into_iter().filter(|c| c.spotify_id.is_empty()) {
        if next.len() >= MAX_TRACKS {
            break;
        }
        next.push(c);
    }
    rewrite(store, id, &next)?;
    let _ = store.conn().execute(
        "UPDATE music_playlists SET spotify_synced_at = ?2 WHERE id = ?1",
        rusqlite::params![id, now_millis()],
    );
    Ok((added, removed))
}

/// Copies a playlist (always private, no Spotify link) to `email`.
pub fn copy_to(store: &DataStore, src: &Playlist, email: &str) -> Result<String, &'static str> {
    let id = create(store, email, &src.name, false, "")?;
    let copied: Vec<PlaylistTrack> = tracks(store, &src.id);
    add_tracks(store, &id, copied)?;
    Ok(id)
}

// ------------------------------------------------------ match cache ----

/// Cache key for a Spotify track's YouTube match: normalized title and
/// first artist.
pub fn match_key(title: &str, artist: &str) -> String {
    let first = artist.split(',').next().unwrap_or(artist);
    format!(
        "{}|{}",
        crate::music_lyrics::normalize(title),
        crate::music_lyrics::normalize(first)
    )
}

/// The version of the rules that picked a stored match, kept on each row for
/// reference. Matches are permanent: a Spotify song keeps the YouTube video it
/// was first matched to (so playlists never wait on a new search), and changing
/// the rules does not discard them. To redo one on purpose, delete its row.
pub const MATCH_RULES_VERSION: i64 = 3;

/// The stored YouTube video for a song, if it was ever matched.
pub fn get_match(store: &DataStore, key: &str) -> Option<String> {
    store
        .conn()
        .query_row(
            "SELECT video_id FROM music_track_matches WHERE match_key = ?1",
            rusqlite::params![key],
            |r| r.get(0),
        )
        .ok()
}

pub fn put_match(store: &DataStore, key: &str, video_id: &str) {
    if !valid_video_id(video_id) {
        return;
    }
    let _ = store.conn().execute(
        "INSERT INTO music_track_matches (match_key, video_id, matched_at, rules_version) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(match_key) DO UPDATE SET video_id = ?2, matched_at = ?3, rules_version = ?4",
        rusqlite::params![key, video_id, now_millis(), MATCH_RULES_VERSION],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_made_under_older_rules_are_still_used() {
        let (dir, s) = store();
        s.conn()
            .execute(
                "INSERT INTO music_track_matches (match_key, video_id, matched_at, rules_version) VALUES (?1, 'aaaaaaaaaaa', 1, 1)",
                rusqlite::params![match_key("One", "A")],
            )
            .unwrap();
        assert_eq!(
            get_match(&s, &match_key("One", "A")).as_deref(),
            Some("aaaaaaaaaaa"),
            "a stored match is kept forever"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    fn store() -> (std::path::PathBuf, DataStore) {
        let dir = std::env::temp_dir().join(format!("music-pl-{}", random_bytes_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        let s = DataStore::open(&dir, &dir).unwrap();
        (dir, s)
    }

    fn t(id: &str, title: &str) -> PlaylistTrack {
        PlaylistTrack {
            id: id.into(),
            title: title.into(),
            artist: "A".into(),
            duration: 100,
            spotify_id: String::new(),
        }
    }
    fn sp(sid: &str, title: &str) -> PlaylistTrack {
        PlaylistTrack {
            id: String::new(),
            spotify_id: sid.into(),
            ..t("", title)
        }
    }

    #[test]
    fn access_rules() {
        let (dir, s) = store();
        let id = create(&s, "Owner@x.test", "  Road   trip ", false, "").unwrap();
        let pl = get(&s, &id).unwrap();
        assert_eq!(pl.name, "Road trip");
        assert!(can_view(&pl, "owner@x.test", ""));
        assert!(!can_view(&pl, "other@x.test", ""));
        let token = set_share(&s, &id, true);
        let pl = get(&s, &id).unwrap();
        assert!(can_view(&pl, "other@x.test", &token));
        assert!(!can_view(&pl, "other@x.test", "wrong"));
        assert!(!can_view(&pl, "other@x.test", ""));
        set_share(&s, &id, false);
        let pl = get(&s, &id).unwrap();
        assert!(!can_view(&pl, "other@x.test", &token));
        assert!(
            !can_view(&pl, "other@x.test", ""),
            "empty token never matches"
        );
        update(&s, &id, None, Some(true)).unwrap();
        assert!(can_view(&get(&s, &id).unwrap(), "other@x.test", ""));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tracks_add_dedupe_move_remove() {
        let (dir, s) = store();
        let id = create(&s, "o@x.test", "P", false, "").unwrap();
        let added = add_tracks(
            &s,
            &id,
            vec![
                t("aaaaaaaaaaa", "1"),
                t("bbbbbbbbbbb", "2"),
                t("aaaaaaaaaaa", "dup"),
                t("bad", "x"),
            ],
        )
        .unwrap();
        assert_eq!(added, 2);
        move_track(&s, &id, 1, 0).unwrap();
        assert_eq!(tracks(&s, &id)[0].title, "2");
        remove_track(&s, &id, 0).unwrap();
        let left = tracks(&s, &id);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].title, "1");
        assert!(remove_track(&s, &id, 5).is_err());
        assert!(move_track(&s, &id, 0, 3).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn caps_names_playlists_and_tracks() {
        let (dir, s) = store();
        assert!(create(&s, "o@x.test", "   ", false, "").is_err());
        assert_eq!(
            clean_name(&"x".repeat(200)).unwrap().chars().count(),
            MAX_NAME_CHARS
        );
        let id = create(&s, "o@x.test", "Big", false, "").unwrap();
        let many: Vec<_> = (0..MAX_TRACKS + 20)
            .map(|i| sp(&format!("sp{i}"), "t"))
            .collect();
        assert_eq!(add_tracks(&s, &id, many).unwrap(), MAX_TRACKS);
        assert_eq!(
            add_tracks(&s, &id, vec![sp("one-more", "t")]),
            Err("playlist is full")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn spotify_sync_keeps_matches_and_manual_tracks() {
        let (dir, s) = store();
        let id = create(&s, "o@x.test", "S", true, "37i9dQZF1DXcBWIGoYBM5M").unwrap();
        sync_spotify(&s, &id, vec![sp("s1", "one"), sp("s2", "two")]).unwrap();
        add_tracks(&s, &id, vec![t("ccccccccccc", "mine")]).unwrap();
        // Pretend s1 got matched to a video earlier.
        let mut cur = tracks(&s, &id);
        cur[0].id = "ddddddddddd".into();
        rewrite(&s, &id, &cur).unwrap();
        let (added, removed) =
            sync_spotify(&s, &id, vec![sp("s1", "one"), sp("s3", "three")]).unwrap();
        assert_eq!((added, removed), (1, 1));
        let now = tracks(&s, &id);
        let titles: Vec<_> = now.iter().map(|x| x.title.as_str()).collect();
        assert_eq!(titles, vec!["one", "three", "mine"]);
        assert_eq!(now[0].id, "ddddddddddd", "existing match survives a sync");
        assert!(get(&s, &id).unwrap().spotify_synced_at > 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn public_listing_and_copy() {
        let (dir, s) = store();
        let a = create(&s, "o@x.test", "Summer 100%", true, "").unwrap();
        add_tracks(&s, &a, vec![t("aaaaaaaaaaa", "1")]).unwrap();
        let _empty = create(&s, "o@x.test", "Summer empty", true, "").unwrap();
        let _private = create(&s, "o@x.test", "Summer private", false, "").unwrap();
        let listed = list_public(&s, "summer", 50);
        assert_eq!(listed.len(), 1, "private and empty playlists aren't listed");
        assert_eq!(list_public(&s, "100%", 50).len(), 1);
        assert_eq!(list_public(&s, "_", 50).len(), 0, "wildcards are literal");
        let copy = copy_to(&s, &get(&s, &a).unwrap(), "friend@x.test").unwrap();
        let c = get(&s, &copy).unwrap();
        assert!(!c.public && is_owner(&c, "friend@x.test"));
        assert_eq!(tracks(&s, &copy).len(), 1);
        delete(&s, &a);
        assert!(get(&s, &a).is_none() && tracks(&s, &a).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn match_cache_round_trip() {
        let (dir, s) = store();
        let k = match_key("Song (Remastered)", "Artist One, Artist Two");
        assert_eq!(k, match_key("song", "artist one"));
        assert!(get_match(&s, &k).is_none());
        put_match(&s, &k, "not valid");
        assert!(get_match(&s, &k).is_none());
        put_match(&s, &k, "aaaaaaaaaaa");
        assert_eq!(get_match(&s, &k).as_deref(), Some("aaaaaaaaaaa"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
