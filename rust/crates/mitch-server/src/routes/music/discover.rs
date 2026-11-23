//! The Discover tab: top songs and browsing by genre (see `docs/music.md`,
//! "Discover"). Nothing here is built from what people on this site keep or
//! play.
//!
//! The lists come from Spotify's own public playlists (read the same way as a
//! playlist import, `spotify.rs`): the Top 50 chart for Top songs and an editorial
//! playlist per genre. Spotify has no audio here; each song is matched to a
//! YouTube video, once, and the match is kept for good (`music_track_matches`).
//! Matching runs in the background right after a list is shown, and a song that
//! isn't matched yet is matched when it is played, like an imported playlist.
//! Only genres with a Spotify playlist are listed, and if Spotify can't be read
//! the list is simply unavailable: a YouTube search stands in badly (it returns
//! compilation videos, not songs), so there is no fallback.

use super::{allow, json_response, limited, playlists, qs, spotify, ytdlp};
use crate::state::AppState;
use axum::response::Response;
use mitch_lib::music_playlists as pl;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const LIST_LEN: usize = 25;
const SPOTIFY_TTL: Duration = Duration::from_secs(6 * 3600);

/// Spotify's "Top 50 - Global" chart. `MUSIC_TOP_SPOTIFY_PLAYLIST` replaces it
/// (any public playlist id, for example a country's Top 50).
const TOP_PLAYLIST: &str = "37i9dQZEVXbMDoHDwVN2tF";

/// `(id, name, Spotify playlist id)`: Spotify's editorial playlist for the genre.
/// Add a genre only with a playlist id that has been checked.
pub const GENRES: &[(&str, &str, &str)] = &[
    ("pop", "Pop", "37i9dQZF1DXcBWIGoYBM5M"),
    ("hip-hop", "Hip-hop", "37i9dQZF1DX0XUsuxWHRQd"),
    ("rock", "Rock", "37i9dQZF1DXcF6B6QPhFDv"),
    ("r-and-b", "R&B", "37i9dQZF1DX4SBhb3fqCJd"),
    ("country", "Country", "37i9dQZF1DX1lVhptIYYxV"),
    ("latin", "Latin", "37i9dQZF1DX10zKzsJ2jva"),
    ("k-pop", "K-pop", "37i9dQZF1DX9tPFwDMOaN1"),
    ("electronic", "Electronic", "37i9dQZF1DX4dyzvuaRJ0n"),
    ("metal", "Metal", "37i9dQZF1DX9qNs32fujYe"),
    ("jazz", "Jazz", "37i9dQZF1DXbITWG1ZJKYt"),
    ("classical", "Classical", "37i9dQZF1DWWEJlAGA9gs0"),
    ("lofi", "Lo-fi", "37i9dQZF1DWWQRwui0ExPn"),
];

type SpotifyCache = Mutex<HashMap<String, (Instant, Vec<pl::PlaylistTrack>)>>;

/// The first songs of a Spotify playlist, kept for six hours.
async fn spotify_songs(list_id: &str) -> Option<Vec<pl::PlaylistTrack>> {
    static CACHE: OnceLock<SpotifyCache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((at, tracks)) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(list_id) {
        if at.elapsed() < SPOTIFY_TTL {
            return Some(tracks.clone());
        }
    }
    let fetched = spotify::fetch(list_id).await.ok()?;
    let mut tracks = fetched.tracks;
    tracks.truncate(LIST_LEN);
    if tracks.is_empty() {
        return None;
    }
    let mut map = cache.lock().unwrap_or_else(|e| e.into_inner());
    if map.len() > 64 {
        map.retain(|_, (at, _)| at.elapsed() < SPOTIFY_TTL);
    }
    map.insert(list_id.to_string(), (Instant::now(), tracks.clone()));
    Some(tracks)
}

/// A track as the player expects it. `id` is the YouTube video when this song
/// has been matched before, else empty (matched on play).
fn track_json(state: &AppState, t: &pl::PlaylistTrack) -> Value {
    let id = pl::get_match(&state.store, &pl::match_key(&t.title, &t.artist)).unwrap_or_default();
    json!({
        "id": id,
        "title": t.title,
        "artist": t.artist,
        "duration": t.duration,
        "spotifyId": t.spotify_id,
    })
}

/// Matches songs to YouTube videos in the background, one at a time, so they
/// are ready when someone presses play.
fn match_in_background(state: &Arc<AppState>, tracks: &[pl::PlaylistTrack]) {
    static BUSY: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    static ONE_AT_A_TIME: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    let todo: Vec<pl::PlaylistTrack> = {
        let mut busy = BUSY
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        tracks
            .iter()
            .filter(|t| pl::get_match(&state.store, &pl::match_key(&t.title, &t.artist)).is_none())
            .filter(|t| busy.insert(pl::match_key(&t.title, &t.artist)))
            .cloned()
            .collect()
    };
    if todo.is_empty() {
        return;
    }
    let state = state.clone();
    tokio::spawn(async move {
        let slot = ONE_AT_A_TIME.get_or_init(|| tokio::sync::Semaphore::new(1));
        let Ok(_permit) = slot.acquire().await else {
            return;
        };
        let mut found = Vec::new();
        let mut stop = false;
        for t in &todo {
            let key = pl::match_key(&t.title, &t.artist);
            if !stop {
                match playlists::find_match(&t.title, &t.artist, t.duration).await {
                    Ok(Some(id)) => {
                        pl::put_match(&state.store, &key, &id);
                        found.push(id);
                    }
                    Ok(None) => {}
                    Err(ytdlp::YtError::Blocked | ytdlp::YtError::NotInstalled) => stop = true,
                    Err(_) => {}
                }
            }
            if let Some(busy) = BUSY.get() {
                busy.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
            }
        }
        ytdlp::warm(found);
    });
}

/// Songs for a Spotify playlist as the player's tracks, matching them to
/// YouTube videos in the background. `None` when Spotify can't be read.
async fn list(state: &Arc<AppState>, spotify_id: &str) -> Option<Vec<Value>> {
    let tracks = spotify_songs(spotify_id).await?;
    match_in_background(state, &tracks);
    Some(tracks.iter().map(|t| track_json(state, t)).collect())
}

/// `GET /api/music/discover` (the home sections) and
/// `GET /api/music/discover?genre=<id>` (one genre's songs).
pub async fn handle(state: &Arc<AppState>, email: &str, search: &str) -> Response {
    if !allow(email, "discover", 20, Duration::from_secs(60)) {
        return limited();
    }
    let genre = qs(search, "genre");
    if !genre.is_empty() {
        let Some((_, name, spotify_id)) = GENRES.iter().find(|g| g.0 == genre) else {
            return json_response(404, json!({ "error": "unknown genre" }));
        };
        return match list(state, spotify_id).await {
            Some(tracks) => json_response(
                200,
                json!({ "genre": genre, "name": name, "tracks": tracks }),
            ),
            None => json_response(
                502,
                json!({ "error": "Couldn't load this genre right now. Try again in a bit." }),
            ),
        };
    }
    let top_id = std::env::var("MUSIC_TOP_SPOTIFY_PLAYLIST")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| TOP_PLAYLIST.to_string());
    let top = list(state, &top_id).await.unwrap_or_default();
    let genres: Vec<_> = GENRES
        .iter()
        .map(|g| json!({ "id": g.0, "name": g.1 }))
        .collect();
    json_response(200, json!({ "top": top, "genres": genres }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genre_ids_are_unique_and_url_safe() {
        let mut seen = std::collections::HashSet::new();
        for (id, name, spotify_id) in GENRES {
            assert!(seen.insert(*id), "duplicate genre {id}");
            assert!(id.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'));
            assert!(!name.is_empty());
            assert!(
                spotify_id.len() == 22 && spotify_id.bytes().all(|b| b.is_ascii_alphanumeric()),
                "{id} needs a 22-character Spotify playlist id"
            );
        }
        assert_eq!(TOP_PLAYLIST.len(), 22);
    }
}
