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
//! - links nothing needs (expired and not in any playlist) are pruned. Deleting
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
const REFRESH_PER_CYCLE: i64 = 25;
const MATCH_PER_CYCLE: i64 = 10;

/// Starts the background worker. Call once at startup.
pub fn spawn(state: Arc<AppState>) {
    ytdlp::attach_store(state.store.clone());
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
        tokio::time::sleep(Duration::from_secs(1)).await;
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
        let ids = pl::tracks(&state.store, &playlist_id)
            .into_iter()
            .map(|t| t.id)
            .filter(|id| !id.is_empty())
            .collect();
        ytdlp::warm(ids);
    });
}
