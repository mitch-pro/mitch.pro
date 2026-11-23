//! Keeps playlist songs ready to play (see `docs/music.md`).
//!
//! Stream links from YouTube last about six hours and only work from the IP
//! that asked for them, so nothing can be stored "forever". Instead this worker
//! keeps every song that is in a playlist supplied with a fresh link in
//! `music_stream_links`:
//!
//! - every 5 minutes, songs in playlists with no link, or one that lapses within
//!   45 minutes, are resolved again (at most `REFRESH_PER_CYCLE` per cycle, one at
//!   a time, so a large library can't hammer the source);
//! - Spotify tracks without a YouTube video yet are matched, and the match is
//!   written to every playlist row holding that track;
//! - links nothing needs (expired and not in any playlist) are pruned, and
//!   lyrics cache rows past their time to live are removed (lyrics of songs in a
//!   playlist are kept);
//! - songs in playlists that have no lyrics cached get them fetched (a few at a
//!   time, `warm_lyrics`). Deleting
//!   a playlist drops its songs' links at once (`music_playlists::delete`).
//!
//! `warm_playlist` does the same for one playlist right away, when it is opened,
//! created, added to or imported, so its songs are ready before anyone presses
//! play. Searches warm their results the same way (`ytdlp::warm`).

use super::{playlists, ytdlp};
use crate::state::AppState;
use mitch_lib::music_playlists as pl;
use std::sync::Arc;
use std::time::Duration;

const CYCLE: Duration = Duration::from_secs(300);
const REFRESH_WINDOW_MS: i64 = 45 * 60 * 1000;
const REFRESH_PER_CYCLE: i64 = 60;
const MATCH_PER_CYCLE: i64 = 30;

/// Starts the background worker. Call once at startup.
pub fn spawn(state: Arc<AppState>) {
    ytdlp::attach_store(state.store.clone());
    // Warm yt-dlp's player cache so the first real click isn't the slow one.
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        ytdlp::prime().await;
    });
    tokio::spawn(async move {
        // Let the server settle before the first pass.
        tokio::time::sleep(Duration::from_secs(60)).await;
        loop {
            if !cycle(&state).await {
                // Source missing or blocked: no point trying again soon.
                tokio::time::sleep(CYCLE * 3).await;
            }
            tokio::time::sleep(CYCLE).await;
        }
    });
}

/// One pass. Returns false when the music source is unusable (not installed
/// or blocking us), so the caller backs off.
async fn cycle(state: &Arc<AppState>) -> bool {
    mitch_lib::music::prune_stream_links(&state.store);
    mitch_lib::music::prune_search_cache(&state.store);
    mitch_lib::music::prune_lyrics_cache(&state.store);
    // Playlist songs without lyrics yet (a few per cycle): they are kept until
    // the playlist is deleted, so this fills them in once.
    warm_lyrics(
        state,
        mitch_lib::music::playlist_lyrics_candidates(&state.store, 400),
    );
    if !match_spotify(state, MATCH_PER_CYCLE).await {
        return false;
    }
    let now = mitch_lib::school::now_millis();
    let due = mitch_lib::music::playlist_videos_needing_link(
        &state.store,
        now + REFRESH_WINDOW_MS,
        REFRESH_PER_CYCLE,
    );
    for id in due {
        match ytdlp::refresh(&id).await {
            Ok(_) => {}
            Err(ytdlp::YtError::Blocked | ytdlp::YtError::NotInstalled) => return false,
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_millis(600)).await;
    }
    true
}

/// Finds YouTube videos for up to `limit` unmatched Spotify tracks in
/// playlists. Returns false if the source is blocked or missing.
async fn match_spotify(state: &Arc<AppState>, limit: i64) -> bool {
    for (spotify_id, title, artist, duration) in pl::unmatched_spotify_tracks(&state.store, limit) {
        let key = pl::match_key(&title, &artist);
        let id = match pl::get_match(&state.store, &key) {
            Some(id) => Some(id),
            None => match playlists::find_match(&title, &artist, duration).await {
                Ok(found) => {
                    if let Some(id) = &found {
                        pl::put_match(&state.store, &key, id);
                    }
                    found
                }
                Err(ytdlp::YtError::Blocked | ytdlp::YtError::NotInstalled) => return false,
                Err(_) => None,
            },
        };
        if let Some(id) = id {
            pl::set_spotify_match(&state.store, &spotify_id, &id);
        }
    }
    true
}

/// Most songs queued or running for background lyrics fetching at once.
const LYRICS_BACKLOG_MAX: usize = 30;
/// Lyrics fetches in flight at once (each one asks several sites).
const LYRICS_CONCURRENCY: usize = 2;

/// Fetches lyrics for songs `(video id, title, artist, seconds)` in the
/// background so they are cached before anyone plays them. Songs already cached
/// are skipped. A burst beyond `LYRICS_BACKLOG_MAX` waiting songs is dropped
/// (a play still looks up its own). Only found lyrics are cached by this path.
pub fn warm_lyrics(state: &Arc<AppState>, songs: Vec<(String, String, String, i64)>) {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static IN_FLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    static SLOTS: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    let todo: Vec<(String, String, String, i64, String)> = {
        let mut set = IN_FLIGHT
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut keep = Vec::new();
        for (id, title, artist, duration) in songs {
            if set.len() >= LYRICS_BACKLOG_MAX {
                break;
            }
            if !mitch_lib::music::valid_video_id(&id)
                || title.chars().count() > 200
                || artist.chars().count() > 200
            {
                continue;
            }
            let key = mitch_lib::music::lyrics_cache_key(&id, title.trim(), artist.trim());
            if mitch_lib::music::get_cached_lyrics(&state.store, &key).is_some() {
                continue;
            }
            if set.insert(key.clone()) {
                keep.push((id, title, artist, duration, key));
            }
        }
        keep
    };
    for (id, title, artist, duration, key) in todo {
        let state = state.clone();
        tokio::spawn(async move {
            let slots = SLOTS.get_or_init(|| tokio::sync::Semaphore::new(LYRICS_CONCURRENCY));
            if let Ok(_permit) = slots.acquire().await {
                let secs = u32::try_from(duration.clamp(0, 86_400)).unwrap_or(0);
                super::fetch_and_cache(
                    &state,
                    &id,
                    title.trim(),
                    artist.trim(),
                    secs,
                    super::FetchMode::Ahead,
                )
                .await;
            }
            if let Some(set) = IN_FLIGHT.get() {
                set.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
            }
        });
    }
}

/// Gets one playlist's songs ready: match any Spotify tracks, then resolve a
/// stream link for every song. Runs in the background; never blocks a request.
pub fn warm_playlist(state: &Arc<AppState>, playlist_id: &str) {
    let state = state.clone();
    let playlist_id = playlist_id.to_string();
    tokio::spawn(async move {
        let tracks = pl::tracks(&state.store, &playlist_id);
        let mut ok = true;
        for t in tracks.iter().filter(|t| t.id.is_empty()) {
            let key = pl::match_key(&t.title, &t.artist);
            let id = match pl::get_match(&state.store, &key) {
                Some(id) => Some(id),
                None => match playlists::find_match(&t.title, &t.artist, t.duration).await {
                    Ok(found) => {
                        if let Some(id) = &found {
                            pl::put_match(&state.store, &key, id);
                        }
                        found
                    }
                    Err(ytdlp::YtError::Blocked | ytdlp::YtError::NotInstalled) => {
                        ok = false;
                        break;
                    }
                    Err(_) => None,
                },
            };
            if let Some(id) = id {
                pl::set_spotify_match(&state.store, &t.spotify_id, &id);
            }
        }
        if !ok {
            return;
        }
        let tracks = pl::tracks(&state.store, &playlist_id);
        warm_lyrics(
            &state,
            tracks
                .iter()
                .filter(|t| !t.id.is_empty())
                .map(|t| (t.id.clone(), t.title.clone(), t.artist.clone(), t.duration))
                .collect(),
        );
        ytdlp::warm(
            tracks
                .into_iter()
                .map(|t| t.id)
                .filter(|id| !id.is_empty())
                .collect(),
        );
    });
}
