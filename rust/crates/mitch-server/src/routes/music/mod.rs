//! `/api/music/*`: the music player's backend (signed-in users only).
//! See `docs/music.md` for the whole design.
//!
//! | endpoint | what |
//! | --- | --- |
//! | `GET search?q=` | yt-dlp search, tracks with cleaned artist/title |
//! | `GET stream?id=` | audio bytes proxied from YouTube's CDN, Range supported |
//! | `GET prefetch?id=` | resolves a stream URL ahead of time (204) |
//! | `GET thumb?id=` | cover art from i.ytimg.com, through this server |
//! | `GET lyrics?id=&title=&artist=&duration=` | best lyrics from every source, word-timed when possible |
//! | `GET library`, `POST library/add`, `POST library/remove` | saved tracks |
//!
//! Every endpoint requires a login. Audio, art and lyrics all go through
//! this server so that a network that blocks YouTube or the lyric sites
//! still works, and so no third party sees the visitor.

mod lyrics;
mod ytdlp;

use crate::routes::me::{cookies_of, json_response, me_uid, parse_body_strict};
use crate::state::AppState;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::Response;
use mitch_lib::music::{self, valid_video_id, CachedLyrics, Favorite};
use mitch_lib::music_lyrics as ml;
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Largest single upstream range request. Google's CDN refuses some larger
/// open-ended ranges; a song is usually under this anyway.
const MAX_RANGE_BYTES: u64 = 10 * 1024 * 1024;

fn email_of(state: &AppState, headers: &HeaderMap) -> Option<String> {
    let cookies = cookies_of(state, headers);
    mitch_lib::auth::email_from_sid(&state.store, &state.id_secret, &me_uid(&cookies))
}

/// Per-user sliding-window limit for the endpoints that start yt-dlp or call
/// outside APIs. In memory; resets on restart.
fn allow(email: &str, bucket: &str, max: usize, window: Duration) -> bool {
    static LOG: OnceLock<Mutex<HashMap<String, VecDeque<Instant>>>> = OnceLock::new();
    let mut log = LOG
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if log.len() > 2000 {
        log.retain(|_, q| q.back().is_some_and(|t| t.elapsed() < window));
    }
    let q = log.entry(format!("{bucket}:{email}")).or_default();
    while q.front().is_some_and(|t| t.elapsed() >= window) {
        q.pop_front();
    }
    if q.len() >= max {
        return false;
    }
    q.push_back(Instant::now());
    true
}

fn limited() -> Response {
    json_response(429, json!({ "error": "slow down a little" }))
}

fn qs(search: &str, key: &str) -> String {
    url::form_urlencoded::parse(search.as_bytes())
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    body_bytes: &[u8],
    search: &str,
) -> Option<Response> {
    let route = path.strip_prefix("/api/music/")?;
    let known = matches!(
        (method.as_str(), route),
        (
            "GET",
            "search" | "stream" | "prefetch" | "thumb" | "lyrics" | "library"
        ) | ("POST", "library/add" | "library/remove")
    );
    if !known {
        return None;
    }
    let Some(email) = email_of(state, headers) else {
        return Some(json_response(401, json!({ "error": "not logged in" })));
    };
    Some(match route {
        "search" => search_tracks(&email, search).await,
        "stream" => stream(&email, headers, search).await,
        "prefetch" => prefetch(&email, search).await,
        "thumb" => thumb(search).await,
        "lyrics" => lyrics_for(state, &email, search).await,
        "library" => library(state, &email),
        "library/add" => library_add(state, &email, body_bytes),
        _ => library_remove(state, &email, body_bytes),
    })
}

fn yt_error(e: &ytdlp::YtError) -> Response {
    // The detail stays in the server log; visitors only get the generic text.
    if let ytdlp::YtError::Failed(detail) = e {
        tracing::warn!("[music] yt-dlp failed: {detail}");
    }
    json_response(e.status(), json!({ "error": e.public_message() }))
}

async fn search_tracks(email: &str, search: &str) -> Response {
    let q = qs(search, "q");
    let q = q.trim();
    if q.is_empty() || q.chars().count() > 120 {
        return json_response(400, json!({ "error": "enter something to search for" }));
    }
    if !allow(email, "search", 20, Duration::from_secs(60)) {
        return limited();
    }
    match ytdlp::search(q).await {
        Ok(tracks) => json_response(200, json!({ "tracks": tracks })),
        Err(e) => yt_error(&e),
    }
}

async fn prefetch(email: &str, search: &str) -> Response {
    let id = qs(search, "id");
    if !valid_video_id(&id) {
        return json_response(400, json!({ "error": "bad track id" }));
    }
    if !allow(email, "resolve", 40, Duration::from_secs(60)) {
        return limited();
    }
    match ytdlp::resolve(&id).await {
        Ok(_) => Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(axum::body::Body::empty())
            .unwrap_or_default(),
        Err(e) => yt_error(&e),
    }
}

/// A single `bytes=a-b` / `bytes=a-` range, rewritten so it asks upstream
/// for at most `MAX_RANGE_BYTES`. Anything else (multi-range, suffix ranges,
/// junk) is treated as "from the start".
pub(crate) fn upstream_range(client: Option<&str>) -> (String, bool) {
    let Some(spec) = client.and_then(|r| r.trim().strip_prefix("bytes=")) else {
        return (format!("bytes=0-{}", MAX_RANGE_BYTES - 1), false);
    };
    let Some((a, b)) = spec.split_once('-') else {
        return (format!("bytes=0-{}", MAX_RANGE_BYTES - 1), false);
    };
    let Ok(start) = a.trim().parse::<u64>() else {
        return (format!("bytes=0-{}", MAX_RANGE_BYTES - 1), false);
    };
    let cap = start.saturating_add(MAX_RANGE_BYTES - 1);
    let end = match b.trim() {
        "" => cap,
        e => match e.parse::<u64>() {
            Ok(e) => e.min(cap),
            Err(_) => return (format!("bytes=0-{}", MAX_RANGE_BYTES - 1), false),
        },
    };
    (format!("bytes={start}-{end}"), true)
}

/// Total size from a `Content-Range: bytes a-b/total` value.
fn content_range_total(v: &str) -> Option<u64> {
    v.rsplit('/').next()?.trim().parse().ok()
}

async fn stream(email: &str, headers: &HeaderMap, search: &str) -> Response {
    let id = qs(search, "id");
    if !valid_video_id(&id) {
        return json_response(400, json!({ "error": "bad track id" }));
    }
    let Some(client) = ytdlp::media_client() else {
        return json_response(503, json!({ "error": "music source is not set up" }));
    };
    let (range, client_sent_range) =
        upstream_range(headers.get(header::RANGE).and_then(|v| v.to_str().ok()));
    let mut attempt = 0;
    loop {
        if !ytdlp::is_cached(&id) && !allow(email, "resolve", 40, Duration::from_secs(60)) {
            return limited();
        }
        let resolved = match ytdlp::resolve(&id).await {
            Ok(r) => r,
            Err(e) => return yt_error(&e),
        };
        let upstream = client
            .get(&resolved.url)
            .header(header::RANGE, &range)
            .send()
            .await;
        let resp = match upstream {
            Ok(r) => r,
            Err(_) => return json_response(502, json!({ "error": "could not load that track" })),
        };
        let status = resp.status();
        if (status == StatusCode::FORBIDDEN || status == StatusCode::GONE) && attempt == 0 {
            // The signed URL expired early or was bound to another IP.
            ytdlp::forget(&id);
            attempt += 1;
            continue;
        }
        if !status.is_success() && status != StatusCode::RANGE_NOT_SATISFIABLE {
            return json_response(502, json!({ "error": "could not load that track" }));
        }
        let up_headers = resp.headers().clone();
        let mut out = Response::builder();
        let total = up_headers
            .get(header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(content_range_total);
        if client_sent_range || status == StatusCode::RANGE_NOT_SATISFIABLE {
            out = out.status(status);
            if let Some(cr) = up_headers.get(header::CONTENT_RANGE) {
                out = out.header(header::CONTENT_RANGE, cr);
            }
            if let Some(cl) = up_headers.get(header::CONTENT_LENGTH) {
                out = out.header(header::CONTENT_LENGTH, cl);
            }
        } else {
            // The browser asked for the whole file; answer 200 with the
            // first chunk's length unless we know the total.
            out = out.status(StatusCode::OK);
            if let Some(t) = total {
                out = out.header(header::CONTENT_LENGTH, t);
            }
        }
        // A whole-file request that is bigger than one chunk can't be
        // answered in one go; browsers always send Range for <audio>, so
        // this only matters for odd clients, which get the first chunk.
        let body = axum::body::Body::from_stream(resp.bytes_stream());
        return out
            .header(header::CONTENT_TYPE, resolved.mime)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CACHE_CONTROL, "private, max-age=3600")
            .header("x-content-type-options", "nosniff")
            .body(body)
            .unwrap_or_default();
    }
}

async fn thumb(search: &str) -> Response {
    let id = qs(search, "id");
    if !valid_video_id(&id) {
        return json_response(400, json!({ "error": "bad track id" }));
    }
    let Some(client) = ytdlp::media_client() else {
        return json_response(503, json!({ "error": "music source is not set up" }));
    };
    let url = format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg");
    let Ok(resp) = client.get(url).send().await else {
        return json_response(502, json!({ "error": "no image" }));
    };
    if !resp.status().is_success() {
        return json_response(404, json!({ "error": "no image" }));
    }
    let Ok(bytes) = resp.bytes().await else {
        return json_response(502, json!({ "error": "no image" }));
    };
    Response::builder()
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, max-age=604800"),
        )
        .header("x-content-type-options", "nosniff")
        .body(axum::body::Body::from(bytes))
        .unwrap_or_default()
}

/// `GET lyrics`. Cached per video id; a query with a custom `title`/`artist`
/// (the "wrong lyrics?" search) bypasses the cache both ways, so one user's
/// correction can't replace what everyone else gets.
async fn lyrics_for(state: &Arc<AppState>, email: &str, search: &str) -> Response {
    let id = qs(search, "id");
    if !valid_video_id(&id) {
        return json_response(400, json!({ "error": "bad track id" }));
    }
    let title = qs(search, "title");
    let artist = qs(search, "artist");
    let custom = qs(search, "custom") == "1";
    let duration_s = qs(search, "duration")
        .parse::<u32>()
        .unwrap_or(0)
        .min(86_400);
    if title.chars().count() > 200 || artist.chars().count() > 200 {
        return json_response(400, json!({ "error": "title or artist too long" }));
    }
    if !custom {
        if let Some(hit) = music::get_cached_lyrics(&state.store, &id) {
            return lyrics_response(
                &hit.source,
                ml::from_cached(&hit.body, &hit.sync, hit.estimated),
                &[],
            );
        }
    }
    if !allow(email, "lyrics", 20, Duration::from_secs(60)) {
        return limited();
    }
    let query = lyrics::Query {
        title: title.trim().to_string(),
        artist: artist.trim().to_string(),
        duration_ms: duration_s * 1000,
        video_id: id.clone(),
    };
    let (found, tried) = lyrics::find(query).await;
    match found {
        Some(f) => {
            if !custom {
                music::put_cached_lyrics(
                    &state.store,
                    &id,
                    &CachedLyrics {
                        source: f.source.to_string(),
                        sync: f.lyrics.sync.as_str().to_string(),
                        // Estimated word timing is recomputed on read.
                        estimated: f.lyrics.sync == ml::Sync::Line,
                        body: ml::to_enhanced_lrc(&f.lyrics),
                    },
                );
            }
            let mut l = f.lyrics;
            ml::estimate_words(&mut l);
            lyrics_response(f.source, l, &tried)
        }
        None => {
            // Only a clean "nobody has it" is cached; if every provider
            // errored (network trouble) the next play should try again.
            let all_failed = tried.iter().all(|t| {
                matches!(
                    t.status.as_str(),
                    "timeout" | "unreachable" | "request failed"
                )
            });
            if !custom && !all_failed {
                music::put_cached_lyrics(
                    &state.store,
                    &id,
                    &CachedLyrics {
                        source: String::new(),
                        sync: "miss".into(),
                        estimated: false,
                        body: String::new(),
                    },
                );
            }
            json_response(200, json!({ "found": false, "tried": tried }))
        }
    }
}

fn lyrics_response(source: &str, l: ml::Lyrics, tried: &[lyrics::Tried]) -> Response {
    if l.is_empty() && source.is_empty() {
        return json_response(200, json!({ "found": false, "tried": tried }));
    }
    json_response(
        200,
        json!({
            "found": true,
            "source": source,
            "sync": l.sync,
            "estimated": l.estimated,
            "lines": l.lines,
            "tried": tried,
        }),
    )
}

fn json_str(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

fn library(state: &Arc<AppState>, email: &str) -> Response {
    json_response(
        200,
        json!({ "tracks": music::list_favorites(&state.store, email) }),
    )
}

fn library_add(state: &Arc<AppState>, email: &str, body: &[u8]) -> Response {
    let Some(v) = parse_body_strict(body) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    let fav = Favorite {
        id: json_str(&v, "id"),
        title: json_str(&v, "title"),
        artist: json_str(&v, "artist"),
        duration: v.get("duration").and_then(|d| d.as_i64()).unwrap_or(0),
    };
    match music::add_favorite(&state.store, email, &fav) {
        Ok(()) => json_response(200, json!({ "ok": true })),
        Err(msg) => json_response(400, json!({ "error": msg })),
    }
}

fn library_remove(state: &Arc<AppState>, email: &str, body: &[u8]) -> Response {
    let Some(v) = parse_body_strict(body) else {
        return json_response(400, json!({ "error": "bad json" }));
    };
    music::remove_favorite(&state.store, email, &json_str(&v, "id"));
    json_response(200, json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_capped_and_sanitized() {
        assert_eq!(
            upstream_range(Some("bytes=100-")),
            (format!("bytes=100-{}", 100 + MAX_RANGE_BYTES - 1), true)
        );
        assert_eq!(
            upstream_range(Some("bytes=0-1023")),
            ("bytes=0-1023".into(), true)
        );
        assert_eq!(
            upstream_range(Some("bytes=5-999999999999")),
            (format!("bytes=5-{}", 5 + MAX_RANGE_BYTES - 1), true)
        );
        assert_eq!(
            upstream_range(None),
            (format!("bytes=0-{}", MAX_RANGE_BYTES - 1), false)
        );
        assert!(!upstream_range(Some("bytes=-500")).1);
        assert!(!upstream_range(Some("bytes=0-1,5-9")).1);
        assert!(!upstream_range(Some("garbage")).1);
        assert_eq!(content_range_total("bytes 0-99/4567"), Some(4567));
        assert_eq!(content_range_total("bytes 0-99/*"), None);
    }

    #[test]
    fn rate_limit_window_counts() {
        let who = "limit-test@example.com";
        for _ in 0..3 {
            assert!(allow(who, "t", 3, Duration::from_secs(60)));
        }
        assert!(!allow(who, "t", 3, Duration::from_secs(60)));
        assert!(allow("other@example.com", "t", 3, Duration::from_secs(60)));
    }
}
