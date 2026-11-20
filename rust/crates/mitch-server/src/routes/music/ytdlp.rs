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
    js_runtime: Option<String>,
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn config() -> &'static Config {
    static CFG: OnceLock<Config> = OnceLock::new();
    CFG.get_or_init(|| Config {
        bin: env_nonempty("MUSIC_YTDLP_BIN").unwrap_or_else(|| "yt-dlp".to_string()),
        proxy: env_nonempty("MUSIC_EGRESS_PROXY"),
        cookies: env_nonempty("MUSIC_YTDLP_COOKIES"),
        js_runtime: env_nonempty("MUSIC_YTDLP_JS_RUNTIME"),
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
                .read_timeout(Duration::from_secs(20));
            if let Some(p) = &config().proxy {
                b = b.proxy(reqwest::Proxy::all(p).ok()?);
            }
            b.build().ok()
        })
        .as_ref()
}

fn spawn_slots() -> &'static Semaphore {
    static SLOTS: OnceLock<Semaphore> = OnceLock::new();
    SLOTS.get_or_init(|| Semaphore::new(6))
}

/// Runs yt-dlp with `args` and returns stdout. At most six run at once.
async fn run(args: &[String]) -> Result<String, YtError> {
    let cfg = config();
    let _slot = spawn_slots()
        .acquire()
        .await
        .map_err(|_| YtError::Failed("shutting down".into()))?;
    let mut cmd = Command::new(&cfg.bin);
    cmd.arg("--ignore-config")
        .arg("--no-warnings")
        .arg("--no-cache-dir")
        .arg("--socket-timeout")
        .arg("12");
    if let Some(p) = &cfg.proxy {
        cmd.arg("--proxy").arg(p);
    }
    if let Some(c) = &cfg.cookies {
        cmd.arg("--cookies").arg(c);
    }
    if let Some(js) = &cfg.js_runtime {
        cmd.arg("--js-runtimes").arg(js);
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

/// Searches YouTube. `--flat-playlist` skips per-video extraction, which is
/// what keeps a search near a second.
pub async fn search(query: &str) -> Result<Vec<Track>, YtError> {
    let key = query.trim().to_lowercase();
    if let Some((at, hit)) = search_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
    {
        if at.elapsed() < SEARCH_TTL {
            return Ok(hit.clone());
        }
    }
    let out = run(&[
        "--flat-playlist".into(),
        "--dump-single-json".into(),
        format!("ytsearch15:{}", query.trim()),
    ])
    .await?;
    let tracks = parse_search(&out);
    let mut cache = search_cache().lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() > 200 {
        cache.retain(|_, (at, _)| at.elapsed() < SEARCH_TTL);
    }
    cache.insert(key, (Instant::now(), tracks.clone()));
    Ok(tracks)
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
    out
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
pub fn warm(ids: Vec<String>) {
    static IN_FLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let todo: Vec<String> = {
        let mut set = IN_FLIGHT
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        ids.into_iter()
            .filter(|id| valid_video_id(id) && !is_cached(id) && set.insert(id.clone()))
            .collect()
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
                        if let Err(YtError::Blocked | YtError::NotInstalled) = resolve(&id).await {
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
        return Ok(r);
    }
    let lock = resolve_lock(id);
    let _guard = lock.lock().await;
    if let Some(r) = cached(id) {
        return Ok(r);
    }
    if let Some(r) = from_db(id) {
        resolve_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string(), r.clone());
        return Ok(r);
    }
    let out = run(&[
        "--no-playlist".into(),
        "-f".into(),
        "bestaudio[ext=m4a]/bestaudio/best".into(),
        "--print".into(),
        "%(url)s".into(),
        "--print".into(),
        "%(ext)s".into(),
        format!("https://www.youtube.com/watch?v={id}"),
    ])
    .await?;
    let resolved = parse_resolved(&out).ok_or_else(|| YtError::Failed("no audio format".into()))?;
    resolve_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id.to_string(), resolved.clone());
    to_db(id, &resolved);
    Ok(resolved)
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
    if !valid_video_id(id) {
        return Ok(None);
    }
    let out = run(&[
        "--skip-download".into(),
        "--no-playlist".into(),
        "--dump-single-json".into(),
        format!("https://www.youtube.com/watch?v={id}"),
    ])
    .await?;
    let info: serde_json::Value =
        serde_json::from_str(&out).map_err(|e| YtError::Failed(e.to_string()))?;
    let Some((url, auto)) = pick_caption_url(&info) else {
        return Ok(None);
    };
    let Some(client) = media_client() else {
        return Ok(None);
    };
    let body = client
        .get(&url)
        .send()
        .await
        .map_err(|e| YtError::Failed(e.to_string()))?
        .error_for_status()
        .map_err(|e| YtError::Failed(e.to_string()))?
        .text()
        .await
        .map_err(|e| YtError::Failed(e.to_string()))?;
    let json = serde_json::from_str(&body).map_err(|e| YtError::Failed(e.to_string()))?;
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
