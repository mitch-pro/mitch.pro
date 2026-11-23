//! Static file serving — byte-faithful port of `serveStatic()` (server.js
//! ~7448-7596) with the in-RAM cache. Important parity facts:
//! - Bun sets NO etag/Last-Modified/Accept-Ranges and handles no conditional
//!   or range requests. Only 200 + redirects + 404/403.
//! - In practice every requestable file goes through the cached branch (the
//!   cache loads on miss), so the cached-branch mime map is the contract.
//! - The site's own files (`webserver/`) are read into memory once, when the server
//!   starts, and served from that snapshot for the life of the process (`webfs`).
//!   They are never re-read from disk, so a deploy that replaces the files under a
//!   running server does not change what it serves: the old server keeps serving
//!   the old frontend until it is replaced. Only folders the server itself writes
//!   into (`LIVE_DIRS`: uploads, backgrounds) are read from disk as before.
//! - Cache-Control classes: code (html/htm/js/css or text types) →
//!   `no-cache, no-store, must-revalidate` + Pragma + Expires 0; media exts →
//!   `public, max-age=31536000, immutable`; else `public, max-age=2592000`.
#![allow(clippy::expect_used)] // infallible static responses

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::{header, HeaderName};
use axum::response::Response;

use crate::errors::err_resp;

pub const STATIC_CACHE_REVALIDATE_MS: u128 = 30 * 60 * 1000;
pub const SITE_TOUR_PATHS: &[&str] = &[
    "/media/site-tour-v1.mp4",
    "/media/site-tour-v2.mp4",
    "/media/site-tour-v3.mp4",
    "/media/site-tour-v4.mp4",
];

/// Single byte ranges for the public tour; unsupported range units and
/// multipart requests are ignored, while unsatisfiable byte ranges are 416.
fn video_byte_range(raw: &str, size: usize) -> Result<Option<(usize, usize)>, ()> {
    let Some(bytes) = raw.strip_prefix("bytes=") else {
        return Ok(None);
    };
    if bytes.contains(',') {
        return Ok(None);
    }
    let (first, last) = bytes.split_once('-').ok_or(())?;
    if size == 0 {
        return Err(());
    }
    if first.is_empty() {
        let suffix = last.parse::<usize>().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        return Ok(Some((size.saturating_sub(suffix), size - 1)));
    }
    let start = first.parse::<usize>().map_err(|_| ())?;
    let end = if last.is_empty() {
        size - 1
    } else {
        last.parse::<usize>().map_err(|_| ())?.min(size - 1)
    };
    if start >= size || start > end {
        return Err(());
    }
    Ok(Some((start, end)))
}

/// Only the explicitly public tour uses this range-aware path. Other static
/// files and their access policies retain the existing behavior.
pub fn serve_tour_video(
    cache: &StaticCache,
    webroot: &Path,
    path: &str,
    headers: &axum::http::HeaderMap,
    head: bool,
) -> Response {
    let Some(file) = safe_webroot_path(webroot, path) else {
        return err_resp(403, None, None);
    };
    let Some(entry) = cache.get(&file) else {
        return err_resp(404, None, None);
    };
    let size = entry.size;
    let range = if head {
        Ok(None)
    } else {
        headers
            .get(header::RANGE)
            .and_then(|v| v.to_str().ok())
            .map(|v| video_byte_range(v, size))
            .unwrap_or(Ok(None))
    };
    let mut response = Response::builder()
        .header(header::CONTENT_TYPE, "video/mp4")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable");
    match range {
        Err(()) => response
            .status(416)
            .header(header::CONTENT_RANGE, format!("bytes */{size}"))
            .header(header::CONTENT_LENGTH, "0")
            .body(axum::body::Body::empty())
            .expect("video range response"),
        Ok(Some((start, end))) => response
            .status(206)
            .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"))
            .header(header::CONTENT_LENGTH, (end - start + 1).to_string())
            .body(axum::body::Body::from(entry.data[start..=end].to_vec()))
            .expect("video response"),
        Ok(None) => {
            response = response
                .status(200)
                .header(header::CONTENT_LENGTH, size.to_string());
            let body = if head {
                axum::body::Body::empty()
            } else {
                axum::body::Body::from(entry.data.as_ref().clone())
            };
            response.body(body).expect("video response")
        }
    }
}

/// Default ceiling for the in-RAM cache (16 GiB in bun; configurable because
/// the dev box is not the VPS). Env: MITCH_STATIC_CACHE_MB.
fn cache_max_bytes() -> u64 {
    std::env::var("MITCH_STATIC_CACHE_MB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(|mb| mb * 1024 * 1024)
        .unwrap_or(4 * 1024 * 1024 * 1024)
}

/// Folders under the webroot that the server or an admin writes into while it
/// runs. They are not part of the frontend, so they are read from disk when
/// requested.
pub const LIVE_DIRS: &[&str] = &["backgrounds", "blog/uploads"];

/// The frontend, loaded into memory once at startup and never refreshed from
/// disk (see the module docs). Files and directories are answered from the
/// snapshot, so "does this page exist" and "read this file" cannot disagree with
/// what was loaded. A path that is outside the webroot, inside a live folder, or
/// too big to keep in memory falls back to the disk.
pub mod webfs {
    use super::{cache_max_bytes, normalize_lexical, LIVE_DIRS};
    use std::collections::{HashMap, HashSet};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, RwLock};

    /// A single file bigger than this stays on disk.
    const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;

    struct Snapshot {
        root: PathBuf,
        files: HashMap<PathBuf, Arc<Vec<u8>>>,
        dirs: HashSet<PathBuf>,
        /// Files that exist but were not loaded (over budget): read from disk.
        on_disk: HashSet<PathBuf>,
    }

    static SNAPSHOT: RwLock<Option<Arc<Snapshot>>> = RwLock::new(None);

    /// What a path resolves to.
    pub enum Found {
        /// The file, from memory.
        File(Arc<Vec<u8>>),
        /// A directory in the snapshot.
        Dir,
        /// Not in the snapshot, and the snapshot covers this path: it does not
        /// exist.
        Missing,
        /// Not covered by the snapshot: ask the disk.
        Disk,
    }

    fn is_live(root: &Path, path: &Path) -> bool {
        LIVE_DIRS.iter().any(|d| path.starts_with(root.join(d)))
    }

    fn is_code(path: &Path) -> bool {
        matches!(
            path.extension().and_then(|e| e.to_str()).unwrap_or(""),
            "html" | "htm" | "js" | "css" | "json" | "txt" | "xml" | "svg" | "wasm"
        )
    }

    fn walk(
        dir: &Path,
        root: &Path,
        depth: u32,
        files: &mut Vec<(PathBuf, u64)>,
        dirs: &mut HashSet<PathBuf>,
    ) {
        if depth > 32 || is_live(root, dir) {
            return;
        }
        dirs.insert(dir.to_path_buf());
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // `metadata` follows symlinks, so a linked file or folder counts.
            let Ok(md) = std::fs::metadata(&path) else {
                continue;
            };
            if md.is_dir() {
                walk(&path, root, depth + 1, files, dirs);
            } else if md.is_file() {
                files.push((path, md.len()));
            }
        }
    }

    fn build(root: &Path) -> Snapshot {
        let mut found: Vec<(PathBuf, u64)> = Vec::new();
        let mut dirs = HashSet::new();
        walk(root, root, 0, &mut found, &mut dirs);
        // Code first, then everything else smallest first, until the budget is
        // used up; the rest stays on disk.
        found.sort_by_key(|(p, len)| (!is_code(p), *len));
        let budget = cache_max_bytes();
        let mut used: u64 = 0;
        let mut files = HashMap::new();
        let mut on_disk = HashSet::new();
        for (path, len) in found {
            if len > MAX_FILE_BYTES || used + len > budget {
                on_disk.insert(path);
                continue;
            }
            match std::fs::read(&path) {
                Ok(data) => {
                    used += data.len() as u64;
                    files.insert(path, Arc::new(data));
                }
                Err(_) => {
                    on_disk.insert(path);
                }
            }
        }
        if !on_disk.is_empty() {
            tracing::warn!(
                "[static] {} files were too big for the in-memory frontend and are read from disk",
                on_disk.len()
            );
        }
        tracing::info!(
            "[static] frontend loaded into memory: {} files, {} MB",
            files.len(),
            used / (1024 * 1024)
        );
        Snapshot {
            root: root.to_path_buf(),
            files,
            dirs,
            on_disk,
        }
    }

    /// `MITCH_STATIC_LIVE=1` (development): no snapshot. Every file is read from
    /// disk on every request so edits show up on the next reload.
    pub fn live() -> bool {
        matches!(
            std::env::var("MITCH_STATIC_LIVE")
                .unwrap_or_default()
                .trim()
                .to_lowercase()
                .as_str(),
            "1" | "true" | "yes" | "on"
        )
    }

    /// Loads the frontend from `root`. Call once at startup, before serving. In
    /// live mode (see [`live`]) nothing is loaded.
    pub fn init(root: &Path) -> (usize, usize) {
        if live() {
            tracing::warn!("[static] MITCH_STATIC_LIVE: serving the frontend straight from disk");
            return (0, 0);
        }
        reload(root)
    }

    /// Replaces the snapshot with what is on disk now. Used at startup and by the
    /// admin "refresh static cache" action; nothing else calls it.
    pub fn reload(root: &Path) -> (usize, usize) {
        if live() {
            return (0, 0);
        }
        let snap = Arc::new(build(root));
        let counts = (snap.files.len(), snap.dirs.len());
        *SNAPSHOT.write().unwrap_or_else(|e| e.into_inner()) = Some(snap);
        counts
    }

    /// Number of files held in memory.
    pub fn file_count() -> usize {
        SNAPSHOT
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|s| s.files.len())
            .unwrap_or(0)
    }

    pub fn lookup(path: &Path) -> Found {
        let Some(snap) = SNAPSHOT.read().unwrap_or_else(|e| e.into_inner()).clone() else {
            return Found::Disk;
        };
        let path = normalize_lexical(path);
        if !path.starts_with(&snap.root) || is_live(&snap.root, &path) {
            return Found::Disk;
        }
        if let Some(data) = snap.files.get(&path) {
            return Found::File(data.clone());
        }
        if snap.on_disk.contains(&path) {
            return Found::Disk;
        }
        if snap.dirs.contains(&path) {
            return Found::Dir;
        }
        Found::Missing
    }

    /// File contents, from memory (or the disk for the exceptions above).
    pub fn read(path: &Path) -> Option<Arc<Vec<u8>>> {
        match lookup(path) {
            Found::File(d) => Some(d),
            Found::Disk => std::fs::read(path).ok().map(Arc::new),
            Found::Dir | Found::Missing => None,
        }
    }

    pub fn read_text(path: &Path) -> Option<String> {
        read(path).map(|d| String::from_utf8_lossy(&d).into_owned())
    }

    pub fn is_file(path: &Path) -> bool {
        match lookup(path) {
            Found::File(_) => true,
            Found::Disk => path.is_file(),
            Found::Dir | Found::Missing => false,
        }
    }

    pub fn is_dir(path: &Path) -> bool {
        match lookup(path) {
            Found::Dir => true,
            Found::Disk => path.is_dir(),
            Found::File(_) | Found::Missing => false,
        }
    }

    pub fn exists(path: &Path) -> bool {
        is_file(path) || is_dir(path)
    }

    /// Names of the directories directly inside `dir`.
    pub fn child_dir_names(dir: &Path) -> Vec<String> {
        let snapshot = SNAPSHOT.read().unwrap_or_else(|e| e.into_inner()).clone();
        let dir = normalize_lexical(dir);
        match snapshot {
            Some(snap) if matches!(lookup(&dir), Found::Dir) => snap
                .dirs
                .iter()
                .filter(|d| d.parent() == Some(dir.as_path()))
                .filter_map(|d| d.file_name().and_then(|n| n.to_str()).map(str::to_string))
                .collect(),
            _ => std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .filter_map(|e| e.file_name().into_string().ok())
                .collect(),
        }
    }
}

#[derive(Clone)]
struct CacheEntry {
    data: Arc<Vec<u8>>,
    mtime_ms: u128,
    size: usize,
    checked_at: u128,
}

#[derive(Default)]
struct Inner {
    map: HashMap<PathBuf, CacheEntry>,
    bytes: usize,
    /// Bun fakes LRU by delete+set on access, so insertion order is recency
    /// order; we evict the first key when over budget.
    order: Vec<PathBuf>,
}

#[derive(Clone)]
pub struct StaticCache {
    inner: Arc<Mutex<Inner>>,
    max_bytes: u64,
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

impl StaticCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
            max_bytes: cache_max_bytes(),
        }
    }

    /// `staticCacheGet` for what the snapshot does not hold (live folders, files
    /// over the memory budget): touch, revalidate against mtime+size every 30
    /// min, load on miss (like `staticCacheLoad`).
    fn get(&self, path: &Path) -> Option<CacheEntry> {
        // The frontend comes from the startup snapshot and is never revalidated.
        match webfs::lookup(path) {
            webfs::Found::File(data) => {
                return Some(CacheEntry {
                    size: data.len(),
                    data,
                    mtime_ms: 0,
                    checked_at: now_ms(),
                });
            }
            webfs::Found::Dir | webfs::Found::Missing => return None,
            webfs::Found::Disk => {}
        }
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = inner.map.get(path).cloned() {
            inner.order.retain(|k| k != path);
            inner.order.push(path.to_path_buf());
            if !webfs::live()
                && now_ms().saturating_sub(entry.checked_at) < STATIC_CACHE_REVALIDATE_MS
            {
                return Some(entry);
            }
            match std::fs::metadata(path) {
                Ok(md) if md.len() as usize == entry.size && mtime_of(&md) == entry.mtime_ms => {
                    let refreshed = CacheEntry {
                        checked_at: now_ms(),
                        ..entry
                    };
                    inner.map.insert(path.to_path_buf(), refreshed.clone());
                    return Some(refreshed);
                }
                _ => {
                    if let Some(old) = inner.map.remove(path) {
                        inner.bytes -= old.size;
                    }
                    inner.order.retain(|k| k != path);
                }
            }
        }
        let Ok(md) = std::fs::metadata(path) else {
            inner.map.remove(path);
            return None;
        };
        if !md.is_file() || md.len() > self.max_bytes {
            return None;
        }
        let Ok(data) = std::fs::read(path) else {
            inner.map.remove(path);
            return None;
        };
        let size = data.len();
        let entry = CacheEntry {
            data: Arc::new(data),
            mtime_ms: mtime_of(&md),
            size,
            checked_at: now_ms(),
        };
        inner.map.insert(path.to_path_buf(), entry.clone());
        inner.order.push(path.to_path_buf());
        inner.bytes += size;
        while inner.bytes as u64 > self.max_bytes {
            let Some(oldest) = inner.order.first().cloned() else {
                break;
            };
            if let Some(evicted) = inner.map.remove(&oldest) {
                inner.bytes -= evicted.size;
            }
            inner.order.remove(0);
        }
        Some(entry)
    }

    pub fn stats(&self) -> (usize, usize, u64) {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        (inner.map.len(), inner.bytes, self.max_bytes)
    }

    /// The admin "refresh static cache" action: the one deliberate way to re-read
    /// the frontend from disk while the server runs. It rebuilds the whole
    /// snapshot (whatever files were named) and empties the cache of live folders.
    pub fn clear(
        &self,
        webroot: &Path,
        _base_dir: &Path,
        specific_files: &[String],
    ) -> ClearResult {
        let before = webfs::file_count();
        let (after, _) = webfs::reload(webroot);
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.map.clear();
        inner.order.clear();
        inner.bytes = 0;
        ClearResult {
            evicted: before,
            reloaded: after,
            full: specific_files.is_empty(),
        }
    }
}

pub struct ClearResult {
    pub evicted: usize,
    pub reloaded: usize,
    pub full: bool,
}

fn mtime_of(md: &std::fs::Metadata) -> u128 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Lexically normalize `.`/`..` components, like `path.resolve`.
fn normalize_lexical(path: &std::path::Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// `safeWebrootPath(relativePath)`: lexical containment inside the webroot
/// (resolve semantics: `..` is collapsed before the prefix check).
pub fn safe_webroot_path(webroot: &std::path::Path, relative: &str) -> Option<PathBuf> {
    let cleaned = relative.trim_start_matches('/');
    let full = normalize_lexical(&webroot.join(cleaned));
    if full != webroot && !full.starts_with(webroot) {
        return None;
    }
    Some(full)
}

/// Cached-branch mime map, verbatim from the JS.
fn mime_type(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "js" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "html" => "text/html; charset=utf-8",
        "htm" => "text/html; charset=utf-8",
        "png" => "image/png",
        "jpg" => "image/jpeg",
        "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "json" => "application/json; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml; charset=utf-8",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        _ => return None,
    })
}

fn is_hashed_asset(url_path: &str) -> bool {
    url_path.starts_with("/matrix/assets/")
        || url_path.starts_with("/matrix/public/element-call/assets/")
        || {
            let filename = url_path.rsplit('/').next().unwrap_or("");
            if let Some(dash) = filename.rfind('-') {
                let rest = &filename[dash + 1..];
                if let Some(dot) = rest.find('.') {
                    let hash = &rest[..dot];
                    let ext = &rest[dot + 1..];
                    hash.len() >= 8
                        && hash
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                        && matches!(
                            ext.to_ascii_lowercase().as_str(),
                            "js" | "css" | "wasm" | "woff" | "woff2" | "ttf" | "png" | "svg"
                        )
                } else {
                    false
                }
            } else {
                false
            }
        }
}

fn cache_control_for(ext: &str, content_type: &str, url_path: &str) -> Vec<(HeaderName, String)> {
    if is_hashed_asset(url_path) && !content_type.contains("text/html") {
        return vec![(
            header::CACHE_CONTROL,
            "public, max-age=31536000, immutable".to_string(),
        )];
    }
    // json: static data files fetched by client JS (e.g. game-portal's
    // catalog.json) need the same freshness guarantee as code — a 30-day
    // default here meant updating one required visitors to hard-reload,
    // since a normal reload never even asked the server.
    let is_code = matches!(ext, "html" | "htm" | "js" | "css" | "json")
        || content_type.contains("text/html")
        || content_type.contains("javascript")
        || content_type.contains("css")
        || content_type.contains("application/json");
    if is_code {
        return vec![
            (
                header::CACHE_CONTROL,
                "no-cache, no-store, must-revalidate".to_string(),
            ),
            (header::PRAGMA, "no-cache".to_string()),
            (header::EXPIRES, "0".to_string()),
        ];
    }
    let media = matches!(
        ext,
        "png"
            | "jpg"
            | "jpeg"
            | "webp"
            | "gif"
            | "avif"
            | "svg"
            | "ico"
            | "mp4"
            | "webm"
            | "mp3"
            | "wav"
            | "woff"
            | "woff2"
            | "ttf"
            | "otf"
    );
    if media {
        return vec![(
            header::CACHE_CONTROL,
            "public, max-age=31536000, immutable".to_string(),
        )];
    }
    vec![(header::CACHE_CONTROL, "public, max-age=2592000".to_string())]
}

/// `Response.redirect(target, 30x)`.
pub fn redirect(target: &str, status: u16) -> Response {
    Response::builder()
        .status(axum::http::StatusCode::from_u16(status).unwrap_or(axum::http::StatusCode::FOUND))
        .header(header::LOCATION, target)
        .body(axum::body::Body::empty())
        .expect("static response")
}

/// `serveStatic(urlPath)` — static serving with directory index handling.
/// HTML bodies pass through `transform_html` (readability + broadcast), like
/// the JS.
pub fn serve_static(
    cache: &StaticCache,
    webroot: &std::path::Path,
    url_path: &str,
    req_headers: Option<&axum::http::HeaderMap>,
    transform_html: impl Fn(String) -> String,
) -> Response {
    let Some(mut file_path) = safe_webroot_path(webroot, url_path) else {
        return err_resp(403, None, None);
    };

    // Directory: check for index.html, then index.htm.
    if webfs::is_dir(&file_path) {
        if !url_path.ends_with('/') {
            return redirect(&format!("{url_path}/"), 301);
        }
        if webfs::exists(&file_path.join("index.html")) {
            file_path = file_path.join("index.html");
        } else if webfs::exists(&file_path.join("index.htm")) {
            let target = format!("{url_path}index.htm");
            return redirect(&target, 302);
        }
    }

    let ext = file_path
        .to_string_lossy()
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase();

    let accepts_gzip = req_headers
        .and_then(|h| h.get(axum::http::header::ACCEPT_ENCODING))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.contains("gzip"))
        .unwrap_or(false);
    let gz_path = PathBuf::from(format!("{}.gz", file_path.display()));
    let can_serve_gzip = accepts_gzip && ext != "html" && ext != "htm" && webfs::exists(&gz_path);
    let target_file_path = if can_serve_gzip { gz_path } else { file_path };

    let Some(entry) = cache.get(&target_file_path) else {
        return err_resp(404, None, None);
    };

    let content_type = mime_type(&ext)
        .unwrap_or("application/octet-stream")
        .to_string();
    let mut headers: Vec<(HeaderName, String)> = vec![(header::CONTENT_TYPE, content_type.clone())];
    if can_serve_gzip {
        headers.push((axum::http::header::CONTENT_ENCODING, "gzip".to_string()));
        headers.push((axum::http::header::VARY, "Accept-Encoding".to_string()));
    }
    headers.extend(cache_control_for(&ext, &content_type, url_path));

    let body: axum::body::Body = if content_type.contains("text/html") {
        let html = transform_html(String::from_utf8_lossy(&entry.data).into_owned());
        axum::body::Body::from(html)
    } else {
        axum::body::Body::from(entry.data.as_ref().clone())
    };
    let mut builder = Response::builder().status(axum::http::StatusCode::OK);
    for (k, v) in &headers {
        builder = builder.header(k.clone(), v);
    }
    builder.body(body).expect("static response")
}

/// Pickle asset shortcut (`Cache-Control: public, max-age=86400`, small MIME
/// set) — server.js ~21044-21056.
pub fn pickle_asset_response(webroot: &std::path::Path, path: &str) -> Option<Response> {
    let rel = path.trim_start_matches('/');
    let asset = safe_webroot_path(webroot, &format!("sexypickleclub/{rel}"))?;
    if !webfs::is_file(&asset) {
        return None;
    }
    let ext = asset
        .to_string_lossy()
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" => "image/jpeg",
        "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    };
    Some(
        Response::builder()
            .status(axum::http::StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            .header(header::CACHE_CONTROL, "public, max-age=86400")
            .body(axum::body::Body::from(
                webfs::read(&asset)?.as_ref().clone(),
            ))
            .expect("static response"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_ranges_cover_browser_seeking_and_invalid_offsets() {
        assert_eq!(video_byte_range("bytes=0-1", 100), Ok(Some((0, 1))));
        assert_eq!(video_byte_range("bytes=75-", 100), Ok(Some((75, 99))));
        assert_eq!(video_byte_range("bytes=-25", 100), Ok(Some((75, 99))));
        assert_eq!(video_byte_range("bytes=-200", 100), Ok(Some((0, 99))));
        assert_eq!(video_byte_range("bytes=90-200", 100), Ok(Some((90, 99))));
        for invalid in ["bytes=100-", "bytes=5-3", "bytes=-0", "bytes=nope-2"] {
            assert_eq!(video_byte_range(invalid, 100), Err(()));
        }
        assert_eq!(video_byte_range("bytes=0-", 0), Err(()));
        assert_eq!(video_byte_range("items=0-1", 100), Ok(None));
        assert_eq!(video_byte_range("bytes=0-1,4-5", 100), Ok(None));
    }

    #[test]
    fn safe_webroot_blocks_traversal() {
        let root = std::path::Path::new("/srv/webroot");
        assert!(safe_webroot_path(root, "/index.html").is_some());
        assert!(safe_webroot_path(root, "index.html").is_some());
        // Lexical containment, like the JS (no percent-decoding).
        assert_eq!(safe_webroot_path(root, "../../etc/passwd"), None);
    }

    #[test]
    fn safe_webroot_path_contains_public_overlay_requests() {
        // handler.rs resolves webserver/public/<path> the same way, with
        // `webroot.join("public")` standing in for the real webroot — a
        // request still can't escape that subdirectory via traversal.
        let public_root = std::path::Path::new("/srv/webroot/public");
        assert_eq!(
            safe_webroot_path(public_root, "/foo.html"),
            Some(std::path::PathBuf::from("/srv/webroot/public/foo.html"))
        );
        assert_eq!(safe_webroot_path(public_root, "../index.html"), None);
        assert_eq!(safe_webroot_path(public_root, "../../etc/passwd"), None);
    }

    #[test]
    fn cache_control_classes_match_js() {
        assert!(cache_control_for(
            "html",
            "text/html; charset=utf-8",
            "/matrix/assets/old-page.html"
        )[0]
        .1
        .contains("no-store"));

        assert_eq!(
            cache_control_for("html", "text/html; charset=utf-8", "/index.html")[0].1,
            "no-cache, no-store, must-revalidate"
        );
        assert_eq!(
            cache_control_for("png", "image/png", "/img.png")[0].1,
            "public, max-age=31536000, immutable"
        );
        assert_eq!(
            cache_control_for("bin", "application/octet-stream", "/data.bin")[0].1,
            "public, max-age=2592000"
        );
        // A client-fetched data file (e.g. game-portal/catalog.json) must
        // revalidate like code does, not cache for 30 days — otherwise a
        // normal reload never even asks the server about an update.
        assert_eq!(
            cache_control_for("json", "application/json", "/game-portal/catalog.json")[0].1,
            "no-cache, no-store, must-revalidate"
        );
    }
}
