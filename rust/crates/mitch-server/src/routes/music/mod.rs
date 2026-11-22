//! `/api/music/*`: the music player's backend (signed-in users only).
//! See `docs/music.md` for the whole design.
//!
//! | endpoint | what |
//! | --- | --- |
//! | `GET search?q=` | yt-dlp search, tracks with cleaned artist/title |
//! | `GET stream?id=` | audio bytes proxied from YouTube's CDN, Range supported |
//! | `GET prefetch?id=` | resolves a stream URL ahead of time (204) |
//! | `GET thumb?id=` | cover art from i.ytimg.com, through this server |
//! | `GET related?id=` | YouTube's Mix for a song, for smart shuffle |
//! | `GET lyrics?id=&title=&artist=&duration=` | best lyrics from every source, word-timed when possible |
//! | `GET library`, `POST library/add`, `POST library/remove` | saved tracks |
//!
//! Every endpoint requires a login. Audio, art and lyrics all go through
//! this server so that a network that blocks YouTube or the lyric sites
//! still works, and so no third party sees the visitor.

mod audio_store;
mod lyrics;
mod playlists;
mod saved;
mod spotify;
mod worker;
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
pub(super) fn allow(email: &str, bucket: &str, max: usize, window: Duration) -> bool {
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

pub(super) fn limited() -> Response {
    json_response(429, json!({ "error": "slow down a little" }))
}

pub(super) fn qs(search: &str, key: &str) -> String {
    url::form_urlencoded::parse(search.as_bytes())
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

/// Starts the music background worker (called once at startup).
pub fn spawn_worker(state: Arc<AppState>) {
    saved::spawn(state.clone());
    worker::spawn(state);
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
    if route.starts_with("playlist") || route == "match" {
        let Some(email) = email_of(state, headers) else {
            return Some(json_response(401, json!({ "error": "not logged in" })));
        };
        return playlists::handle(state, &email, method, route, body_bytes, search).await;
    }
    let known = matches!(
        (method.as_str(), route),
        (
            "GET",
            "search"
                | "stream"
                | "prefetch"
                | "thumb"
                | "lyrics"
                | "library"
                | "related"
                | "storage"
        ) | ("POST", "library/add" | "library/remove")
    );
    if !known {
        return None;
    }
    let Some(email) = email_of(state, headers) else {
        return Some(json_response(401, json!({ "error": "not logged in" })));
    };
    Some(match route {
        "search" => search_tracks(state, &email, search).await,
        "stream" => stream(state, &email, headers, search).await,
        "storage" => saved::summary(state, &email),
        "prefetch" => prefetch(&email, search).await,
        "related" => related_tracks(&email, search).await,
        "thumb" => thumb(search).await,
        "lyrics" => lyrics_for(state, &email, search).await,
        "library" => library(state, &email),
        "library/add" => library_add(state, &email, body_bytes),
        _ => library_remove(state, &email, body_bytes),
    })
}

pub(super) fn yt_error(e: &ytdlp::YtError) -> Response {
    // The detail stays in the server log; visitors only get the generic text.
    if let ytdlp::YtError::Failed(detail) = e {
        tracing::warn!("[music] yt-dlp failed: {detail}");
    }
    json_response(e.status(), json!({ "error": e.public_message() }))
}

async fn search_tracks(state: &Arc<AppState>, email: &str, search: &str) -> Response {
    let q = qs(search, "q");
    let q = q.trim();
    if q.is_empty() || q.chars().count() > 120 {
        return json_response(400, json!({ "error": "enter something to search for" }));
    }
    if !allow(email, "search", 20, Duration::from_secs(60)) {
        return limited();
    }
    match ytdlp::search(q).await {
        Ok(tracks) => {
            // Start resolving every result now so a click plays at once.
            ytdlp::warm_search(tracks.iter().map(|t| t.id.clone()).collect());
            // ...and fetch their lyrics ahead of time too, so those are there
            // when a song is picked.
            worker::warm_lyrics(
                state,
                tracks
                    .iter()
                    .map(|t| (t.id.clone(), t.title.clone(), t.artist.clone(), t.duration))
                    .collect(),
            );
            json_response(200, json!({ "tracks": tracks }))
        }
        Err(e) => yt_error(&e),
    }
}

/// `GET related?id=`: songs that go with this one, for smart shuffle.
async fn related_tracks(email: &str, search: &str) -> Response {
    let id = qs(search, "id");
    if !valid_video_id(&id) {
        return json_response(400, json!({ "error": "bad track id" }));
    }
    if !allow(email, "related", 20, Duration::from_secs(60)) {
        return limited();
    }
    match ytdlp::related(&id).await {
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

async fn stream(state: &Arc<AppState>, email: &str, headers: &HeaderMap, search: &str) -> Response {
    let id = qs(search, "id");
    if !valid_video_id(&id) {
        return json_response(400, json!({ "error": "bad track id" }));
    }
    // A saved song plays from the audio store: no lookup, no expiring link.
    if let Some(resp) = saved::serve(state, &id, headers).await {
        return resp;
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
        let t_resolve = Instant::now();
        let (resolved, source) = match ytdlp::resolve_timed(&id).await {
            Ok(r) => r,
            Err(e) => return yt_error(&e),
        };
        let resolve_ms = t_resolve.elapsed().as_millis();
        let t_upstream = Instant::now();
        let upstream = client
            .get(&resolved.url)
            .header(header::RANGE, &range)
            .send()
            .await;
        let upstream_ms = t_upstream.elapsed().as_millis();
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
        // Where the time went, visible in the browser's network panel.
        return out
            .header(
                "server-timing",
                format!("resolve;desc=\"{source}\";dur={resolve_ms}, upstream;dur={upstream_ms}"),
            )
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
    // "Wrong lyrics?" sent again without changing the title or artist: a retry
    // of the normal lookup, which is allowed to improve what everyone gets.
    let retry = custom && qs(search, "retry") == "1";
    let custom = custom && !retry;
    let skip_cache = custom || retry;
    let duration_s = qs(search, "duration")
        .parse::<u32>()
        .unwrap_or(0)
        .min(86_400);
    if title.chars().count() > 200 || artist.chars().count() > 200 {
        return json_response(400, json!({ "error": "title or artist too long" }));
    }
    // Keyed by the title and artist as well as the video, so what a client
    // sends can't replace the lyrics everyone else gets for the video.
    let cache_key = music::lyrics_cache_key(&id, title.trim(), artist.trim());
    if !skip_cache {
        if let Some(mut hit) = music::get_cached_lyrics(&state.store, &cache_key) {
            // Lyrics warmed in the background haven't had their timing checked
            // yet: do it now, once, before they are shown.
            if !hit.aligned && allow(email, "lyrics", 20, Duration::from_secs(60)) {
                hit = align_cached(state, &cache_key, &id, hit).await;
            }
            // Line-timed lyrics get one patient try at word timing in the
            // background (a slow source may have missed the first, short wait).
            if hit.sync == "line" && !hit.upgrade_tried {
                spawn_upgrade(state, &id, title.trim(), artist.trim(), duration_s);
            }
            return lyrics_response(
                &hit.source,
                ml::from_cached(&hit.body, &hit.sync, hit.estimated),
                &[],
                hit.approximate,
            );
        }
    }
    if !allow(email, "lyrics", 20, Duration::from_secs(60)) {
        return limited();
    }
    let fetched = fetch_and_cache(
        state,
        &id,
        title.trim(),
        artist.trim(),
        duration_s,
        if retry {
            FetchMode::Refresh
        } else {
            FetchMode::Request { custom }
        },
    )
    .await;
    match fetched.found {
        Some(f) => {
            let mut l = f.lyrics;
            ml::estimate_words(&mut l);
            lyrics_response(f.source, l, &fetched.tried, f.approximate)
        }
        None => json_response(200, json!({ "found": false, "tried": fetched.tried })),
    }
}

/// Who is asking, which decides what a lookup may do and cache.
#[derive(Clone, Copy)]
pub(super) enum FetchMode {
    /// A visitor asked. `custom` is the "wrong lyrics?" search: it is never
    /// cached, in either direction. A clean "nobody has it" is cached; if every
    /// source errored (network trouble) the next play should try again.
    Request { custom: bool },
    /// Background fetching ahead of a play (search results, playlist songs):
    /// no YouTube-captions fallback (that costs a yt-dlp run) and only found
    /// lyrics are cached, so a miss here never blocks the fuller lookup a real
    /// play does.
    Ahead,
    /// A second, patient lookup for a song whose lyrics are already cached
    /// (the "wrong lyrics?" button pressed without changing anything, or the
    /// background retry for line-timed lyrics). It waits longer for word timing
    /// and replaces the cached row only if the result is better.
    Refresh,
}

pub(super) struct Fetched {
    pub found: Option<lyrics::Found>,
    pub tried: Vec<lyrics::Tried>,
}

/// Looks lyrics up across every source and caches the outcome under the rules
/// above. The one path both the endpoint and the background warmers use.
pub(super) async fn fetch_and_cache(
    state: &Arc<AppState>,
    id: &str,
    title: &str,
    artist: &str,
    duration_s: u32,
    mode: FetchMode,
) -> Fetched {
    let cache_key = music::lyrics_cache_key(id, title, artist);
    let (custom, with_captions, patient) = match mode {
        FetchMode::Request { custom } => (custom, true, false),
        FetchMode::Ahead => (false, false, false),
        FetchMode::Refresh => (false, true, true),
    };
    let (mut found, tried) =
        lyrics::find(title, artist, duration_s * 1000, id, with_captions, patient).await;
    // A real play checks the timing against the video's captions right away; a
    // background fetch leaves that for the first play (`aligned` stays false).
    let mut aligned = true;
    if let Some(f) = found.as_mut() {
        if needs_alignment(f.rank, f.approximate, f.source) {
            if !matches!(mode, FetchMode::Ahead) {
                lyrics::align(id, &mut f.lyrics).await;
            } else {
                aligned = false;
            }
        }
    }
    match &found {
        Some(f) => {
            // A refresh keeps what is cached unless the new result has better
            // timing (word over line over text only).
            let better = match mode {
                FetchMode::Refresh => music::get_cached_lyrics(&state.store, &cache_key)
                    .is_none_or(|old| sync_rank(f.lyrics.sync.as_str()) > sync_rank(&old.sync)),
                _ => true,
            };
            if matches!(mode, FetchMode::Refresh) && !better {
                music::mark_lyrics_upgrade_tried(&state.store, &cache_key);
            }
            if !custom && better {
                music::put_cached_lyrics(
                    &state.store,
                    &cache_key,
                    &CachedLyrics {
                        source: f.source.to_string(),
                        sync: f.lyrics.sync.as_str().to_string(),
                        // Estimated word timing is recomputed on read.
                        estimated: ml::has_unworded_lines(&f.lyrics),
                        body: ml::to_enhanced_lrc(&f.lyrics),
                        approximate: f.approximate,
                        aligned,
                        upgrade_tried: matches!(mode, FetchMode::Refresh),
                    },
                );
            }
        }
        None => {
            let all_failed = tried.iter().all(|t| {
                matches!(
                    t.status.as_str(),
                    "timeout" | "unreachable" | "request failed"
                )
            });
            if matches!(mode, FetchMode::Refresh) {
                music::mark_lyrics_upgrade_tried(&state.store, &cache_key);
            }
            if matches!(mode, FetchMode::Request { custom: false }) && !all_failed {
                music::put_cached_lyrics(
                    &state.store,
                    &cache_key,
                    &CachedLyrics {
                        source: String::new(),
                        sync: "miss".into(),
                        estimated: false,
                        body: String::new(),
                        approximate: false,
                        aligned: true,
                        upgrade_tried: true,
                    },
                );
            }
        }
    }
    Fetched { found, tried }
}

/// How good a lyrics row's timing is: word over line over plain text.
fn sync_rank(sync: &str) -> u8 {
    match sync {
        "word" => 3,
        "line" => 2,
        "none" => 1,
        _ => 0,
    }
}

/// Songs whose background word-timing retry is running now (one each).
fn upgrading() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static U: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    U.get_or_init(Default::default)
}

/// Retries the lookup of a line-timed song patiently in the background and
/// keeps the result if it has word timing. One at a time, and once per song.
fn spawn_upgrade(state: &Arc<AppState>, id: &str, title: &str, artist: &str, duration_s: u32) {
    static ONE_AT_A_TIME: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    let key = music::lyrics_cache_key(id, title, artist);
    if !upgrading()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key.clone())
    {
        return;
    }
    let (state, id, title, artist) = (
        state.clone(),
        id.to_string(),
        title.to_string(),
        artist.to_string(),
    );
    tokio::spawn(async move {
        if let Ok(_permit) = ONE_AT_A_TIME.acquire().await {
            let _ =
                fetch_and_cache(&state, &id, &title, &artist, duration_s, FetchMode::Refresh).await;
        }
        upgrading()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&key);
    });
}

/// Whether a result's timing is worth checking against the captions: it has
/// line or word timing from a lyrics source for this length of recording. (The
/// captions themselves, plain text and full-length matches have nothing to fix.)
fn needs_alignment(rank: u8, approximate: bool, source: &str) -> bool {
    rank >= 3 && !approximate && source != "YouTube captions"
}

/// Songs being aligned right now, so concurrent plays do the work once.
fn aligning() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static A: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    A.get_or_init(Default::default)
}

/// Checks a cached row's timing against the video's captions, stores the
/// result and returns it. Whatever happens the row ends up marked checked.
async fn align_cached(
    state: &Arc<AppState>,
    cache_key: &str,
    id: &str,
    hit: CachedLyrics,
) -> CachedLyrics {
    let mut l = ml::from_cached(&hit.body, &hit.sync, hit.estimated);
    let sync_rank = if l.sync == ml::Sync::Word { 4 } else { 3 };
    if !needs_alignment(sync_rank, hit.approximate, &hit.source) {
        return CachedLyrics {
            aligned: true,
            ..hit
        };
    }
    if !aligning()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(cache_key.to_string())
    {
        return hit; // someone else is on it; show what there is
    }
    let moved = lyrics::align(id, &mut l).await;
    aligning()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(cache_key);
    let row = CachedLyrics {
        body: if moved.is_some() {
            ml::to_enhanced_lrc(&l)
        } else {
            hit.body
        },
        estimated: if moved.is_some() {
            ml::has_unworded_lines(&l)
        } else {
            hit.estimated
        },
        aligned: true,
        ..hit
    };
    music::put_cached_lyrics(&state.store, cache_key, &row);
    row
}

fn lyrics_response(
    source: &str,
    l: ml::Lyrics,
    tried: &[lyrics::Tried],
    approximate: bool,
) -> Response {
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
            "approximate": approximate,
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
