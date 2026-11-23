//! The Discover tab: top songs, songs popular on this site, and browsing by
//! genre (see `docs/music.md`, "Discover").
//!
//! - **Top songs**: an official chart playlist if `MUSIC_TOP_PLAYLIST` names one
//!   (a YouTube playlist id), otherwise YouTube searches for this year's hits.
//! - **Popular here**: songs kept by at least `POPULAR_MIN_KEEPERS` accounts. Only
//!   the songs are returned; nobody's listening is tracked.
//! - **Genres**: one cached search per genre (`GENRES`), the same search cache as
//!   the Search tab, so repeat visits are instant.

use super::{allow, json_response, limited, qs, worker, yt_error, ytdlp};
use crate::state::AppState;
use axum::response::Response;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Fewest accounts that must keep a song for it to count as popular.
const POPULAR_MIN_KEEPERS: i64 = 3;
const LIST_LEN: usize = 25;

/// `(id, name, search words, add the year)`.
pub const GENRES: &[(&str, &str, &str, bool)] = &[
    ("pop", "Pop", "top pop songs", true),
    ("hip-hop", "Hip-hop", "top hip hop songs", true),
    ("rock", "Rock", "top rock songs", true),
    ("r-and-b", "R&B", "top r&b songs", true),
    ("country", "Country", "top country songs", true),
    ("latin", "Latin", "top latin songs", true),
    ("k-pop", "K-pop", "top k-pop songs", true),
    (
        "electronic",
        "Electronic",
        "top electronic dance songs",
        true,
    ),
    ("indie", "Indie", "top indie songs", true),
    (
        "alternative",
        "Alternative",
        "best alternative songs",
        false,
    ),
    ("metal", "Metal", "best metal songs", false),
    ("punk", "Punk", "best punk rock songs", false),
    ("jazz", "Jazz", "best jazz songs", false),
    (
        "classical",
        "Classical",
        "best classical music pieces",
        false,
    ),
    ("lofi", "Lo-fi", "lofi songs with vocals", false),
    ("reggae", "Reggae", "best reggae songs", false),
    ("soul", "Soul & funk", "best soul and funk songs", false),
    ("anime", "Anime", "top anime opening songs", false),
];

fn year() -> i32 {
    use chrono::Datelike;
    chrono::Utc::now().year()
}

/// Songs of a chart playlist, kept for six hours.
async fn chart_playlist(list_id: &str) -> Result<Vec<ytdlp::Track>, ytdlp::YtError> {
    type Cache = std::sync::Mutex<Option<(Instant, String, Vec<ytdlp::Track>)>>;
    static CACHE: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();
    const TTL: Duration = Duration::from_secs(6 * 3600);
    if let Some((at, id, tracks)) = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
    {
        if id == list_id && at.elapsed() < TTL {
            return Ok(tracks.clone());
        }
    }
    let tracks = ytdlp::playlist(list_id, LIST_LEN).await?;
    *CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner()) =
        Some((Instant::now(), list_id.to_string(), tracks.clone()));
    Ok(tracks)
}

async fn top_songs() -> Result<Vec<ytdlp::Track>, ytdlp::YtError> {
    if let Ok(list_id) = std::env::var("MUSIC_TOP_PLAYLIST") {
        let list_id = list_id.trim().to_string();
        if !list_id.is_empty() {
            return chart_playlist(&list_id).await;
        }
    }
    let y = year();
    let first = format!("top songs of {y} official music video");
    let second = format!("billboard hot 100 {y} songs");
    let (a, b) = tokio::join!(ytdlp::search(&first), ytdlp::search(&second));
    // Interleave the two lists so neither dominates, and drop repeats.
    let (a, b) = (a?, b.unwrap_or_default());
    let mut out: Vec<ytdlp::Track> = Vec::new();
    for i in 0..a.len().max(b.len()) {
        for t in [a.get(i), b.get(i)].into_iter().flatten() {
            if !out.iter().any(|o| o.id == t.id) {
                out.push(t.clone());
            }
        }
    }
    out.truncate(LIST_LEN);
    Ok(out)
}

fn warm(state: &Arc<AppState>, tracks: &[ytdlp::Track]) {
    ytdlp::warm_search(tracks.iter().map(|t| t.id.clone()).collect());
    worker::warm_lyrics(
        state,
        tracks
            .iter()
            .map(|t| (t.id.clone(), t.title.clone(), t.artist.clone(), t.duration))
            .collect(),
    );
}

/// `GET /api/music/discover` (the home sections) and
/// `GET /api/music/discover?genre=<id>` (one genre's songs).
pub async fn handle(state: &Arc<AppState>, email: &str, search: &str) -> Response {
    if !allow(email, "discover", 20, Duration::from_secs(60)) {
        return limited();
    }
    let genre = qs(search, "genre");
    if !genre.is_empty() {
        let Some((_, name, words, fresh)) = GENRES.iter().find(|g| g.0 == genre) else {
            return json_response(404, json!({ "error": "unknown genre" }));
        };
        let query = if *fresh {
            format!("{words} {}", year())
        } else {
            (*words).to_string()
        };
        return match ytdlp::search(&query).await {
            Ok(tracks) => {
                warm(state, &tracks);
                json_response(
                    200,
                    json!({ "genre": genre, "name": name, "tracks": tracks }),
                )
            }
            Err(e) => yt_error(&e),
        };
    }
    let top = match top_songs().await {
        Ok(t) => t,
        // The other sections still work when the music source is down.
        Err(e) => {
            tracing::warn!("[music] top songs unavailable: {}", e.public_message());
            Vec::new()
        }
    };
    warm(state, &top);
    let popular: Vec<_> = mitch_lib::music::popular_songs(
        &state.store,
        POPULAR_MIN_KEEPERS,
        LIST_LEN as i64,
    )
    .into_iter()
    .map(|p| json!({ "id": p.id, "title": p.title, "artist": p.artist, "duration": p.duration }))
    .collect();
    let genres: Vec<_> = GENRES
        .iter()
        .map(|g| json!({ "id": g.0, "name": g.1 }))
        .collect();
    json_response(
        200,
        json!({ "top": top, "popular": popular, "genres": genres }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genre_ids_are_unique_and_url_safe() {
        let mut seen = std::collections::HashSet::new();
        for (id, name, words, _) in GENRES {
            assert!(seen.insert(*id), "duplicate genre {id}");
            assert!(id.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'));
            assert!(!name.is_empty() && !words.is_empty());
        }
    }
}
