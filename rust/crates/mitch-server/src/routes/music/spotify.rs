//! Reading a public Spotify playlist for import and sync.
//!
//! No Spotify account, app or API key is involved. Spotify's public embed page
//! for a playlist (`open.spotify.com/embed/playlist/<id>`) carries the
//! playlist's name and track list as JSON; that is all we read. Limits that
//! follow from this, and which the UI states:
//! - only **public** playlists (a private one answers 404);
//! - the embed lists at most the first 100 tracks;
//! - no audio: tracks are matched to YouTube at play time (`match_track`).
//!
//! Spotify can change the embed page; `parse_embed` is the one place to fix.
//! Only the fixed host `open.spotify.com` is ever fetched.

use mitch_lib::music_playlists::PlaylistTrack;
use std::sync::OnceLock;
use std::time::Duration;

pub struct SpotifyPlaylist {
    pub name: String,
    pub tracks: Vec<PlaylistTrack>,
}

/// A 22-character base62 playlist id from a share URL, a `spotify:playlist:`
/// URI, or a bare id.
pub fn parse_playlist_id(input: &str) -> Option<String> {
    let input = input.trim();
    let id_ok = |s: &str| s.len() == 22 && s.bytes().all(|b| b.is_ascii_alphanumeric());
    if let Some(rest) = input.strip_prefix("spotify:playlist:") {
        return id_ok(rest).then(|| rest.to_string());
    }
    if id_ok(input) {
        return Some(input.to_string());
    }
    let url = url::Url::parse(input).ok()?;
    if url.scheme() != "https" || url.host_str() != Some("open.spotify.com") {
        return None;
    }
    let mut segs = url.path_segments()?;
    while let Some(s) = segs.next() {
        if s == "playlist" {
            let id = segs.next()?;
            return id_ok(id).then(|| id.to_string());
        }
    }
    None
}

/// Drops a trailing " [From ...]" or " - From ..." style note.
fn clean_title(t: &str) -> String {
    let t = t.trim();
    match t.rfind(" [") {
        Some(i) if t.ends_with(']') && t[i..].to_lowercase().starts_with(" [from") => {
            t[..i].trim().to_string()
        }
        _ => t.to_string(),
    }
}

/// Why an import failed. The first three are about the link, so they answer 400
/// with a message that says what to fix; only `Unavailable` (Spotify itself
/// failing) is a 502.
#[derive(Debug, PartialEq, Eq)]
pub enum ImportError {
    /// Spotify has no public playlist at this link: it is private, deleted, or
    /// not a real id.
    NotPublic,
    /// The link is to something else (an album, an artist).
    NotAPlaylist,
    /// A public playlist with nothing in it we can read.
    Empty,
    /// Spotify didn't answer or answered something unusable.
    Unavailable,
}

impl ImportError {
    pub fn status(&self) -> u16 {
        match self {
            ImportError::Unavailable => 502,
            _ => 400,
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            ImportError::NotPublic => {
                "Invalid link, or the playlist isn't public. Only public Spotify playlists can be imported."
            }
            ImportError::NotAPlaylist => "That isn't a link to a Spotify playlist.",
            ImportError::Empty => "That playlist has no songs Spotify shares publicly.",
            ImportError::Unavailable => "Couldn't reach Spotify. Try again in a moment.",
        }
    }
}

pub fn parse_embed(html: &str) -> Result<SpotifyPlaylist, ImportError> {
    // A page without the playlist data is what Spotify serves for a private or
    // missing playlist (it would also be what a changed page looks like, which
    // the server log line in `fetch` is there to reveal).
    const FAIL: ImportError = ImportError::NotPublic;
    let start = html
        .find("id=\"__NEXT_DATA__\"")
        .and_then(|i| html[i..].find('>').map(|j| i + j + 1))
        .ok_or(FAIL)?;
    let end = html[start..].find("</script>").ok_or(FAIL)? + start;
    let data: serde_json::Value = serde_json::from_str(&html[start..end]).map_err(|_| FAIL)?;
    let entity = data
        .pointer("/props/pageProps/state/data/entity")
        .ok_or(FAIL)?;
    if entity.get("type").and_then(|t| t.as_str()) != Some("playlist") {
        return Err(ImportError::NotAPlaylist);
    }
    let name = entity
        .get("name")
        .or_else(|| entity.get("title"))
        .and_then(|n| n.as_str())
        .unwrap_or("Spotify playlist")
        .to_string();
    let mut tracks = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in entity
        .get("trackList")
        .and_then(|t| t.as_array())
        .into_iter()
        .flatten()
    {
        let Some(sid) = item
            .get("uri")
            .and_then(|u| u.as_str())
            .and_then(|u| u.strip_prefix("spotify:track:"))
        else {
            continue; // episodes and local files
        };
        if !seen.insert(sid.to_string()) {
            continue;
        }
        let title = clean_title(item.get("title").and_then(|t| t.as_str()).unwrap_or(""));
        if title.is_empty() {
            continue;
        }
        let artist = item
            .get("subtitle")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .replace('\u{a0}', " ");
        let ms = item.get("duration").and_then(|d| d.as_i64()).unwrap_or(0);
        tracks.push(PlaylistTrack {
            id: String::new(),
            title,
            artist,
            duration: (ms + 500) / 1000,
            spotify_id: sid.to_string(),
        });
    }
    if tracks.is_empty() {
        return Err(ImportError::Empty);
    }
    Ok(SpotifyPlaylist { name, tracks })
}

fn client() -> Option<&'static reqwest::Client> {
    static C: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(super::ytdlp::DESKTOP_UA)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(12))
            .build()
            .ok()
    })
    .as_ref()
}

/// Fetches a public playlist.
pub async fn fetch(id: &str) -> Result<SpotifyPlaylist, ImportError> {
    let c = client().ok_or(ImportError::Unavailable)?;
    let resp = c
        .get(format!("https://open.spotify.com/embed/playlist/{id}"))
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
        .map_err(|_| ImportError::Unavailable)?;
    let status = resp.status().as_u16();
    if status == 404 {
        return Err(ImportError::NotPublic);
    }
    if !resp.status().is_success() {
        tracing::warn!("[music] Spotify embed answered {status} for a playlist import");
        return Err(ImportError::Unavailable);
    }
    let bytes = resp.bytes().await.map_err(|_| ImportError::Unavailable)?;
    if bytes.len() > super::ytdlp::MAX_BODY_BYTES {
        return Err(ImportError::Unavailable);
    }
    let parsed = parse_embed(&String::from_utf8_lossy(&bytes));
    if parsed.as_ref().err() == Some(&ImportError::NotPublic) {
        // Normal for a private playlist; if Spotify changed its page this is
        // where it would show up, so leave a trace for the owner.
        tracing::info!(
            "[music] Spotify embed had no playlist data ({} bytes): private playlist, or the page changed",
            bytes.len()
        );
    }
    parsed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_playlist_ids() {
        let id = "37i9dQZF1DXcBWIGoYBM5M";
        assert_eq!(
            parse_playlist_id(&format!("https://open.spotify.com/playlist/{id}?si=abc")).as_deref(),
            Some(id)
        );
        assert_eq!(
            parse_playlist_id(&format!("https://open.spotify.com/intl-de/playlist/{id}"))
                .as_deref(),
            Some(id)
        );
        assert_eq!(
            parse_playlist_id(&format!("spotify:playlist:{id}")).as_deref(),
            Some(id)
        );
        assert_eq!(parse_playlist_id(id).as_deref(), Some(id));
        assert!(parse_playlist_id("https://evil.com/playlist/37i9dQZF1DXcBWIGoYBM5M").is_none());
        assert!(
            parse_playlist_id("http://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M").is_none()
        );
        assert!(
            parse_playlist_id("https://open.spotify.com/album/37i9dQZF1DXcBWIGoYBM5M").is_none()
        );
        assert!(parse_playlist_id("short").is_none());
    }

    #[test]
    fn link_problems_answer_400_and_say_what_to_fix() {
        for e in [
            ImportError::NotPublic,
            ImportError::NotAPlaylist,
            ImportError::Empty,
        ] {
            assert_eq!(
                e.status(),
                400,
                "{e:?} is about the link, not a server failure"
            );
        }
        assert_eq!(ImportError::Unavailable.status(), 502);
        let m = ImportError::NotPublic.message();
        assert!(m.starts_with("Invalid link") && m.contains("public"), "{m}");
    }

    #[test]
    fn parses_embed_page() {
        let data = serde_json::json!({"props":{"pageProps":{"state":{"data":{"entity":{
        "type":"playlist","name":"Mix",
        "trackList":[
            {"uri":"spotify:track:AAA","title":"Flower [From GTAVI: Original Score]","subtitle":"Justice,\u{a0}SZA","duration":224877},
            {"uri":"spotify:track:AAA","title":"dup","subtitle":"x","duration":1},
            {"uri":"spotify:episode:EEE","title":"podcast","subtitle":"x","duration":1},
            {"uri":"spotify:track:BBB","title":"Second","subtitle":"Someone","duration":180000}
        ]}}}}}});
        let html = format!(
            "<html><script id=\"__NEXT_DATA__\" type=\"application/json\">{data}</script></html>"
        );
        let p = parse_embed(&html).unwrap();
        assert_eq!(p.name, "Mix");
        assert_eq!(p.tracks.len(), 2);
        assert_eq!(p.tracks[0].title, "Flower");
        assert_eq!(p.tracks[0].artist, "Justice, SZA");
        assert_eq!(p.tracks[0].duration, 225);
        assert_eq!(p.tracks[0].spotify_id, "AAA");
        assert!(p.tracks[0].id.is_empty());
        assert_eq!(
            parse_embed("<html></html>").err(),
            Some(ImportError::NotPublic)
        );
        let album = html.replace("\"playlist\"", "\"album\"");
        assert_eq!(parse_embed(&album).err(), Some(ImportError::NotAPlaylist));
    }
}
