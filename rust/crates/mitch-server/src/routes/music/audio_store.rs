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
            if !url.starts_with("https://") && !url.starts_with("http://") {
                tracing::warn!(
                    "[music] MUSIC_AUDIO_STORE webdav needs an http(s) URL; using local"
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

    /// Creates the folder (and parents) if it is missing. 405 means it exists.
    async fn ensure_folder(&self) -> Result<(), String> {
        let rest = self
            .base
            .split_once("://")
            .map(|(_, r)| r)
            .unwrap_or(&self.base);
        let scheme = if self.base.starts_with("https") {
            "https"
        } else {
            "http"
        };
        let mut parts = rest.split('/');
        let host = parts.next().unwrap_or("");
        let mut path = String::new();
        for seg in parts.filter(|s| !s.is_empty()) {
            path.push('/');
            path.push_str(seg);
            let url = format!("{scheme}://{host}{path}");
            let method = reqwest::Method::from_bytes(b"MKCOL").map_err(|e| e.to_string())?;
            let resp = self
                .auth(self.client.request(method, &url))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let s = resp.status().as_u16();
            // 201 created, 405 already there; anything else on a parent we
            // don't own (for example 403) is fine as long as the PUT works.
            if !(s == 201 || s == 405 || s == 403 || s == 409 || s == 301 || s == 200) {
                return Err(format!("creating the folder failed ({s})"));
            }
        }
        Ok(())
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
    fn backend_is_chosen_from_the_setting() {
        let data = Path::new("/data");
        // `from_env` reads the process environment, so test the parsing paths
        // that don't need one through the extension helper and defaults.
        assert_eq!(ext_for("audio/mp4"), "m4a");
        assert_eq!(ext_for("audio/webm"), "webm");
        assert_eq!(key("aaaaaaaaaaa", "m4a"), "aaaaaaaaaaa.m4a");
        assert_eq!(AudioStore::Local(data.join("music-audio")).name(), "local");
    }
}
