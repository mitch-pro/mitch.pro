//! yt-dlp wrapper: search, stream-URL resolution and caption download.
//!
//! The server never stores audio. It asks yt-dlp for a short-lived signed
//! URL on YouTube's CDN, remembers it (`RESOLVE_TTL`), and `mod.rs` proxies
//! bytes from it to the visitor. Signed URLs are bound to the IP that asked
//! for them, so the proxy fetch and yt-dlp must leave through the same
//! connection: both honor `MUSIC_EGRESS_PROXY`.
//!
//! Environment (all optional):
//! - `MUSIC_YTDLP_BIN`: yt-dlp binary, default `yt-dlp`.
//! - `MUSIC_EGRESS_PROXY`: `http://host:port` proxy for yt-dlp and the
//!   stream/caption/thumbnail fetches (e.g. a VPN egress; datacenter IPs are
//!   often answered with "Sign in to confirm you're not a bot").
//! - `MUSIC_YTDLP_COOKIES`: path to a cookies.txt for yt-dlp.
//! - `MUSIC_YTDLP_JS_RUNTIME`: passed as `--js-runtimes` (e.g. `node`) when
//!   yt-dlp can't find deno on its own.

use mitch_lib::music::valid_video_id;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::sync::Semaphore;

/// A resolved stream URL is reused for this long. Google's URLs last about
/// six hours; this stays well inside that.
const RESOLVE_TTL: Duration = Duration::from_secs(4 * 3600);
const SEARCH_TTL: Duration = Duration::from_secs(10 * 60);
const RUN_TIMEOUT: Duration = Duration::from_secs(25);
/// How long a request may wait for one of the yt-dlp process slots.
const SLOT_WAIT: Duration = Duration::from_secs(20);
/// Most songs queued or running for background link warming at once; the
/// rest of a burst is dropped (a click still resolves its own song).
const WARM_BACKLOG_MAX: usize = 40;
/// How long a search must stand still before its results are warmed.
const WARM_SETTLE: Duration = Duration::from_millis(1500);
/// Tracks longer than this are left out of search (mixes, podcasts, streams).
const MAX_TRACK_SECONDS: i64 = 20 * 60;

pub const DESKTOP_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

#[derive(Debug)]
pub enum YtError {
    /// The yt-dlp binary isn't installed.
    NotInstalled,
    /// YouTube asked for a login (bot check on this IP).
    Blocked,
    Timeout,
    Failed(String),
}

impl YtError {
    pub fn status(&self) -> u16 {
        match self {
            YtError::NotInstalled => 503,
            YtError::Blocked => 502,
            YtError::Timeout => 504,
            YtError::Failed(_) => 502,
        }
    }

    /// Message safe to show a visitor (no paths or stderr).
    pub fn public_message(&self) -> &'static str {
        match self {
            YtError::NotInstalled => "music source is not set up on this server",
            YtError::Blocked => "the music source blocked this server",
            YtError::Timeout => "the music source took too long",
            YtError::Failed(_) => "could not load that track",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub duration: i64,
}

#[derive(Debug, Clone)]
pub struct Resolved {
    pub url: String,
    pub mime: &'static str,
    pub expires: Instant,
}

struct Config {
    bin: String,
    proxy: Option<String>,
    cookies: Option<String>,
    /// The cookies themselves (from `MUSIC_YTDLP_COOKIES_B64`), for deployments
    /// that keep secrets in a secret manager and put no file on the server.
    cookies_data: Option<Vec<u8>>,
    js_runtime: Option<String>,
    cache_dir: String,
    extractor_args: Option<String>,
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// Decodes `MUSIC_YTDLP_COOKIES_B64` (the base64 of a Netscape cookies.txt).
/// A value that isn't valid base64 is ignored with a warning (the value itself
/// is never logged).
fn cookies_from_env() -> Option<Vec<u8>> {
    use base64::Engine;
    let raw = env_nonempty("MUSIC_YTDLP_COOKIES_B64")?;
    let compact: String = raw.split_whitespace().collect();
    match base64::engine::general_purpose::STANDARD.decode(compact) {
        Ok(bytes) if !bytes.is_empty() => Some(bytes),
        _ => {
            tracing::warn!("[music] MUSIC_YTDLP_COOKIES_B64 is not valid base64; ignoring it");
            None
        }
    }
}

fn config() -> &'static Config {
    static CFG: OnceLock<Config> = OnceLock::new();
    CFG.get_or_init(|| Config {
        bin: env_nonempty("MUSIC_YTDLP_BIN").unwrap_or_else(|| "yt-dlp".to_string()),
        proxy: env_nonempty("MUSIC_EGRESS_PROXY"),
        cookies: env_nonempty("MUSIC_YTDLP_COOKIES"),
        cookies_data: cookies_from_env(),
        js_runtime: env_nonempty("MUSIC_YTDLP_JS_RUNTIME"),
        cache_dir: env_nonempty("MUSIC_YTDLP_CACHE_DIR").unwrap_or_else(|| {
            std::env::temp_dir()
                .join("mitch-ytdlp-cache")
                .to_string_lossy()
                .into_owned()
        }),
        extractor_args: env_nonempty("MUSIC_YTDLP_EXTRACTOR_ARGS"),
    })
}

/// HTTP client for everything that must leave through the same connection as
/// yt-dlp (stream bytes, captions, thumbnails). No total timeout: a song is
/// streamed for minutes. A stalled upstream is cut off by the read timeout.
pub fn media_client() -> Option<&'static reqwest::Client> {
    static CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            let mut b = reqwest::Client::builder()
                .user_agent(DESKTOP_UA)
                .connect_timeout(Duration::from_secs(8))
                .read_timeout(Duration::from_secs(20))
                // A redirect may only lead to another host we already fetch
                // from, so a stream URL can't be bounced to an internal address.
                .redirect(reqwest::redirect::Policy::custom(|attempt| {
                    if attempt.previous().len() >= 5 {
                        attempt.error("too many redirects")
                    } else if media_redirect_ok(attempt.url().as_str()) {
                        attempt.follow()
                    } else {
                        attempt.stop()
                    }
                }));
            if let Some(p) = &config().proxy {
                b = b.proxy(reqwest::Proxy::all(p).ok()?);
            }
            b.build().ok()
        })
        .as_ref()
}

fn spawn_slots() -> &'static Semaphore {
    static SLOTS: OnceLock<Semaphore> = OnceLock::new();
    SLOTS.get_or_init(|| Semaphore::new(4))
}

/// A private, short-lived cookies file for one yt-dlp run, removed when the run
/// ends (including when it is cancelled). It is made from the file named by
/// `MUSIC_YTDLP_COOKIES` or from the secret in `MUSIC_YTDLP_COOKIES_B64`, and
/// lives in memory-backed `/dev/shm` when there is one, so a secret held only in
/// a secret manager is never written to disk.
struct CookieCopy(std::path::PathBuf);

impl CookieCopy {
    fn make(path: Option<&str>, data: Option<&[u8]>, fallback_dir: &str) -> Option<Self> {
        let bytes: Vec<u8> = match (data, path) {
            (Some(d), _) => d.to_vec(),
            (None, Some(p)) => match std::fs::read(p) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(
                        "[music] can't read MUSIC_YTDLP_COOKIES ({e}); running without cookies"
                    );
                    return None;
                }
            },
            (None, None) => return None,
        };
        let shm = std::path::Path::new("/dev/shm");
        let dir = if shm.is_dir() {
            shm.to_path_buf()
        } else {
            let d = std::path::PathBuf::from(fallback_dir);
            let _ = std::fs::create_dir_all(&d);
            d
        };
        let dst = dir.join(format!(
            "mitch-cookies-{}.txt",
            mitch_lib::crypto::random_bytes_hex(8)
        ));
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600); // never readable by others, not even briefly
        }
        let written = opts.open(&dst).and_then(|mut f| {
            use std::io::Write;
            f.write_all(&bytes)
        });
        if let Err(e) = written {
            tracing::warn!("[music] can't write a cookies file ({e}); running without cookies");
            let _ = std::fs::remove_file(&dst);
            return None;
        }
        Some(CookieCopy(dst))
    }
}

impl Drop for CookieCopy {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Slots kept for searches. A search is what someone is waiting on, so it never
/// queues behind the stream-link, caption and related-song runs that share the
/// main slots.
fn search_slots() -> &'static Semaphore {
    static SLOTS: OnceLock<Semaphore> = OnceLock::new();
    SLOTS.get_or_init(|| Semaphore::new(3))
}

/// Slots for work nobody is waiting on (warming links, suggestions, checking
/// lyric timing). Kept small and apart from the main slots so a click never
/// queues behind it, and so background work can't crowd the machine's CPU.
fn background_slots() -> &'static Semaphore {
    static SLOTS: OnceLock<Semaphore> = OnceLock::new();
    SLOTS.get_or_init(|| Semaphore::new(2))
}

/// Runs yt-dlp with `args` and returns stdout. At most four run at once for
/// things a person is waiting on (see also `search_slots`, `background_slots`).
async fn run(args: &[String]) -> Result<String, YtError> {
    run_in(args, spawn_slots()).await
}

/// Same, in the background pool.
async fn run_bg(args: &[String]) -> Result<String, YtError> {
    run_in(args, background_slots()).await
}

async fn run_in(args: &[String], slots: &Semaphore) -> Result<String, YtError> {
    let cfg = config();
    // Waiting for a free yt-dlp slot is bounded too, so a burst of requests
    // fails fast instead of piling up.
    let _slot = match tokio::time::timeout(SLOT_WAIT, slots.acquire()).await {
        Ok(Ok(slot)) => slot,
        Ok(Err(_)) => return Err(YtError::Failed("shutting down".into())),
        Err(_) => return Err(YtError::Timeout),
    };
    let mut cmd = Command::new(&cfg.bin);
    cmd.arg("--ignore-config")
        .arg("--no-warnings")
        // yt-dlp caches the solved YouTube player script here. Without it every
        // uncached song would download and re-solve the player first, which is
        // most of a resolve's time.
        .arg("--cache-dir")
        .arg(&cfg.cache_dir)
        .arg("--socket-timeout")
        .arg("12");
    if let Some(p) = &cfg.proxy {
        cmd.arg("--proxy").arg(p);
    }
    // yt-dlp rewrites its cookies file when it exits, and several runs go at
    // once, so each run gets its own copy; the original is never touched.
    let _cookie_copy = CookieCopy::make(
        cfg.cookies.as_deref(),
        cfg.cookies_data.as_deref(),
        &cfg.cache_dir,
    );
    if let Some(copy) = &_cookie_copy {
        cmd.arg("--cookies").arg(&copy.0);
    }
    if let Some(js) = &cfg.js_runtime {
        cmd.arg("--js-runtimes").arg(js);
    }
    if let Some(args) = &cfg.extractor_args {
        cmd.arg("--extractor-args").arg(args);
    }
    cmd.args(args)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null());
    let out = match tokio::time::timeout(RUN_TIMEOUT, cmd.output()).await {
        Err(_) => return Err(YtError::Timeout),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(YtError::NotInstalled)
        }
        Ok(Err(e)) => return Err(YtError::Failed(e.to_string())),
        Ok(Ok(o)) => o,
    };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        if err.contains("Sign in to confirm") || err.contains("not a bot") {
            return Err(YtError::Blocked);
        }
        return Err(YtError::Failed(err.chars().take(300).collect()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

type SearchCache = Mutex<HashMap<String, (Instant, Vec<Track>)>>;

fn search_cache() -> &'static SearchCache {
    static C: OnceLock<SearchCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Queries being refreshed in the background, so a burst of identical
/// searches starts one refresh, not many.
fn refreshing() -> &'static Mutex<std::collections::HashSet<String>> {
    static R: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// Searches YouTube, answering from the cache when it can.
///
/// - Results younger than `SEARCH_TTL` are returned as they are.
/// - Older results (up to 30 days, kept in `music_search_results`) are returned
///   at once and a background refresh fetches the current list for next time.
/// - A query never seen before is fetched live.
pub async fn search(query: &str) -> Result<Vec<Track>, YtError> {
    let key = query.trim().to_lowercase();
    let mem = search_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
        .cloned();
    if let Some((at, hit)) = mem {
        if at.elapsed() < SEARCH_TTL {
            return Ok(hit);
        }
    }
    if let Some(store) = store() {
        if let Some((fetched_at, rows)) = mitch_lib::music::get_cached_search(store, &key) {
            if !rows.is_empty() {
                let tracks: Vec<Track> = rows
                    .into_iter()
                    .map(|r| Track {
                        id: r.id,
                        title: r.title,
                        artist: r.artist,
                        duration: r.duration,
                    })
                    .collect();
                let tracks = only_songs(tracks);
                let age_ms = mitch_lib::school::now_millis() - fetched_at;
                if age_ms >= SEARCH_TTL.as_millis() as i64 {
                    refresh_search(key.clone());
                    // Show what we have for the next few minutes meanwhile.
                    remember(&key, &tracks, Instant::now());
                } else {
                    let at = Instant::now()
                        .checked_sub(Duration::from_millis(age_ms.max(0) as u64))
                        .unwrap_or_else(Instant::now);
                    remember(&key, &tracks, at);
                }
                return Ok(tracks);
            }
        }
    }
    fetch_live(&key).await
}

fn remember(key: &str, tracks: &[Track], at: Instant) {
    let mut cache = search_cache().lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() > 200 {
        cache.retain(|_, (t, _)| t.elapsed() < SEARCH_TTL);
    }
    cache.insert(key.to_string(), (at, tracks.to_vec()));
}

/// Runs yt-dlp for a query and stores the result in memory and the database.
async fn fetch_live(key: &str) -> Result<Vec<Track>, YtError> {
    let started = Instant::now();
    let out = run_in(
        &[
            "--flat-playlist".into(),
            "--dump-single-json".into(),
            format!("ytsearch15:{key}"),
        ],
        search_slots(),
    )
    .await?;
    if started.elapsed() > Duration::from_millis(2500) {
        tracing::info!(
            "[music] slow search ({} ms): yt-dlp itself, not queueing, took this long",
            started.elapsed().as_millis()
        );
    }
    let tracks = parse_search(&out);
    remember(key, &tracks, Instant::now());
    if let Some(store) = store() {
        let rows: Vec<_> = tracks
            .iter()
            .map(|t| mitch_lib::music::SearchRow {
                id: t.id.clone(),
                title: t.title.clone(),
                artist: t.artist.clone(),
                duration: t.duration,
            })
            .collect();
        mitch_lib::music::put_cached_search(store, key, &rows);
    }
    Ok(tracks)
}

/// Checks a stale query for updates in the background. A failure leaves the
/// old results in place.
fn refresh_search(key: String) {
    if !refreshing()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key.clone())
    {
        return;
    }
    tokio::spawn(async move {
        if let Ok(tracks) = fetch_live(&key).await {
            warm(tracks.iter().map(|t| t.id.clone()).collect());
        }
        refreshing()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&key);
    });
}

/// Turns yt-dlp's search JSON into tracks, dropping live streams, mixes and
/// anything without a valid video id and sensible length.
pub fn parse_search(json: &str) -> Vec<Track> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in v
        .get("entries")
        .and_then(|e| e.as_array())
        .into_iter()
        .flatten()
    {
        let id = e.get("id").and_then(|x| x.as_str()).unwrap_or("");
        let dur = e
            .get("duration")
            .and_then(|x| x.as_f64())
            .unwrap_or(0.0)
            .round() as i64;
        let live = matches!(
            e.get("live_status").and_then(|x| x.as_str()),
            Some("is_live") | Some("is_upcoming")
        );
        if !valid_video_id(id) || live || dur <= 0 || dur > MAX_TRACK_SECONDS {
            continue;
        }
        let title = e.get("title").and_then(|x| x.as_str()).unwrap_or("");
        let channel = e
            .get("channel")
            .or_else(|| e.get("uploader"))
            .and_then(|x| x.as_str())
            .unwrap_or("");
        let (artist, title) = mitch_lib::music_lyrics::clean_youtube_title(title, channel);
        out.push(Track {
            id: id.to_string(),
            title,
            artist,
            duration: dur,
        });
    }
    only_songs(out)
}

/// Words in a title that mark a video that isn't a song: reactions, reviews,
/// episodes, compilations, tutorials. Matched as whole words, so "remix" is
/// not "mix".
const NOT_SONG_WORDS: &[&str] = &[
    "react",
    "reacts",
    "reaction",
    "reactions",
    "reacting",
    "review",
    "explained",
    "explanation",
    "breakdown",
    "analysis",
    "tutorial",
    "interview",
    "trailer",
    "episode",
    "podcast",
    "flashmob",
    "compilation",
    "karaoke",
    "playlist",
    "mix",
    "hour",
    "hours",
    "tiktok",
    "shorts",
    "meme",
    "ova",
];

/// Whether a result looks like a song rather than a reaction, episode, mix or
/// other non-music video: a sensible length (1 to 12 minutes) and none of
/// `NOT_SONG_WORDS` or a few phrases in the title.
pub fn looks_like_song(t: &Track) -> bool {
    if !(60..=720).contains(&t.duration) {
        return false;
    }
    let lower = t.title.to_lowercase();
    if [
        "top 10",
        "top 20",
        "full album",
        "side by side",
        "first time",
    ]
    .iter()
    .any(|p| lower.contains(p))
    {
        return false;
    }
    !lower
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| NOT_SONG_WORDS.contains(&w))
}

/// Keeps the songs. If nothing passes (an unusual query), the list is returned
/// as it was rather than showing nothing.
pub fn only_songs(tracks: Vec<Track>) -> Vec<Track> {
    let songs: Vec<Track> = tracks
        .iter()
        .filter(|t| looks_like_song(t))
        .cloned()
        .collect();
    if songs.is_empty() {
        tracks
    } else {
        songs
    }
}

/// The first `limit` songs of a YouTube playlist (for example an official
/// chart). `list_id` must look like a playlist id. Not cached here.
pub async fn playlist(list_id: &str, limit: usize) -> Result<Vec<Track>, YtError> {
    let ok = (10..=64).contains(&list_id.len())
        && list_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !ok {
        return Err(YtError::Failed("bad playlist id".into()));
    }
    let out = run_bg(&[
        "--flat-playlist".into(),
        "--dump-single-json".into(),
        "--playlist-end".into(),
        limit.to_string(),
        format!("https://www.youtube.com/playlist?list={list_id}"),
    ])
    .await?;
    Ok(only_songs(parse_search(&out)))
}

/// Songs YouTube's own "Mix" for a video suggests (its radio of related songs),
/// for smart shuffle. The seed song itself is left out. Cached for 30 minutes.
pub async fn related(id: &str) -> Result<Vec<Track>, YtError> {
    type Cache = Mutex<HashMap<String, (Instant, Vec<Track>)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    const TTL: Duration = Duration::from_secs(30 * 60);
    if !valid_video_id(id) {
        return Err(YtError::Failed("bad id".into()));
    }
    if let Some((at, hit)) = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
    {
        if at.elapsed() < TTL {
            return Ok(hit.clone());
        }
    }
    let out = run_bg(&[
        "--flat-playlist".into(),
        "--dump-single-json".into(),
        "--playlist-end".into(),
        "20".into(),
        format!("https://www.youtube.com/watch?v={id}&list=RD{id}"),
    ])
    .await?;
    let tracks: Vec<Track> = parse_search(&out)
        .into_iter()
        .filter(|t| t.id != id)
        .collect();
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if cache.len() > 200 {
        cache.retain(|_, (at, _)| at.elapsed() < TTL);
    }
    cache.insert(id.to_string(), (Instant::now(), tracks.clone()));
    Ok(tracks)
}

fn resolve_cache() -> &'static Mutex<HashMap<String, Resolved>> {
    static C: OnceLock<Mutex<HashMap<String, Resolved>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// One lock per video id, so two requests for the same new track (the
/// `<audio>` element opens two connections) share one yt-dlp run.
fn resolve_lock(id: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    static L: OnceLock<Mutex<HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>> =
        OnceLock::new();
    let mut map = L
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if map.len() > 512 {
        map.retain(|_, l| std::sync::Arc::strong_count(l) > 1);
    }
    map.entry(id.to_string()).or_default().clone()
}

/// Whether a fresh stream URL is already cached (no yt-dlp run needed).
pub fn is_cached(id: &str) -> bool {
    resolve_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
        .is_some_and(|r| r.expires > Instant::now())
        || from_db(id).is_some()
}

/// Forces a fresh resolution (for the background refresh of playlist songs),
/// replacing the cached link.
pub async fn refresh(id: &str) -> Result<Resolved, YtError> {
    resolve_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(id);
    if let Some(store) = store() {
        mitch_lib::music::delete_stream_link(store, id);
    }
    resolve(id).await
}

pub fn forget(id: &str) {
    resolve_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(id);
    if let Some(store) = store() {
        mitch_lib::music::delete_stream_link(store, id);
    }
}

/// The database that keeps resolved links across restarts. Set once at
/// startup by `worker::spawn`; without it links are only kept in memory.
static STORE: OnceLock<Arc<mitch_lib::data::DataStore>> = OnceLock::new();

pub fn attach_store(store: Arc<mitch_lib::data::DataStore>) {
    let _ = STORE.set(store);
}

fn store() -> Option<&'static Arc<mitch_lib::data::DataStore>> {
    STORE.get()
}

/// A stored link must have at least this long left to be handed out.
const MIN_LINK_LIFE_MS: i64 = 10 * 60 * 1000;

fn from_db(id: &str) -> Option<Resolved> {
    let link = mitch_lib::music::get_stream_link(store()?, id, MIN_LINK_LIFE_MS)?;
    if !stream_host_ok(&link.url) {
        return None;
    }
    let left = (link.expires_at - mitch_lib::school::now_millis()).max(0) as u64;
    Some(Resolved {
        url: link.url,
        mime: mime_static(&link.mime),
        expires: Instant::now() + Duration::from_millis(left),
    })
}

fn mime_static(m: &str) -> &'static str {
    match m {
        "audio/webm" => "audio/webm",
        "audio/mpeg" => "audio/mpeg",
        "audio/ogg" => "audio/ogg",
        _ => "audio/mp4",
    }
}

fn to_db(id: &str, r: &Resolved) {
    if let Some(store) = store() {
        let left = r
            .expires
            .saturating_duration_since(Instant::now())
            .as_millis() as i64;
        mitch_lib::music::put_stream_link(
            store,
            id,
            &mitch_lib::music::StreamLink {
                url: r.url.clone(),
                mime: r.mime.to_string(),
                expires_at: mitch_lib::school::now_millis() + left,
            },
        );
    }
}

fn warm_limit() -> &'static Semaphore {
    static S: OnceLock<Semaphore> = OnceLock::new();
    S.get_or_init(|| Semaphore::new(2))
}

/// Resolves stream links for `ids` in the background, two at a time, so a
/// click on any of them starts at once. Interactive requests never wait on
/// this queue (it has its own limit; they only share the yt-dlp process
/// slots). Ids already cached or already being warmed are skipped, and the
/// batch stops if the source is blocked or missing rather than hammering it.
/// Warms the links for a search's results. Unlike `warm`, a newer search
/// supersedes it: the results are only resolved once the search has stood
/// still for a moment, and queued ones are dropped when another search comes
/// in, so typing doesn't pile up resolves for results nobody will see.
pub fn warm_search(ids: Vec<String>) {
    static CURRENT: OnceLock<Arc<std::sync::atomic::AtomicU64>> = OnceLock::new();
    let current = CURRENT.get_or_init(Default::default).clone();
    let mine = current.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    tokio::spawn(async move {
        tokio::time::sleep(WARM_SETTLE).await;
        if current.load(std::sync::atomic::Ordering::SeqCst) != mine {
            return;
        }
        // Resolve in order, stopping as soon as a newer search arrives.
        for id in ids {
            if current.load(std::sync::atomic::Ordering::SeqCst) != mine {
                return;
            }
            if valid_video_id(&id) && !is_cached(&id) {
                if let Err(YtError::Blocked | YtError::NotInstalled) = resolve_bg(&id).await {
                    return;
                }
            }
        }
    });
}

pub fn warm(ids: Vec<String>) {
    static IN_FLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let todo: Vec<String> = {
        let mut set = IN_FLIGHT
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut keep = Vec::new();
        for id in ids {
            if set.len() >= WARM_BACKLOG_MAX {
                break;
            }
            if valid_video_id(&id) && !is_cached(&id) && set.insert(id.clone()) {
                keep.push(id);
            }
        }
        keep
    };
    if todo.is_empty() {
        return;
    }
    tokio::spawn(async move {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut handles = Vec::new();
        for id in todo {
            let stop = stop.clone();
            handles.push(tokio::spawn(async move {
                if let Ok(_permit) = warm_limit().acquire().await {
                    if !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        if let Err(YtError::Blocked | YtError::NotInstalled) = resolve_bg(&id).await
                        {
                            stop.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                }
                if let Some(set) = IN_FLIGHT.get() {
                    set.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
                }
            }));
        }
        for h in handles {
            let _ = h.await;
        }
    });
}

/// The stream URL for a video id, from cache when fresh.
pub async fn resolve(id: &str) -> Result<Resolved, YtError> {
    resolve_timed(id).await.map(|(r, _)| r)
}

/// Like `resolve`, and says where the link came from: `memory`, `db` or
/// `yt-dlp` (the slow one). Used for the `Server-Timing` header and the log.
pub async fn resolve_timed(id: &str) -> Result<(Resolved, &'static str), YtError> {
    resolve_in(id, false).await
}

/// Resolves in the background pool: for warming, where nobody is waiting.
pub(super) async fn resolve_background(id: &str) -> Result<Resolved, YtError> {
    resolve_bg(id).await
}

async fn resolve_bg(id: &str) -> Result<Resolved, YtError> {
    resolve_in(id, true).await.map(|(r, _)| r)
}

async fn resolve_in(id: &str, background: bool) -> Result<(Resolved, &'static str), YtError> {
    if !valid_video_id(id) {
        return Err(YtError::Failed("bad id".into()));
    }
    let cached = |id: &str| {
        resolve_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .filter(|r| r.expires > Instant::now())
            .cloned()
    };
    if let Some(r) = cached(id) {
        return Ok((r, "memory"));
    }
    let lock = resolve_lock(id);
    let _guard = lock.lock().await;
    if let Some(r) = cached(id) {
        return Ok((r, "memory"));
    }
    if let Some(r) = from_db(id) {
        resolve_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string(), r.clone());
        return Ok((r, "db"));
    }
    let started = Instant::now();
    let args = [
        "--no-playlist".to_string(),
        "-f".into(),
        "bestaudio[ext=m4a]/bestaudio/best".into(),
        "--print".into(),
        "%(url)s".into(),
        "--print".into(),
        "%(ext)s".into(),
        format!("https://www.youtube.com/watch?v={id}"),
    ];
    let out = if background {
        run_bg(&args).await?
    } else {
        run(&args).await?
    };
    let resolved = parse_resolved(&out).ok_or_else(|| YtError::Failed("no audio format".into()))?;
    resolve_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id.to_string(), resolved.clone());
    to_db(id, &resolved);
    let took = started.elapsed();
    if took > Duration::from_millis(2500) {
        tracing::info!("[music] resolving {id} took {} ms", took.as_millis());
    }
    Ok((resolved, "yt-dlp"))
}

/// Resolves a well-known video once so yt-dlp's player cache is populated
/// before the first real click (the first extraction after a deploy is the
/// slowest). Logs how long it took, which is also a startup check that yt-dlp
/// works from this server.
pub async fn prime() {
    const PRIME_VIDEO: &str = "jNQXAC9IVRw";
    let started = Instant::now();
    match run(&[
        "--no-playlist".into(),
        "-f".into(),
        "bestaudio[ext=m4a]/bestaudio/best".into(),
        "--simulate".into(),
        "--print".into(),
        "%(id)s".into(),
        format!("https://www.youtube.com/watch?v={PRIME_VIDEO}"),
    ])
    .await
    {
        Ok(_) => tracing::info!(
            "[music] yt-dlp primed in {} ms",
            started.elapsed().as_millis()
        ),
        Err(e) => tracing::warn!("[music] yt-dlp priming failed: {:?}", e),
    }
}

/// Parses `--print` output (url line, then extension line). Only Google video
/// hosts are accepted, so a crafted extractor result can never turn the
/// stream proxy into an open proxy.
pub fn parse_resolved(out: &str) -> Option<Resolved> {
    let mut lines = out.lines();
    let url = lines.next()?.trim().to_string();
    let ext = lines.next().unwrap_or("m4a").trim();
    if !stream_host_ok(&url) {
        return None;
    }
    let mime = match ext {
        "m4a" | "mp4" => "audio/mp4",
        "webm" => "audio/webm",
        "mp3" => "audio/mpeg",
        "opus" | "ogg" => "audio/ogg",
        _ => "audio/mp4",
    };
    // The URL carries its own expiry (`expire=<unix seconds>`).
    let now_s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let ttl = url::Url::parse(&url)
        .ok()
        .and_then(|u| {
            u.query_pairs()
                .find(|(k, _)| k == "expire")
                .and_then(|(_, v)| v.parse::<u64>().ok())
        })
        .map(|exp| Duration::from_secs(exp.saturating_sub(now_s).saturating_sub(300)))
        .unwrap_or(RESOLVE_TTL)
        .min(RESOLVE_TTL);
    Some(Resolved {
        url,
        mime,
        expires: Instant::now() + ttl,
    })
}

/// Whether a URL is an https Google video CDN URL.
pub fn stream_host_ok(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else {
        return false;
    };
    u.scheme() == "https"
        && u.host_str()
            .is_some_and(|h| h == "googlevideo.com" || h.ends_with(".googlevideo.com"))
}

/// Hosts a redirect from the media client may lead to: Google's video CDN,
/// YouTube's thumbnail hosts and YouTube itself, over https.
pub fn media_redirect_ok(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else {
        return false;
    };
    let Some(host) = u.host_str() else {
        return false;
    };
    let under = |d: &str| host == d || host.ends_with(&format!(".{d}"));
    u.scheme() == "https"
        && (under("googlevideo.com") || under("ytimg.com") || host == "www.youtube.com")
}

/// Largest text body read from any outside service (lyrics, captions, the
/// Spotify page).
pub const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Whether a URL is YouTube's caption endpoint (the only caption host fetched).
pub fn caption_url_ok(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else {
        return false;
    };
    u.scheme() == "https" && u.host_str() == Some("www.youtube.com") && u.path() == "/api/timedtext"
}

/// A caption track as `json3`, and whether it is machine-generated.
pub struct Captions {
    pub json: serde_json::Value,
    pub auto: bool,
}

/// Fetches the best caption track for a video: a manual one in the video's
/// language if present, otherwise the original-language auto-generated one.
/// Auto-generated captions are timed per word, so they give word-by-word
/// highlighting for videos no lyrics source knows.
pub async fn captions(id: &str) -> Result<Option<Captions>, YtError> {
    captions_in(id, false).await
}

/// Captions fetched in the background pool, to check lyric timing.
pub async fn captions_bg(id: &str) -> Result<Option<Captions>, YtError> {
    captions_in(id, true).await
}

async fn captions_in(id: &str, background: bool) -> Result<Option<Captions>, YtError> {
    if !valid_video_id(id) {
        return Ok(None);
    }
    let args = [
        "--skip-download".to_string(),
        "--no-playlist".into(),
        "--dump-single-json".into(),
        format!("https://www.youtube.com/watch?v={id}"),
    ];
    let out = if background {
        run_bg(&args).await?
    } else {
        run(&args).await?
    };
    let info: serde_json::Value =
        serde_json::from_str(&out).map_err(|e| YtError::Failed(e.to_string()))?;
    let Some((url, auto)) = pick_caption_url(&info) else {
        return Ok(None);
    };
    let Some(client) = media_client() else {
        return Ok(None);
    };
    let bytes = client
        .get(&url)
        .send()
        .await
        .map_err(|e| YtError::Failed(e.to_string()))?
        .error_for_status()
        .map_err(|e| YtError::Failed(e.to_string()))?
        .bytes()
        .await
        .map_err(|e| YtError::Failed(e.to_string()))?;
    if bytes.len() > MAX_BODY_BYTES {
        return Err(YtError::Failed("caption track too large".into()));
    }
    let json = serde_json::from_slice(&bytes).map_err(|e| YtError::Failed(e.to_string()))?;
    Ok(Some(Captions { json, auto }))
}

/// Chooses a json3 caption URL from yt-dlp's info JSON. Returns (url, auto).
pub fn pick_caption_url(info: &serde_json::Value) -> Option<(String, bool)> {
    let lang = info.get("language").and_then(|l| l.as_str()).unwrap_or("");
    let json3 = |tracks: &serde_json::Value| -> Option<String> {
        tracks
            .as_array()?
            .iter()
            .find(|f| f.get("ext").and_then(|e| e.as_str()) == Some("json3"))
            .and_then(|f| f.get("url")?.as_str())
            .filter(|u| caption_url_ok(u))
            .map(str::to_string)
    };
    let find = |key: &str, want_orig: bool| -> Option<String> {
        let map = info.get(key)?.as_object()?;
        let base = lang.split('-').next().unwrap_or("");
        let order = |k: &str| -> Option<u8> {
            if want_orig && k.ends_with("-orig") {
                return Some(0);
            }
            if !want_orig && !base.is_empty() && k.split('-').next() == Some(base) {
                return Some(0);
            }
            None
        };
        let mut keys: Vec<&String> = map.keys().filter(|k| order(k).is_some()).collect();
        keys.sort();
        if keys.is_empty() && !want_orig && !map.is_empty() && base.is_empty() {
            // Unknown language: only fall back to English.
            keys = map.keys().filter(|k| k.starts_with("en")).collect();
        }
        keys.into_iter().find_map(|k| json3(&map[k]))
    };
    find("subtitles", false)
        .map(|u| (u, false))
        .or_else(|| find("automatic_captions", true).map(|u| (u, true)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_host_must_be_google_video() {
        assert!(stream_host_ok(
            "https://rr1---sn-abc.googlevideo.com/videoplayback?x=1"
        ));
        assert!(!stream_host_ok("http://rr1.googlevideo.com/videoplayback"));
        assert!(!stream_host_ok("https://evil.com/?a=googlevideo.com"));
        assert!(!stream_host_ok("https://googlevideo.com.evil.com/x"));
        assert!(!stream_host_ok("https://127.0.0.1/x"));
        assert!(caption_url_ok(
            "https://www.youtube.com/api/timedtext?v=x&fmt=json3"
        ));
        assert!(!caption_url_ok("https://www.youtube.com/watch?v=x"));
        assert!(!caption_url_ok("https://evil.com/api/timedtext"));
    }

    #[test]
    fn redirects_may_only_stay_on_known_hosts() {
        assert!(media_redirect_ok(
            "https://rr2---sn-x.googlevideo.com/videoplayback?a=1"
        ));
        assert!(media_redirect_ok("https://i.ytimg.com/vi/x/hqdefault.jpg"));
        assert!(media_redirect_ok("https://www.youtube.com/api/timedtext"));
        assert!(!media_redirect_ok("http://rr2.googlevideo.com/x"));
        assert!(!media_redirect_ok("https://127.0.0.1/x"));
        assert!(!media_redirect_ok(
            "https://169.254.169.254/latest/meta-data"
        ));
        assert!(!media_redirect_ok("https://googlevideo.com.evil.com/x"));
        assert!(!media_redirect_ok("https://evilytimg.com/x"));
    }

    #[test]
    fn parses_resolved_output() {
        let r = parse_resolved(
            "https://rr1.googlevideo.com/videoplayback?expire=99999999999&x=1\nm4a\n",
        )
        .unwrap();
        assert_eq!(r.mime, "audio/mp4");
        assert!(r.expires > Instant::now());
        assert!(parse_resolved("https://example.com/a.mp3\nmp3\n").is_none());
        assert!(parse_resolved("").is_none());
        let webm = parse_resolved("https://a.googlevideo.com/v\nwebm").unwrap();
        assert_eq!(webm.mime, "audio/webm");
    }

    #[test]
    fn parses_search_results() {
        let json = serde_json::json!({"entries":[
            {"id":"fJ9rUzIMcZQ","title":"Queen – Bohemian Rhapsody (Official Video Remastered)","channel":"Queen Official","duration":360},
            {"id":"short","title":"bad id","duration":100},
            {"id":"aaaaaaaaaaa","title":"live","duration":100,"live_status":"is_live"},
            {"id":"bbbbbbbbbbb","title":"mix","duration":7200},
            {"id":"ccccccccccc","title":"Blinding Lights","channel":"The Weeknd - Topic","duration":200.4}
        ]}).to_string();
        let t = parse_search(&json);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].artist, "Queen");
        assert_eq!(t[0].title, "Bohemian Rhapsody");
        assert_eq!(t[1].artist, "The Weeknd");
        assert_eq!(t[1].duration, 200);
        assert!(parse_search("not json").is_empty());
    }

    #[test]
    fn each_run_gets_its_own_cookie_copy_that_is_removed_after() {
        let dir = std::env::temp_dir().join(format!(
            "ytdlp-cookies-{}",
            mitch_lib::crypto::random_bytes_hex(6)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("cookies.txt");
        std::fs::write(&src, "# Netscape HTTP Cookie File\n").unwrap();
        let cache = dir.join("cache");
        let path = src.to_str().unwrap();
        let a = CookieCopy::make(Some(path), None, cache.to_str().unwrap()).unwrap();
        let b = CookieCopy::make(Some(path), None, cache.to_str().unwrap()).unwrap();
        assert_ne!(a.0, b.0, "concurrent runs must not share a file");
        assert_eq!(
            std::fs::read_to_string(&a.0).unwrap(),
            "# Netscape HTTP Cookie File\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&a.0).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "only the server's user may read it");
        }
        let kept = a.0.clone();
        drop(a);
        assert!(!kept.exists(), "the copy is removed when the run ends");
        assert!(src.exists(), "the original is never touched");
        assert!(
            CookieCopy::make(Some("/nonexistent/c.txt"), None, cache.to_str().unwrap()).is_none()
        );
        assert!(CookieCopy::make(None, None, cache.to_str().unwrap()).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cookies_can_come_from_a_secret_with_no_file_on_the_server() {
        let secret = b"# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tX\tY\n";
        let c = CookieCopy::make(None, Some(secret), "/nonexistent-fallback").unwrap();
        assert_eq!(std::fs::read(&c.0).unwrap(), secret);
        let path = c.0.clone();
        // It is a memory-backed file when the system has /dev/shm.
        if std::path::Path::new("/dev/shm").is_dir() {
            assert!(path.starts_with("/dev/shm"));
        }
        drop(c);
        assert!(!path.exists());
    }

    #[test]
    fn keeps_songs_and_drops_other_videos() {
        let t = |title: &str, duration: i64| Track {
            id: "aaaaaaaaaaa".into(),
            title: title.into(),
            artist: String::new(),
            duration,
        };
        for song in [
            t("Bohemian Rhapsody", 355),
            t("Blinding Lights (Remix)", 200),
            t("Re:ZERO Season 3 Opening", 101),
            t("Bohemian Rhapsody (Live at Wembley)", 330),
        ] {
            assert!(looks_like_song(&song), "{}", song.title);
        }
        for other in [
            t("ISEKAI HATERS FIRST TIME REACTION TO ALL OPENINGS", 300),
            t("Bohemian Rhapsody Flashmob in Paris", 358),
            t("Best of 80s mix", 300),
            t("Top 10 anime openings", 300),
            t("Re:Zero season 2 review", 400),
            t("Some song", 20),
            t("Some song", 3600),
        ] {
            assert!(!looks_like_song(&other), "{}", other.title);
        }
        // Nothing passing means the list is shown as it was.
        assert_eq!(only_songs(vec![t("a reaction", 300)]).len(), 1);
        assert_eq!(
            only_songs(vec![t("a reaction", 300), t("A song", 200)]).len(),
            1
        );
    }

    #[test]
    fn picks_caption_tracks() {
        let info = serde_json::json!({
        "language":"en",
        "subtitles":{},
        "automatic_captions":{
            "en-orig":[{"ext":"vtt","url":"https://www.youtube.com/api/timedtext?a=1&fmt=vtt"},{"ext":"json3","url":"https://www.youtube.com/api/timedtext?a=1&fmt=json3"}],
            "de":[{"ext":"json3","url":"https://www.youtube.com/api/timedtext?a=1&tlang=de&fmt=json3"}]
        }});
        let (url, auto) = pick_caption_url(&info).unwrap();
        assert!(auto && url.contains("fmt=json3") && !url.contains("tlang"));
        let manual = serde_json::json!({"language":"en","subtitles":{"en":[{"ext":"json3","url":"https://www.youtube.com/api/timedtext?m=1"}]}});
        assert!(!pick_caption_url(&manual).unwrap().1);
        let evil = serde_json::json!({"language":"en","subtitles":{"en":[{"ext":"json3","url":"https://evil.com/api/timedtext"}]}});
        assert!(pick_caption_url(&evil).is_none());
    }
}
