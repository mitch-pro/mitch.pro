//! Saved songs: downloads songs people keep into the audio store, serves them
//! from there, and cleans up files nobody keeps (see `docs/music.md`).

use super::{audio_store, json_response, ytdlp};
use crate::state::AppState;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Response;
use mitch_lib::music_audio as audio;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Bytes asked from YouTube per request.
const CHUNK: u64 = 4 * 1024 * 1024;
/// A file bigger than this is not saved (a long mix or livestream).
const MAX_FILE_BYTES: u64 = 80 * 1024 * 1024;
const IDLE_WAIT: Duration = Duration::from_secs(30);
const BETWEEN_SONGS: Duration = Duration::from_secs(4);
const BLOCKED_WAIT: Duration = Duration::from_secs(15 * 60);
const SONGS_PER_PASS: usize = 3;

/// Starts the background saver. Call once at startup.
pub fn spawn(state: Arc<AppState>) {
    let store = audio_store::init(&state.cfg.data_dir);
    // Scratch files left by a crash.
    let _ = std::fs::remove_dir_all(tmp_dir(&state));
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(90)).await;
        loop {
            collect_garbage(&state, store).await;
            convert_existing(&state, store).await;
            let work = audio::next_to_save(&state.store, SONGS_PER_PASS);
            if work.is_empty() {
                tokio::time::sleep(IDLE_WAIT).await;
                continue;
            }
            for (id, _) in work {
                let claimed = in_flight()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(id.clone());
                if !claimed {
                    continue;
                }
                let outcome = save_one(&state, store, &id).await;
                in_flight()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                match outcome {
                    Outcome::Saved | Outcome::Skipped => {}
                    Outcome::SourceUnavailable => {
                        tokio::time::sleep(BLOCKED_WAIT).await;
                        break;
                    }
                }
                tokio::time::sleep(BETWEEN_SONGS).await;
            }
        }
    });
}

/// Songs being saved right now, so the loop and an urgent request don't both
/// download one.
fn in_flight() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static SET: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    SET.get_or_init(Default::default)
}

/// Starts saving a song at once if it should be saved (it is coming up in
/// someone's queue). Does nothing for songs nobody keeps, or already saved.
pub fn save_soon(state: &Arc<AppState>, id: &str) {
    let Some(store) = audio_store::get() else {
        return;
    };
    if !audio::wants_save(&state.store, id) {
        return;
    }
    let claimed = in_flight()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id.to_string());
    if !claimed {
        return;
    }
    let state = state.clone();
    let id = id.to_string();
    tokio::spawn(async move {
        // At most two urgent saves at once, so a burst of requests can't start
        // dozens of downloads.
        static URGENT: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
        let slots = URGENT.get_or_init(|| tokio::sync::Semaphore::new(2));
        let _permit = slots.acquire().await;
        save_one(&state, store, &id).await;
        in_flight()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
    });
}

enum Outcome {
    Saved,
    /// Failed or did not fit; recorded, so it is retried later.
    Skipped,
    /// yt-dlp is missing or YouTube blocks the server.
    SourceUnavailable,
}

fn tmp_dir(state: &AppState) -> PathBuf {
    state.cfg.data_dir.join("music-audio-tmp")
}

async fn save_one(state: &Arc<AppState>, store: &audio_store::AudioStore, id: &str) -> Outcome {
    let tmp = tmp_dir(state).join(format!("{id}.part"));
    let outcome = match download(id, &tmp).await {
        Err(ytdlp::YtError::Blocked | ytdlp::YtError::NotInstalled) => Outcome::SourceUnavailable,
        Err(e) => {
            tracing::info!("[music] could not save {id}: {}", e.public_message());
            audio::record_failure(&state.store, id);
            Outcome::Skipped
        }
        Ok((size, mime)) => {
            // YouTube's audio is a fragmented MP4 (index spread through the
            // file). Rewrite it as a plain MP4 with the index first, which
            // iPhones start playing at once; keep the original if that fails.
            let fast = tmp.with_extension("fast");
            let (path, size, progressive) =
                if mime == "audio/mp4" && make_progressive(&tmp, &fast).await {
                    let len = tokio::fs::metadata(&fast)
                        .await
                        .map(|m| m.len())
                        .unwrap_or(0);
                    if len > 0 {
                        (fast.clone(), len, true)
                    } else {
                        (tmp.clone(), size, false)
                    }
                } else {
                    (tmp.clone(), size, false)
                };
            // Only keep it if someone who keeps this song still has room.
            let result =
                if !audio::has_room(&state.store, id, i64::try_from(size).unwrap_or(i64::MAX)) {
                    audio::record_failure(&state.store, id);
                    Outcome::Skipped
                } else {
                    let ext = audio_store::ext_for(mime);
                    match store.put(&audio_store::key(id, ext), &path).await {
                        Ok(()) => {
                            audio::put_file(
                                &state.store,
                                &audio::AudioFile {
                                    video_id: id.to_string(),
                                    size: i64::try_from(size).unwrap_or(i64::MAX),
                                    mime: mime.to_string(),
                                    ext: ext.to_string(),
                                },
                            );
                            if progressive {
                                audio::mark_progressive(&state.store, id);
                            }
                            Outcome::Saved
                        }
                        Err(e) => {
                            tracing::warn!("[music] audio store refused {id}: {e}");
                            audio::record_failure(&state.store, id);
                            Outcome::Skipped
                        }
                    }
                };
            let _ = tokio::fs::remove_file(&fast).await;
            result
        }
    };
    let _ = tokio::fs::remove_file(&tmp).await;
    outcome
}

/// Rewrites `src` (YouTube's fragmented MP4 audio) as a plain MP4 at `dst` with
/// the index at the front, without re-encoding (about a second for a song).
/// iPhones need the whole index up front to start a song at once and to know
/// its length; a fragmented file makes them read on before playing. `false`
/// when ffmpeg is missing or fails, in which case the caller keeps `src`.
async fn make_progressive(src: &std::path::Path, dst: &std::path::Path) -> bool {
    let run = tokio::process::Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-y", "-i"])
        .arg(src)
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-c",
            "copy",
            "-movflags",
            "+faststart",
            "-f",
            "mp4",
        ])
        .arg(dst)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .status();
    let ok = matches!(
        tokio::time::timeout(Duration::from_secs(90), run).await,
        Ok(Ok(status)) if status.success()
    );
    if !ok {
        let _ = tokio::fs::remove_file(dst).await;
    }
    ok
}

/// Rewrites saved songs that predate `make_progressive` (a couple per pass).
/// A song that can't be rewritten is marked done anyway, so it isn't retried
/// forever. The file is replaced under the same name; a listener who is
/// mid-song at that instant may hiccup once.
async fn convert_existing(state: &Arc<AppState>, store: &audio_store::AudioStore) {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;
    for f in audio::needing_progressive(&state.store, 2) {
        let key = audio_store::key(&f.video_id, &f.ext);
        let raw = tmp_dir(state).join(format!("{}.old", f.video_id));
        let fast = tmp_dir(state).join(format!("{}.fast", f.video_id));
        let _ = tokio::fs::create_dir_all(tmp_dir(state)).await;
        let size = u64::try_from(f.size).unwrap_or(0);
        let fetched = async {
            let read = store.read(&key, 0, size.saturating_sub(1)).await.ok()?;
            let mut out = tokio::fs::File::create(&raw).await.ok()?;
            let mut body = read.body.into_data_stream();
            while let Some(chunk) = body.next().await {
                out.write_all(&chunk.ok()?).await.ok()?;
            }
            out.flush().await.ok()
        }
        .await
        .is_some();
        if fetched && size > 0 && make_progressive(&raw, &fast).await {
            let len = tokio::fs::metadata(&fast)
                .await
                .map(|m| m.len())
                .unwrap_or(0);
            if len > 0 && store.put(&key, &fast).await.is_ok() {
                audio::put_file(
                    &state.store,
                    &audio::AudioFile {
                        size: i64::try_from(len).unwrap_or(i64::MAX),
                        ..f.clone()
                    },
                );
            }
        }
        audio::mark_progressive(&state.store, &f.video_id);
        let _ = tokio::fs::remove_file(&raw).await;
        let _ = tokio::fs::remove_file(&fast).await;
    }
}

/// Downloads the song's audio to `dest` and returns its size and MIME type.
async fn download(id: &str, dest: &std::path::Path) -> Result<(u64, &'static str), ytdlp::YtError> {
    use tokio::io::AsyncWriteExt;
    let fail = |m: &str| ytdlp::YtError::Failed(m.to_string());
    let client = ytdlp::media_client().ok_or(ytdlp::YtError::NotInstalled)?;
    if let Some(dir) = dest.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|_| fail("no scratch space"))?;
    }
    let mut retried = false;
    'attempt: loop {
        let resolved = ytdlp::resolve_background(id).await?;
        let mut file = tokio::fs::File::create(dest)
            .await
            .map_err(|_| fail("no scratch space"))?;
        let mut start: u64 = 0;
        loop {
            let resp = client
                .get(&resolved.url)
                .header(
                    header::RANGE,
                    format!("bytes={start}-{}", start + CHUNK - 1),
                )
                .send()
                .await
                .map_err(|_| fail("download failed"))?;
            let status = resp.status();
            if (status == StatusCode::FORBIDDEN || status == StatusCode::GONE) && !retried {
                retried = true;
                ytdlp::forget(id);
                continue 'attempt;
            }
            if !status.is_success() {
                return Err(fail("download refused"));
            }
            let total = resp
                .headers()
                .get(header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.rsplit('/').next()?.trim().parse::<u64>().ok());
            let whole = status == StatusCode::OK;
            let mut resp = resp;
            let mut got: u64 = 0;
            while let Some(bytes) = resp.chunk().await.map_err(|_| fail("download cut off"))? {
                got += bytes.len() as u64;
                if start + got > MAX_FILE_BYTES {
                    return Err(fail("too large"));
                }
                file.write_all(&bytes)
                    .await
                    .map_err(|_| fail("no scratch space"))?;
            }
            start += got;
            let done = whole || got == 0 || total.is_some_and(|t| start >= t);
            if done {
                file.flush().await.map_err(|_| fail("no scratch space"))?;
                return Ok((start, resolved.mime));
            }
        }
    }
}

/// Deletes saved files nobody keeps any more.
async fn collect_garbage(state: &Arc<AppState>, store: &audio_store::AudioStore) {
    for f in audio::orphan_files(&state.store, 25) {
        if store
            .delete(&audio_store::key(&f.video_id, &f.ext))
            .await
            .is_ok()
        {
            audio::delete_file(&state.store, &f.video_id);
        }
    }
    audio::prune_failures(&state.store);
}

/// Serves a saved song, honouring a single `Range`. `None` when the song isn't
/// saved or the store can't be read right now (the caller then streams live).
pub async fn serve(state: &Arc<AppState>, id: &str, headers: &HeaderMap) -> Option<Response> {
    let file = audio::get_file(&state.store, id)?;
    let store = audio_store::get()?;
    let size = u64::try_from(file.size).ok().filter(|s| *s > 0)?;
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|r| r.trim().strip_prefix("bytes="))
        .and_then(|spec| spec.split_once('-'))
        .and_then(|(a, b)| {
            let start = a.trim().parse::<u64>().ok()?;
            let end = match b.trim() {
                "" => size - 1,
                e => e.parse::<u64>().ok()?.min(size - 1),
            };
            Some((start, end))
        });
    let (start, end, partial) = match range {
        Some((s, e)) if s <= e && s < size => (s, e, true),
        Some(_) => {
            return Some(
                Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{size}"))
                    .body(axum::body::Body::empty())
                    .unwrap_or_default(),
            )
        }
        None => (0, size - 1, false),
    };
    let read = match store
        .read(&audio_store::key(id, &file.ext), start, end)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            if e.contains("No such file") || e.contains("os error 2") {
                // The file is gone from the folder: forget it so it is saved again.
                audio::delete_file(&state.store, id);
            }
            return None;
        }
    };
    let mut out = Response::builder()
        .status(if partial {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_TYPE, file.mime)
        .header(header::CONTENT_LENGTH, read.length)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "private, max-age=3600")
        .header("x-content-type-options", "nosniff")
        .header("server-timing", "resolve;desc=\"saved\";dur=0");
    if partial {
        out = out.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
    }
    out.body(read.body).ok()
}

/// `GET /api/music/storage`: how much saved audio the caller keeps.
pub fn summary(state: &Arc<AppState>, email: &str) -> Response {
    let (songs, saved) = audio::counts(&state.store, email);
    json_response(
        200,
        json!({
            "usedBytes": audio::used_bytes(&state.store, email),
            "limitBytes": audio::USER_QUOTA_BYTES,
            "songs": songs,
            "saved": saved,
            "backend": audio_store::get().map(|s| s.name()).unwrap_or("local"),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Top-level MP4 box names, in order.
    fn boxes(data: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        let mut i = 0;
        while i + 8 <= data.len() {
            let size =
                u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
            out.push(String::from_utf8_lossy(&data[i + 4..i + 8]).to_string());
            if size == 0 {
                break;
            }
            i += size;
        }
        out
    }

    #[tokio::test]
    async fn a_fragmented_m4a_becomes_a_plain_mp4_with_the_index_first() {
        let dir = std::env::temp_dir().join(format!("mitch-prog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (src, dst) = (dir.join("frag.m4a"), dir.join("plain.m4a"));
        let made = std::process::Command::new("ffmpeg")
            .args([
                "-nostdin", "-v", "error", "-y", "-f", "lavfi", "-i", "sine=d=5", "-c:a", "aac",
            ])
            .args([
                "-movflags",
                "frag_keyframe+empty_moov+default_base_moof+dash",
            ])
            .arg(&src)
            .status();
        if !matches!(made, Ok(s) if s.success()) {
            return; // no ffmpeg on this machine
        }
        assert!(boxes(&std::fs::read(&src).unwrap()).contains(&"moof".to_string()));
        assert!(make_progressive(&src, &dst).await);
        let plain = boxes(&std::fs::read(&dst).unwrap());
        assert!(!plain.contains(&"moof".to_string()), "{plain:?}");
        let moov = plain.iter().position(|b| b == "moov").unwrap();
        let mdat = plain.iter().position(|b| b == "mdat").unwrap();
        assert!(moov < mdat, "index must come first: {plain:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
