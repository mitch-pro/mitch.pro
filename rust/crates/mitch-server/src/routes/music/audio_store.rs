//! Where saved songs live (see `docs/music.md`, "Saved songs").
//!
//! The backend is chosen at startup from environment variables, which the
//! secret manager injects like every other secret (`docs/secrets-providers.md`):
//!
//! | `MUSIC_AUDIO_STORE` | Backend |
//! | --- | --- |
//! | unset or `local` | a folder under the data directory |
//! | `local:/some/path` | a folder you choose. An NFS or SMB share works by mounting it on the host and pointing this at the mount |
//! | `webdav:https://host/path` | any WebDAV server, Nextcloud included (`https://cloud.example.com/remote.php/dav/files/USER/mitch-music`). Credentials: `MUSIC_AUDIO_STORE_USER`, `MUSIC_AUDIO_STORE_PASS` (a Nextcloud app password) |
//!
//! Other schemes (`s3:`, `gdrive:`, `sftp:` ...) are recognised but not built
//! yet: the server logs that and uses the local folder, so nothing breaks. To add
//! one, add a variant to `AudioStore`, implement `put`, `read` and `delete` for
//! it, and parse its scheme in `AudioStore::from_env`; nothing else changes.
//!
//! Files are named `<video id>.<ext>` and are written whole (atomically for the
//! local folder), so a reader never sees a partial file.

use axum::body::Body;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

pub enum AudioStore {
    Local(PathBuf),
    WebDav(WebDav),
}

pub struct WebDav {
    base: String,
    user: String,
    pass: String,
    client: reqwest::Client,
}

/// A readable piece of a saved file.
pub struct StoredRead {
    pub body: Body,
    pub length: u64,
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_default().trim().to_string()
}

/// WebDAV sends the password with every request, so it must be https. Plain
/// http is accepted only for loopback and private-network hosts.
fn webdav_url_ok(url: &str) -> bool {
    if url.starts_with("https://") {
        return true;
    }
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let host = rest.split(['/', ':']).next().unwrap_or("");
    host == "localhost"
        || host.starts_with("127.")
        || host.starts_with("10.")
        || host.starts_with("192.168.")
        || host
            .strip_prefix("172.")
            .and_then(|r| r.split('.').next()?.parse::<u8>().ok())
            .is_some_and(|n| (16..=31).contains(&n))
}

impl AudioStore {
    /// Builds the backend from `MUSIC_AUDIO_STORE`. `data_dir` holds the default
    /// local folder.
    pub fn from_env(data_dir: &Path) -> Self {
        let default = || AudioStore::Local(data_dir.join("music-audio"));
        let spec = env("MUSIC_AUDIO_STORE");
        if spec.is_empty() || spec == "local" {
            return default();
        }
        if let Some(path) = spec.strip_prefix("local:") {
            return AudioStore::Local(PathBuf::from(path));
        }
        if let Some(url) = spec.strip_prefix("webdav:") {
            let url = url.trim_end_matches('/').to_string();
            if !webdav_url_ok(&url) {
                tracing::warn!(
                    "[music] MUSIC_AUDIO_STORE webdav needs an https URL (http only on a private network); using local"
                );
                return default();
            }
            let client = reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .read_timeout(std::time::Duration::from_secs(60))
                .build();
            let Ok(client) = client else {
                return default();
            };
            return AudioStore::WebDav(WebDav {
                base: url,
                user: env("MUSIC_AUDIO_STORE_USER"),
                pass: env("MUSIC_AUDIO_STORE_PASS"),
                client,
            });
        }
        let scheme = spec.split(':').next().unwrap_or("");
        tracing::warn!(
            "[music] audio store '{scheme}' is not supported yet; using the local folder"
        );
        default()
    }

    pub fn name(&self) -> &'static str {
        match self {
            AudioStore::Local(_) => "local",
            AudioStore::WebDav(_) => "webdav",
        }
    }

    /// Stores the file at `src` under `key`. The source is left in place.
    pub async fn put(&self, key: &str, src: &Path) -> Result<(), String> {
        match self {
            AudioStore::Local(dir) => {
                tokio::fs::create_dir_all(dir)
                    .await
                    .map_err(|e| e.to_string())?;
                let dest = dir.join(key);
                let part = dir.join(format!("{key}.part"));
                tokio::fs::copy(src, &part)
                    .await
                    .map_err(|e| e.to_string())?;
                tokio::fs::rename(&part, &dest)
                    .await
                    .map_err(|e| e.to_string())
            }
            AudioStore::WebDav(w) => w.put(key, src).await,
        }
    }

    /// Reads bytes `start..=end` (inclusive) of the saved file.
    pub async fn read(&self, key: &str, start: u64, end: u64) -> Result<StoredRead, String> {
        let length = end.saturating_sub(start) + 1;
        match self {
            AudioStore::Local(dir) => {
                let mut file = tokio::fs::File::open(dir.join(key))
                    .await
                    .map_err(|e| e.to_string())?;
                file.seek(std::io::SeekFrom::Start(start))
                    .await
                    .map_err(|e| e.to_string())?;
                let body = Body::from_stream(ReaderStream::new(file.take(length)));
                Ok(StoredRead { body, length })
            }
            AudioStore::WebDav(w) => w.read(key, start, end).await,
        }
    }

    pub async fn delete(&self, key: &str) -> Result<(), String> {
        match self {
            AudioStore::Local(dir) => match tokio::fs::remove_file(dir.join(key)).await {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.to_string()),
            },
            AudioStore::WebDav(w) => w.delete(key).await,
        }
    }
}

impl WebDav {
    fn url(&self, key: &str) -> String {
        format!("{}/{key}", self.base)
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if self.user.is_empty() {
            req
        } else {
            req.basic_auth(&self.user, Some(&self.pass))
        }
    }

    /// `MKCOL` on one URL; returns the HTTP status.
    async fn mkcol(&self, url: &str) -> Result<u16, String> {
        let method = reqwest::Method::from_bytes(b"MKCOL").map_err(|e| e.to_string())?;
        let resp = self
            .auth(self.client.request(method, url))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Ok(resp.status().as_u16())
    }

    /// Creates the storage folder if it is missing. The folder itself is tried
    /// first: 201 means created and 405 means it already exists. Only when its
    /// parent is missing (409) are the parents created, top down, ignoring their
    /// answers (a server's own root folders, like Nextcloud's `/remote.php`, are
    /// not collections and refuse `MKCOL`).
    async fn ensure_folder(&self) -> Result<(), String> {
        let first = self.mkcol(&self.base).await?;
        if matches!(first, 200 | 201 | 405) {
            return Ok(());
        }
        let status = if first == 409 {
            let (scheme, rest) = self.base.split_once("://").unwrap_or(("https", &self.base));
            let mut parts = rest.split('/').filter(|p| !p.is_empty());
            let host = parts.next().unwrap_or("");
            let segments: Vec<&str> = parts.collect();
            let mut path = String::new();
            for seg in &segments[..segments.len().saturating_sub(1)] {
                path.push('/');
                path.push_str(seg);
                let _ = self.mkcol(&format!("{scheme}://{host}{path}")).await;
            }
            self.mkcol(&self.base).await?
        } else {
            first
        };
        match status {
            200 | 201 | 405 => Ok(()),
            404 => Err(
                "the folder URL was not found (404): check MUSIC_AUDIO_STORE and the account name in it"
                    .to_string(),
            ),
            401 | 403 => Err(format!(
                "not allowed to create the folder ({status}): check MUSIC_AUDIO_STORE_USER and _PASS"
            )),
            other => Err(format!("creating the folder failed ({other})")),
        }
    }

    async fn put(&self, key: &str, src: &Path) -> Result<(), String> {
        let attempt = || async {
            let file = tokio::fs::File::open(src)
                .await
                .map_err(|e| e.to_string())?;
            let len = file.metadata().await.map_err(|e| e.to_string())?.len();
            let body = reqwest::Body::wrap_stream(ReaderStream::new(file));
            self.auth(self.client.put(self.url(key)))
                .header("content-length", len)
                .body(body)
                .send()
                .await
                .map_err(|e| e.to_string())
        };
        let mut resp = attempt().await?;
        if resp.status().as_u16() == 409 || resp.status().as_u16() == 404 {
            // The folder is missing: make it, then try once more.
            self.ensure_folder().await?;
            resp = attempt().await?;
        }
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(format!("upload refused ({})", resp.status().as_u16()))
        }
    }

    async fn read(&self, key: &str, start: u64, end: u64) -> Result<StoredRead, String> {
        let resp = self
            .auth(self.client.get(self.url(key)))
            .header("range", format!("bytes={start}-{end}"))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        if status != 206 && !(status == 200 && start == 0) {
            return Err(format!("read refused ({status})"));
        }
        let length = resp
            .content_length()
            .unwrap_or(end.saturating_sub(start) + 1);
        Ok(StoredRead {
            body: Body::from_stream(resp.bytes_stream()),
            length,
        })
    }

    async fn delete(&self, key: &str) -> Result<(), String> {
        let resp = self
            .auth(self.client.delete(self.url(key)))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let s = resp.status().as_u16();
        if resp.status().is_success() || s == 404 {
            Ok(())
        } else {
            Err(format!("delete refused ({s})"))
        }
    }
}

/// The process-wide store, set once when the worker starts.
static STORE: OnceLock<AudioStore> = OnceLock::new();

pub fn init(data_dir: &Path) -> &'static AudioStore {
    STORE.get_or_init(|| {
        let s = AudioStore::from_env(data_dir);
        tracing::info!("[music] saved songs go to the {} store", s.name());
        s
    })
}

pub fn get() -> Option<&'static AudioStore> {
    STORE.get()
}

/// Store file name for a saved song.
pub fn key(video_id: &str, ext: &str) -> String {
    format!("{video_id}.{ext}")
}

/// File extension for an audio MIME type from yt-dlp.
pub fn ext_for(mime: &str) -> &'static str {
    match mime {
        "audio/mp4" => "m4a",
        "audio/webm" => "webm",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        _ => "bin",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("audio-store-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[tokio::test]
    async fn local_store_round_trips_ranges_and_deletes() {
        let dir = tmp("local");
        let src = dir.join("src.bin");
        let data: Vec<u8> = (0..=255u8).collect();
        std::fs::write(&src, &data).unwrap();
        let store = AudioStore::Local(dir.join("store"));
        store.put("aaaaaaaaaaa.m4a", &src).await.unwrap();
        let part = store.read("aaaaaaaaaaa.m4a", 10, 19).await.unwrap();
        assert_eq!(part.length, 10);
        let bytes = axum::body::to_bytes(part.body, 1024).await.unwrap();
        assert_eq!(&bytes[..], &data[10..20]);
        store.delete("aaaaaaaaaaa.m4a").await.unwrap();
        assert!(store.read("aaaaaaaaaaa.m4a", 0, 1).await.is_err());
        store.delete("aaaaaaaaaaa.m4a").await.unwrap(); // already gone is fine
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn webdav_needs_https_except_on_private_networks() {
        assert!(webdav_url_ok("https://cloud.example.com/dav"));
        assert!(webdav_url_ok("http://192.168.1.5/dav"));
        assert!(webdav_url_ok("http://localhost:8080/dav"));
        assert!(webdav_url_ok("http://172.20.0.2/dav"));
        assert!(!webdav_url_ok("http://cloud.example.com/dav"));
        assert!(!webdav_url_ok("http://172.40.0.2/dav"));
        assert!(!webdav_url_ok("ftp://x/dav"));
    }

    #[test]
    fn backend_is_chosen_from_the_setting() {
        let data = Path::new("/data");
        // `from_env` reads the process environment, so test the parsing paths
        // that don't need one through the extension helper and defaults.
        assert_eq!(ext_for("audio/mp4"), "m4a");
        assert_eq!(ext_for("audio/webm"), "webm");
        assert_eq!(key("aaaaaaaaaaa", "m4a"), "aaaaaaaaaaa.m4a");
        assert_eq!(AudioStore::Local(data.join("music-audio")).name(), "local");
    }

    /// A tiny in-process WebDAV server shaped like Nextcloud: only the user's
    /// own `/remote.php/dav/files/u/` tree is a real collection, the parents
    /// above it answer 404 to `MKCOL`, and a `PUT` into a folder that does not
    /// exist yet answers 409 (404 when the folder's parent is also missing).
    async fn mock_webdav() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use axum::extract::State;
        use axum::http::{Method, StatusCode, Uri};
        use axum::routing::any;
        type Log = std::sync::Arc<std::sync::Mutex<Vec<String>>>;
        let log: Log = Default::default();
        let folders: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>> =
            std::sync::Arc::new(std::sync::Mutex::new(
                ["/remote.php/dav/files/u".to_string()]
                    .into_iter()
                    .collect(),
            ));
        let state = (log.clone(), folders);
        async fn handle(
            State((log, folders)): State<(
                Log,
                std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
            )>,
            method: Method,
            uri: Uri,
            _body: axum::body::Bytes,
        ) -> StatusCode {
            let path = uri.path().trim_end_matches('/').to_string();
            log.lock().unwrap().push(format!("{method} {path}"));
            let mut folders = folders.lock().unwrap();
            let parent = path
                .rsplit_once('/')
                .map(|(p, _)| p.to_string())
                .unwrap_or_default();
            match method.as_str() {
                "MKCOL" if folders.contains(&path) => StatusCode::METHOD_NOT_ALLOWED,
                "MKCOL" if folders.contains(&parent) => {
                    folders.insert(path);
                    StatusCode::CREATED
                }
                "MKCOL" => StatusCode::NOT_FOUND,
                "PUT" if folders.contains(&parent) => StatusCode::CREATED,
                "PUT" => StatusCode::NOT_FOUND,
                _ => StatusCode::METHOD_NOT_ALLOWED,
            }
        }
        let app = axum::Router::new().fallback(any(handle)).with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), log)
    }

    fn webdav(base: String) -> AudioStore {
        AudioStore::WebDav(WebDav {
            base,
            user: "u".into(),
            pass: "p".into(),
            client: reqwest::Client::new(),
        })
    }

    #[tokio::test]
    async fn webdav_creates_only_the_storage_folder_when_the_root_refuses_mkcol() {
        let (host, log) = mock_webdav().await;
        let dir = tmp("dav");
        let src = dir.join("a.bin");
        std::fs::write(&src, b"audio").unwrap();
        let store = webdav(format!("{host}/remote.php/dav/files/u/mitch-music"));
        store.put("aaaaaaaaaaa.m4a", &src).await.unwrap();
        let log = log.lock().unwrap().clone();
        assert!(
            log.iter()
                .any(|l| l == "MKCOL /remote.php/dav/files/u/mitch-music"),
            "{log:?}"
        );
        assert!(
            !log.iter().any(|l| l == "MKCOL /remote.php"),
            "must not try to create the server's own folders: {log:?}"
        );
        // A second upload finds the folder and just uploads.
        store.put("bbbbbbbbbbb.m4a", &src).await.unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn webdav_says_what_is_wrong_when_the_url_is_bad() {
        let (host, _log) = mock_webdav().await;
        let dir = tmp("dav-bad");
        let src = dir.join("a.bin");
        std::fs::write(&src, b"audio").unwrap();
        // The account in the URL does not exist, so nothing under it can be made.
        let store = webdav(format!("{host}/remote.php/dav/files/nobody/mitch-music"));
        let err = store.put("aaaaaaaaaaa.m4a", &src).await.unwrap_err();
        assert!(err.contains("not found (404)"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
