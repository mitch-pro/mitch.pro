//! Saved audio for songs people keep (see `docs/music.md`, "Saved songs").
//!
//! A song in someone's playlist or favorites is downloaded once and kept in the
//! audio store, so it plays without a YouTube lookup and its link never expires.
//! Files are shared: one copy per video, however many people keep the song.
//! Every user may keep up to `USER_QUOTA_BYTES` of distinct songs; a shared song
//! counts toward each person who keeps it. This module is the bookkeeping (what
//! exists, who uses how much, what to save or delete); the bytes live in the
//! store backend (`routes/music/audio_store.rs`).

use crate::data::DataStore;
use crate::music::valid_video_id;

/// Most audio one user may keep saved.
pub const USER_QUOTA_BYTES: i64 = 512 * 1024 * 1024;

/// Bytes per second used to estimate a song's size before it is downloaded
/// (about 136 kbps, a little above YouTube's usual AAC).
const EST_BYTES_PER_SECOND: i64 = 17_000;
const EST_MIN_BYTES: i64 = 2 * 1024 * 1024;

fn now_millis() -> i64 {
    crate::school::now_millis()
}

/// Songs of `email` that are kept: in a playlist they own or in their favorites.
const REFS: &str =
    "SELECT p.owner_email AS email, t.video_id AS video_id, MAX(t.duration_s) AS dur \
     FROM music_playlist_tracks t JOIN music_playlists p ON p.id = t.playlist_id \
     WHERE t.video_id != '' GROUP BY p.owner_email, t.video_id \
     UNION \
     SELECT email, video_id, MAX(duration_s) FROM music_favorites GROUP BY email, video_id";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioFile {
    pub video_id: String,
    pub size: i64,
    pub mime: String,
    /// File extension used in the store (`m4a`, `webm`...).
    pub ext: String,
}

pub fn get_file(store: &DataStore, video_id: &str) -> Option<AudioFile> {
    store
        .conn()
        .query_row(
            "SELECT video_id, size, mime, ext FROM music_audio_files WHERE video_id = ?1",
            rusqlite::params![video_id],
            |r| {
                Ok(AudioFile {
                    video_id: r.get(0)?,
                    size: r.get(1)?,
                    mime: r.get(2)?,
                    ext: r.get(3)?,
                })
            },
        )
        .ok()
}

pub fn put_file(store: &DataStore, f: &AudioFile) {
    if !valid_video_id(&f.video_id) {
        return;
    }
    let _ = store.conn().execute(
        "INSERT INTO music_audio_files (video_id, size, mime, ext, stored_at) VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(video_id) DO UPDATE SET size = ?2, mime = ?3, ext = ?4, stored_at = ?5",
        rusqlite::params![f.video_id, f.size, f.mime, f.ext, now_millis()],
    );
    let _ = store.conn().execute(
        "DELETE FROM music_audio_failures WHERE video_id = ?1",
        rusqlite::params![f.video_id],
    );
}

pub fn delete_file(store: &DataStore, video_id: &str) {
    let _ = store.conn().execute(
        "DELETE FROM music_audio_files WHERE video_id = ?1",
        rusqlite::params![video_id],
    );
}

/// Bytes of saved audio `email` keeps (their playlists and favorites).
pub fn used_bytes(store: &DataStore, email: &str) -> i64 {
    store
        .conn()
        .query_row(
            &format!(
                "SELECT COALESCE(SUM(f.size), 0) FROM music_audio_files f \
                 WHERE f.video_id IN (SELECT video_id FROM ({REFS}) WHERE email = ?1)"
            ),
            rusqlite::params![email],
            |r| r.get(0),
        )
        .unwrap_or(0)
}

/// How many distinct songs `email` keeps, and how many of those are saved.
pub fn counts(store: &DataStore, email: &str) -> (i64, i64) {
    let conn = store.conn();
    let total: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM ({REFS}) WHERE email = ?1"),
            rusqlite::params![email],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let saved: i64 = conn
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM ({REFS}) r JOIN music_audio_files f ON f.video_id = r.video_id \
                 WHERE r.email = ?1"
            ),
            rusqlite::params![email],
            |r| r.get(0),
        )
        .unwrap_or(0);
    (total, saved)
}

fn estimate(duration_s: i64) -> i64 {
    (duration_s.max(0) * EST_BYTES_PER_SECOND).max(EST_MIN_BYTES)
}

/// People who keep `video_id`.
fn keepers(store: &DataStore, video_id: &str) -> Vec<String> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(&format!("SELECT email FROM ({REFS}) WHERE video_id = ?1"))
    else {
        return Vec::new();
    };
    let rows = stmt.query_map(rusqlite::params![video_id], |r| r.get(0));
    rows.map(|r| r.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// Whether at least one person who keeps `video_id` would stay within their
/// quota with `size` more bytes saved.
pub fn has_room(store: &DataStore, video_id: &str, size: i64) -> bool {
    keepers(store, video_id)
        .iter()
        .any(|email| used_bytes(store, email) + size <= USER_QUOTA_BYTES)
}

/// Songs to save next: kept by someone, not saved yet, not waiting out a
/// failure, and small enough (by estimate) for one of their keepers' quota.
/// Returns `(video_id, duration_s)`, oldest additions first.
pub fn next_to_save(store: &DataStore, limit: usize) -> Vec<(String, i64)> {
    let now = now_millis();
    let rows: Vec<(String, i64)> = {
        let conn = store.conn();
        let Ok(mut stmt) = conn.prepare(&format!(
            "SELECT r.video_id, MAX(r.dur) FROM ({REFS}) r \
             LEFT JOIN music_audio_files f ON f.video_id = r.video_id \
             LEFT JOIN music_audio_failures x ON x.video_id = r.video_id \
             WHERE f.video_id IS NULL AND (x.video_id IS NULL OR x.next_try_at <= ?1) \
             GROUP BY r.video_id LIMIT 400"
        )) else {
            return Vec::new();
        };
        let mapped = stmt.query_map(rusqlite::params![now], |r| Ok((r.get(0)?, r.get(1)?)));
        mapped
            .map(|m| m.filter_map(Result::ok).collect())
            .unwrap_or_default()
    };
    rows.into_iter()
        .filter(|(id, dur)| has_room(store, id, estimate(*dur)))
        .take(limit)
        .collect()
}

/// Whether `video_id` should be saved now: nobody has it saved, it is not
/// waiting out a failure, and some keeper has room for it.
pub fn wants_save(store: &DataStore, video_id: &str) -> bool {
    if get_file(store, video_id).is_some() {
        return false;
    }
    let conn = store.conn();
    let waiting: bool = conn
        .query_row(
            "SELECT 1 FROM music_audio_failures WHERE video_id = ?1 AND next_try_at > ?2",
            rusqlite::params![video_id, now_millis()],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if waiting {
        return false;
    }
    let dur: i64 = conn
        .query_row(
            &format!("SELECT COALESCE(MAX(dur), 0) FROM ({REFS}) WHERE video_id = ?1"),
            rusqlite::params![video_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    drop(conn);
    has_room(store, video_id, estimate(dur))
}

/// A save failed: wait longer each time (1 h, 2 h, ... at most 24 h).
pub fn record_failure(store: &DataStore, video_id: &str) {
    let conn = store.conn();
    let tries: i64 = conn
        .query_row(
            "SELECT tries FROM music_audio_failures WHERE video_id = ?1",
            rusqlite::params![video_id],
            |r| r.get(0),
        )
        .unwrap_or(0)
        + 1;
    let wait_ms = (3_600_000_i64 << (tries - 1).clamp(0, 5)).min(24 * 3_600_000);
    let _ = conn.execute(
        "INSERT INTO music_audio_failures (video_id, tries, next_try_at) VALUES (?1, ?2, ?3) \
         ON CONFLICT(video_id) DO UPDATE SET tries = ?2, next_try_at = ?3",
        rusqlite::params![video_id, tries, now_millis() + wait_ms],
    );
}

/// Saved files nobody keeps any more (playlist deleted, song removed...).
pub fn orphan_files(store: &DataStore, limit: i64) -> Vec<AudioFile> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT video_id, size, mime, ext FROM music_audio_files \
         WHERE video_id NOT IN (SELECT video_id FROM ({REFS})) LIMIT ?1"
    )) else {
        return Vec::new();
    };
    let rows = stmt.query_map(rusqlite::params![limit], |r| {
        Ok(AudioFile {
            video_id: r.get(0)?,
            size: r.get(1)?,
            mime: r.get(2)?,
            ext: r.get(3)?,
        })
    });
    rows.map(|r| r.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// Failure rows for songs nobody keeps are dropped.
pub fn prune_failures(store: &DataStore) {
    let _ = store.conn().execute(
        &format!(
            "DELETE FROM music_audio_failures WHERE video_id NOT IN (SELECT video_id FROM ({REFS}))"
        ),
        [],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::random_bytes_hex;

    fn store() -> (std::path::PathBuf, DataStore) {
        let dir = std::env::temp_dir().join(format!("music-audio-{}", random_bytes_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        let s = DataStore::open(&dir, &dir).unwrap();
        (dir, s)
    }

    fn keep(s: &DataStore, email: &str, id: &str, secs: i64) {
        s.conn()
            .execute(
                "INSERT OR IGNORE INTO music_favorites (email, video_id, title, artist, duration_s, added_at) \
                 VALUES (?1, ?2, 't', 'a', ?3, 1)",
                rusqlite::params![email, id, secs],
            )
            .unwrap();
    }

    fn file(id: &str, size: i64) -> AudioFile {
        AudioFile {
            video_id: id.into(),
            size,
            mime: "audio/mp4".into(),
            ext: "m4a".into(),
        }
    }

    #[test]
    fn quota_counts_each_keepers_songs_and_shared_files_count_for_both() {
        let (dir, s) = store();
        keep(&s, "a@example.com", "aaaaaaaaaaa", 200);
        keep(&s, "b@example.com", "aaaaaaaaaaa", 200);
        keep(&s, "a@example.com", "bbbbbbbbbbb", 200);
        put_file(&s, &file("aaaaaaaaaaa", 5_000_000));
        put_file(&s, &file("bbbbbbbbbbb", 7_000_000));
        assert_eq!(used_bytes(&s, "a@example.com"), 12_000_000);
        assert_eq!(used_bytes(&s, "b@example.com"), 5_000_000);
        assert_eq!(counts(&s, "a@example.com"), (2, 2));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn songs_are_saved_only_while_a_keeper_has_room() {
        let (dir, s) = store();
        keep(&s, "a@example.com", "aaaaaaaaaaa", 200);
        assert_eq!(next_to_save(&s, 10), vec![("aaaaaaaaaaa".to_string(), 200)]);
        // A is nearly full: no room for another song, so it isn't picked.
        put_file(&s, &file("zzzzzzzzzzz", USER_QUOTA_BYTES - 1_000_000));
        keep(&s, "a@example.com", "zzzzzzzzzzz", 200);
        assert!(next_to_save(&s, 10).is_empty());
        // But a second keeper with room makes it worth saving.
        keep(&s, "b@example.com", "aaaaaaaaaaa", 200);
        assert_eq!(next_to_save(&s, 10).len(), 1);
        assert!(has_room(&s, "aaaaaaaaaaa", 5_000_000));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failures_back_off_and_a_saved_song_clears_them() {
        let (dir, s) = store();
        keep(&s, "a@example.com", "aaaaaaaaaaa", 200);
        record_failure(&s, "aaaaaaaaaaa");
        assert!(next_to_save(&s, 10).is_empty(), "waits out the failure");
        put_file(&s, &file("aaaaaaaaaaa", 4_000_000));
        let left: i64 = s
            .conn()
            .query_row("SELECT COUNT(*) FROM music_audio_failures", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(left, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn files_nobody_keeps_are_orphans() {
        let (dir, s) = store();
        keep(&s, "a@example.com", "aaaaaaaaaaa", 200);
        put_file(&s, &file("aaaaaaaaaaa", 1));
        put_file(&s, &file("bbbbbbbbbbb", 1));
        let orphans = orphan_files(&s, 10);
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].video_id, "bbbbbbbbbbb");
        let _ = std::fs::remove_dir_all(dir);
    }
}
