//! Cover art for `GET /api/music/thumb` (see `docs/music.md`, "Cover art").
//!
//! A YouTube video's own thumbnail is whatever the uploader picked (often a
//! generic frame or channel icon), so the real album cover is looked up first,
//! by the song's title and artist, from iTunes and then Deezer. The video's
//! thumbnail stays as the fallback when neither knows the song, or when the
//! site has no title/artist for the video.
//!
//! Safety: the lookup only ever sends the song's title and artist (never who is
//! asking), image URLs from the catalogs are accepted only from Apple's and
//! Deezer's image hosts over https, redirects are not followed, and images are
//! size-limited. Results (including "none found") are cached per video id in
//! `music_cover_art`, so each song costs at most a couple of requests.

use super::{playlists, ytdlp};
use crate::state::AppState;
use mitch_lib::music as store;
use mitch_lib::music_lyrics as ml;
use serde_json::Value;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::Semaphore;

/// Largest cover accepted from a catalog.
pub const MAX_IMAGE_BYTES: usize = 3 * 1024 * 1024;
/// A catalog result must score at least this against the wanted title/artist.
const MIN_TITLE_SCORE: f32 = 0.7;
const MIN_ARTIST_SCORE: f32 = 0.6;

fn client() -> Option<&'static reqwest::Client> {
    static C: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(ytdlp::DESKTOP_UA)
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .ok()
    })
    .as_ref()
}

/// The shared client, for fetching the chosen image.
pub fn image_client() -> Option<&'static reqwest::Client> {
    client()
}

/// An image URL is only fetched from Apple's and Deezer's image hosts.
pub fn image_host_ok(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else {
        return false;
    };
    let Some(host) = u.host_str() else {
        return false;
    };
    let under = |d: &str| host == d || host.ends_with(&format!(".{d}"));
    u.scheme() == "https" && (under("mzstatic.com") || under("dzcdn.net"))
}

/// What a catalog returned for one song.
struct Hit {
    title: String,
    artist: String,
    seconds: i64,
    image: String,
}

/// The cover for a video: `Some(url)` when a catalog has one, `None` to use
/// the video's own thumbnail. Cached; unknown videos are not looked up.
pub async fn find(state: &Arc<AppState>, id: &str) -> Option<String> {
    if let Some(cached) = store::get_cover(&state.store, id) {
        return (!cached.is_empty()).then_some(cached);
    }
    let (title, artist, seconds) = store::known_track(&state.store, id)?;
    // At most a few lookups at once, so a page of new songs can't flood the catalogs.
    static SLOTS: OnceLock<Semaphore> = OnceLock::new();
    let _permit = SLOTS
        .get_or_init(|| Semaphore::new(4))
        .acquire()
        .await
        .ok()?;
    // Another request may have finished the same lookup while this one waited.
    if let Some(cached) = store::get_cover(&state.store, id) {
        return (!cached.is_empty()).then_some(cached);
    }
    let first_artist = artist.split(',').next().unwrap_or("").trim().to_string();
    let found = match itunes(&title, &first_artist).await {
        Some(hits) => pick(&title, &artist, seconds, &hits).map(|u| (u, "itunes")),
        None => None,
    };
    let found = match found {
        Some(f) => Some(f),
        None => match deezer(&title, &first_artist).await {
            Some(hits) => pick(&title, &artist, seconds, &hits).map(|u| (u, "deezer")),
            None => None,
        },
    };
    match found {
        Some((url, source)) => {
            store::put_cover(&state.store, id, &url, source);
            Some(url)
        }
        None => {
            store::put_cover(&state.store, id, "", "none");
            None
        }
    }
}

/// The best catalog result for the wanted song: the title and artist must
/// agree, the length be within 10 s when both are known, and it must not be a
/// different version (live, remix, ...) unless the wanted title says so.
fn pick(title: &str, artist: &str, seconds: i64, hits: &[Hit]) -> Option<String> {
    let wanted = playlists::version_marks(title);
    let mut best: Option<(f32, &Hit)> = None;
    for h in hits {
        if !image_host_ok(&h.image) {
            continue;
        }
        if seconds > 0 && h.seconds > 0 && (h.seconds - seconds).abs() > 10 {
            continue;
        }
        if playlists::version_marks(&h.title) & !wanted != 0 {
            continue;
        }
        let by_artist = if artist.trim().is_empty() {
            1.0
        } else {
            ml::artist_match(artist, &h.title, &h.artist)
        };
        let by_title = ml::match_score(title, "", &h.title, "");
        if by_artist < MIN_ARTIST_SCORE || by_title < MIN_TITLE_SCORE {
            continue;
        }
        let score = by_title * 0.7 + by_artist * 0.3;
        if best.is_none_or(|(b, _)| score > b) {
            best = Some((score, h));
        }
    }
    best.map(|(_, h)| h.image.clone())
}

async fn get_json(url: &str) -> Option<Value> {
    let resp = client()?.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json().await.ok()
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// iTunes Search API: `artworkUrl100` with its size changed to 600x600.
async fn itunes(title: &str, artist: &str) -> Option<Vec<Hit>> {
    let term = format!("{artist} {title}");
    let url = format!(
        "https://itunes.apple.com/search?media=music&entity=song&limit=8&term={}",
        enc(term.trim())
    );
    let body = get_json(&url).await?;
    let hits = body
        .get("results")?
        .as_array()?
        .iter()
        .filter_map(|r| {
            let art = text(r, "artworkUrl100");
            if art.is_empty() {
                return None;
            }
            Some(Hit {
                title: text(r, "trackName"),
                artist: text(r, "artistName"),
                seconds: r
                    .get("trackTimeMillis")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    / 1000,
                image: art.replace("100x100bb", "600x600bb"),
            })
        })
        .collect();
    Some(hits)
}

/// Deezer search: the album's `cover_xl` (1000x1000).
async fn deezer(title: &str, artist: &str) -> Option<Vec<Hit>> {
    let q = format!("{artist} {title}");
    let url = format!("https://api.deezer.com/search?limit=8&q={}", enc(q.trim()));
    let body = get_json(&url).await?;
    let hits = body
        .get("data")?
        .as_array()?
        .iter()
        .filter_map(|r| {
            let art = r
                .get("album")
                .map(|a| text(a, "cover_xl"))
                .unwrap_or_default();
            if art.is_empty() {
                return None;
            }
            Some(Hit {
                title: text(r, "title"),
                artist: r.get("artist").map(|a| text(a, "name")).unwrap_or_default(),
                seconds: r.get("duration").and_then(Value::as_i64).unwrap_or(0),
                image: art,
            })
        })
        .collect();
    Some(hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(title: &str, artist: &str, secs: i64) -> Hit {
        Hit {
            title: title.into(),
            artist: artist.into(),
            seconds: secs,
            image: "https://is1-ssl.mzstatic.com/image/x/600x600bb.jpg".into(),
        }
    }

    #[test]
    fn only_catalog_image_hosts_are_fetched() {
        assert!(image_host_ok(
            "https://is1-ssl.mzstatic.com/a/600x600bb.jpg"
        ));
        assert!(image_host_ok(
            "https://cdn-images.dzcdn.net/images/cover/x/1000x1000.jpg"
        ));
        assert!(!image_host_ok("http://is1-ssl.mzstatic.com/a.jpg"));
        assert!(!image_host_ok("https://evilmzstatic.com/a.jpg"));
        assert!(!image_host_ok("https://127.0.0.1/a.jpg"));
    }

    #[test]
    fn picks_the_studio_recording_by_the_same_artist() {
        let hits = vec![
            hit("Blinding Lights (Live)", "The Weeknd", 200),
            hit("Blinding Lights", "Someone Else", 200),
            hit("Blinding Lights", "The Weeknd", 200),
        ];
        let got = pick("Blinding Lights", "The Weeknd", 200, &hits);
        assert!(got.is_some());
        // Only the third is acceptable.
        assert_eq!(pick("Blinding Lights", "The Weeknd", 200, &hits[..2]), None);
    }

    #[test]
    fn a_very_different_length_is_another_recording() {
        let hits = vec![hit("Blinding Lights", "The Weeknd", 400)];
        assert_eq!(pick("Blinding Lights", "The Weeknd", 200, &hits), None);
    }
}
