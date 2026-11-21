//! Playlist endpoints under `/api/music/` (see `docs/music.md`).
//!
//! | endpoint | |
//! | --- | --- |
//! | `GET playlists` | the caller's playlists |
//! | `GET playlists/public?q=` | public playlists, newest first |
//! | `GET playlist?id=&t=` | one playlist with tracks; `t` is a share token |
//! | `POST playlists/create`, `playlist/update`, `playlist/delete` | owner only |
//! | `POST playlist/tracks/add`, `.../remove`, `.../move` | owner only |
//! | `POST playlist/share` `{id, enable}` | owner only: create or revoke the share link |
//! | `POST playlist/copy` `{id, t}` | copy anything the caller can view into their own private list |
//! | `POST playlist/spotify/import`, `.../sync` | see `spotify.rs` |
//! | `GET match?title=&artist=&duration=` | YouTube match for a Spotify track |
//!
//! A playlist the caller can't open answers 404, the same as a missing one,
//! so ids can't be probed. The owner's email is never returned, only their
//! display name, photo and equipped cosmetics.

use super::{allow, limited, qs, ytdlp};
use crate::routes::me::{json_response, parse_body_strict};
use crate::state::AppState;
use axum::http::Method;
use axum::response::Response;
use mitch_lib::music_lyrics as ml;
use mitch_lib::music_playlists::{self as pl, Playlist, PlaylistTrack};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

const MAX_ADD_PER_CALL: usize = 100;
/// A Spotify-linked playlist is refreshed automatically by the page when it
/// is opened and older than this.
const SPOTIFY_AUTO_SYNC_MS: i64 = 24 * 3600 * 1000;

fn not_found() -> Response {
    json_response(404, json!({ "error": "playlist not found" }))
}
fn bad(msg: &str) -> Response {
    json_response(400, json!({ "error": msg }))
}

/// Name, photo and cosmetics of a playlist owner. `with_data_photo` also
/// allows an uploaded (data URL) photo, which is too big for list responses.
fn owner_identity(state: &AppState, email: &str, with_data_photo: bool) -> Value {
    let norm = mitch_lib::auth::normalize_email(email);
    let prof = mitch_lib::profiles::profile_get_opt(&state.store, &norm).unwrap_or(json!({}));
    let pfp_raw = prof.get("pfp").and_then(|p| p.as_str()).unwrap_or("");
    let pfp =
        mitch_lib::profile::sanitize_profile_image_url(pfp_raw, with_data_photo, 1000, 120_000);
    let cosm = mitch_lib::shop::cosmetics_get(&state.store, &norm).unwrap_or(json!({}));
    json!({
        "name": crate::routes::companion::display_name_for(state, email),
        "pfp": pfp,
        "color": mitch_lib::shop::public_active_color(&state.store, &norm, cosm.get("activeColor").unwrap_or(&Value::Null)),
        "badge": cosm.get("activeBadge").cloned().unwrap_or(Value::Null),
    })
}

fn summary(state: &AppState, p: &Playlist, count: i64, viewer: &str, full_identity: bool) -> Value {
    json!({
        "id": p.id,
        "name": p.name,
        "public": p.public,
        "trackCount": count,
        "mine": pl::is_owner(p, viewer),
        "spotify": !p.spotify_id.is_empty(),
        "spotifySyncedAt": p.spotify_synced_at,
        "covers": pl::cover_ids(&state.store, &p.id, 4),
        "updatedAt": p.updated_at,
        "owner": owner_identity(state, &p.owner_email, full_identity),
    })
}

fn parse_tracks(v: &Value) -> Vec<PlaylistTrack> {
    v.get("tracks")
        .and_then(|t| t.as_array())
        .map(|a| {
            a.iter()
                .take(MAX_ADD_PER_CALL)
                .map(|t| PlaylistTrack {
                    id: t
                        .get("id")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    title: t
                        .get("title")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    artist: t
                        .get("artist")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    duration: t.get("duration").and_then(|x| x.as_i64()).unwrap_or(0),
                    spotify_id: t
                        .get("spotifyId")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

/// The playlist if `email` owns it, else the error response to send.
#[allow(clippy::result_large_err)]
fn owned(state: &AppState, email: &str, id: &str) -> Result<Playlist, Response> {
    match pl::get(&state.store, id) {
        Some(p) if pl::is_owner(&p, email) => Ok(p),
        Some(_) => Err(json_response(
            403,
            json!({ "error": "that isn't your playlist" }),
        )),
        None => Err(not_found()),
    }
}

pub async fn handle(
    state: &Arc<AppState>,
    email: &str,
    method: &Method,
    route: &str,
    body: &[u8],
    search: &str,
) -> Option<Response> {
    let get = *method == Method::GET;
    let post = *method == Method::POST;
    if get {
        return Some(match route {
            "playlists" => my_playlists(state, email),
            "playlists/public" => public_playlists(state, email, search),
            "playlist" => one_playlist(state, email, search),
            "match" => match_track(state, email, search).await,
            _ => return None,
        });
    }
    if !post {
        return None;
    }
    let v = match parse_body_strict(body) {
        Some(v) => v,
        None => return Some(bad("bad json")),
    };
    let id = text(&v, "id");
    Some(match route {
        "playlists/create" => create(state, email, &v),
        "playlist/update" => update(state, email, &id, &v),
        "playlist/delete" => match owned(state, email, &id) {
            Ok(p) => {
                pl::delete(&state.store, &p.id);
                json_response(200, json!({ "ok": true }))
            }
            Err(r) => r,
        },
        "playlist/tracks/add" => match owned(state, email, &id) {
            Ok(p) => match pl::add_tracks(&state.store, &p.id, parse_tracks(&v)) {
                Ok(n) => {
                    super::worker::warm_playlist(state, &p.id);
                    json_response(200, json!({ "added": n }))
                }
                Err(m) => bad(m),
            },
            Err(r) => r,
        },
        "playlist/tracks/remove" => match owned(state, email, &id) {
            Ok(p) => result(pl::remove_track(&state.store, &p.id, index(&v, "position"))),
            Err(r) => r,
        },
        "playlist/tracks/move" => match owned(state, email, &id) {
            Ok(p) => result(pl::move_track(
                &state.store,
                &p.id,
                index(&v, "from"),
                index(&v, "to"),
            )),
            Err(r) => r,
        },
        "playlist/share" => match owned(state, email, &id) {
            Ok(p) => {
                let enable = v.get("enable").and_then(|e| e.as_bool()).unwrap_or(false);
                let token = pl::set_share(&state.store, &p.id, enable);
                json_response(200, json!({ "shareToken": token }))
            }
            Err(r) => r,
        },
        "playlist/copy" => copy(state, email, &id, &text(&v, "t")),
        "playlist/spotify/import" => spotify_import(state, email, &v).await,
        "playlist/spotify/sync" => match owned(state, email, &id) {
            Ok(p) => spotify_sync(state, email, &p).await,
            Err(r) => r,
        },
        _ => return None,
    })
}

fn index(v: &Value, key: &str) -> usize {
    v.get(key)
        .and_then(|x| x.as_u64())
        .map(|n| n as usize)
        .unwrap_or(usize::MAX)
}

fn result(r: Result<(), &'static str>) -> Response {
    match r {
        Ok(()) => json_response(200, json!({ "ok": true })),
        Err(m) => bad(m),
    }
}

fn my_playlists(state: &Arc<AppState>, email: &str) -> Response {
    let list: Vec<Value> = pl::list_mine(&state.store, email)
        .iter()
        .map(|(p, n)| summary(state, p, *n, email, false))
        .collect();
    json_response(200, json!({ "playlists": list }))
}

fn public_playlists(state: &Arc<AppState>, email: &str, search: &str) -> Response {
    let list: Vec<Value> = pl::list_public(&state.store, &qs(search, "q"), 50)
        .iter()
        .map(|(p, n)| summary(state, p, *n, email, false))
        .collect();
    json_response(200, json!({ "playlists": list }))
}

fn one_playlist(state: &Arc<AppState>, email: &str, search: &str) -> Response {
    let Some(p) = pl::get(&state.store, &qs(search, "id")) else {
        return not_found();
    };
    if !pl::can_view(&p, email, &qs(search, "t")) {
        return not_found();
    }
    let tracks = pl::tracks(&state.store, &p.id);
    // Get its songs ready (matches and stream links) before anyone presses play.
    super::worker::warm_playlist(state, &p.id);
    let mut out = summary(state, &p, tracks.len() as i64, email, true);
    if pl::is_owner(&p, email) {
        out["shareToken"] = json!(p.share_token);
        out["spotifyStale"] = json!(
            !p.spotify_id.is_empty()
                && mitch_lib::school::now_millis() - p.spotify_synced_at > SPOTIFY_AUTO_SYNC_MS
        );
    }
    json_response(200, json!({ "playlist": out, "tracks": tracks }))
}

fn create(state: &Arc<AppState>, email: &str, v: &Value) -> Response {
    let public = v.get("public").and_then(|p| p.as_bool()).unwrap_or(false);
    match pl::create(&state.store, email, &text(v, "name"), public, "") {
        Ok(id) => {
            let _ = pl::add_tracks(&state.store, &id, parse_tracks(v));
            super::worker::warm_playlist(state, &id);
            json_response(200, json!({ "id": id }))
        }
        Err(m) => bad(m),
    }
}

fn update(state: &Arc<AppState>, email: &str, id: &str, v: &Value) -> Response {
    let p = match owned(state, email, id) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let name = v.get("name").and_then(|n| n.as_str());
    let public = v.get("public").and_then(|n| n.as_bool());
    result(pl::update(&state.store, &p.id, name, public))
}

fn copy(state: &Arc<AppState>, email: &str, id: &str, token: &str) -> Response {
    let Some(p) = pl::get(&state.store, id) else {
        return not_found();
    };
    if !pl::can_view(&p, email, token) {
        return not_found();
    }
    match pl::copy_to(&state.store, &p, email) {
        Ok(new_id) => json_response(200, json!({ "id": new_id })),
        Err(m) => bad(m),
    }
}

async fn spotify_import(state: &Arc<AppState>, email: &str, v: &Value) -> Response {
    let Some(sid) = super::spotify::parse_playlist_id(&text(v, "url")) else {
        return bad("Invalid link. Paste the link to a public Spotify playlist.");
    };
    if !allow(email, "spotify", 6, Duration::from_secs(60)) {
        return limited();
    }
    let imported = match super::spotify::fetch(&sid).await {
        Ok(p) => p,
        Err(e) => return json_response(e.status(), json!({ "error": e.message() })),
    };
    let public = v.get("public").and_then(|p| p.as_bool()).unwrap_or(false);
    let id = match pl::create(&state.store, email, &imported.name, public, &sid) {
        Ok(id) => id,
        Err(m) => return bad(m),
    };
    let n = imported.tracks.len();
    let _ = pl::sync_spotify(&state.store, &id, imported.tracks);
    super::worker::warm_playlist(state, &id);
    json_response(200, json!({ "id": id, "tracks": n }))
}

async fn spotify_sync(state: &Arc<AppState>, email: &str, p: &Playlist) -> Response {
    if p.spotify_id.is_empty() {
        return bad("This playlist isn't linked to Spotify");
    }
    if !allow(email, "spotify", 6, Duration::from_secs(60)) {
        return limited();
    }
    let fetched = match super::spotify::fetch(&p.spotify_id).await {
        Ok(f) => f,
        Err(e) => return json_response(e.status(), json!({ "error": e.message() })),
    };
    match pl::sync_spotify(&state.store, &p.id, fetched.tracks) {
        Ok((added, removed)) => {
            super::worker::warm_playlist(state, &p.id);
            json_response(200, json!({ "added": added, "removed": removed }))
        }
        Err(m) => bad(m),
    }
}

/// Words that mark a different version of a song than the studio recording.
const VERSION_WORDS: &[&str] = &[
    "live",
    "concert",
    "acoustic",
    "unplugged",
    "cover",
    "remix",
    "instrumental",
    "karaoke",
    "slowed",
    "reverb",
    "nightcore",
    "sped",
    "8d",
    "tour",
    "session",
    "sessions",
    "demo",
    "mashup",
    "stripped",
    "orchestral",
    "piano",
    "edit",
    "extended",
    "radio",
];

/// The version words a title carries (a bit per entry of `VERSION_WORDS`).
fn version_marks(title: &str) -> u32 {
    let lower = title.to_lowercase();
    let mut bits = 0;
    for w in lower.split(|c: char| !c.is_alphanumeric()) {
        if let Some(i) = VERSION_WORDS.iter().position(|v| *v == w) {
            bits |= 1 << i;
        }
    }
    bits
}

/// Picks the search result that is most likely the same recording as a
/// Spotify track: title/artist must match (`>= 0.5`), the length be within 4 s
/// when known, and the video must not be a different version (live, cover,
/// remix, slowed, instrumental...) unless the Spotify title says so itself;
/// among those, the closest title and length wins. Nothing is better than the
/// wrong version, so if no candidate qualifies the song stays unmatched.
pub(crate) fn best_match(
    title: &str,
    artist: &str,
    duration: i64,
    found: &[ytdlp::Track],
) -> Option<usize> {
    let first = artist.split(',').next().unwrap_or(artist);
    let wanted = version_marks(title);
    let mut best: Option<(usize, f32)> = None;
    for (i, t) in found.iter().enumerate() {
        let gap = (t.duration - duration).abs();
        if duration > 0 && gap > 4 {
            continue;
        }
        if version_marks(&t.title) & !wanted != 0 {
            continue;
        }
        let score = ml::match_score(title, first, &t.title, &t.artist);
        if score < 0.5 {
            continue;
        }
        let ranked = score - (gap as f32) * 0.03;
        if best.is_none_or(|(_, b)| ranked > b) {
            best = Some((i, ranked));
        }
    }
    best.map(|(i, _)| i)
}

/// Searches YouTube for the song and returns the best match's video id.
pub(super) async fn find_match(
    title: &str,
    artist: &str,
    duration: i64,
) -> Result<Option<String>, ytdlp::YtError> {
    let first = artist.split(',').next().unwrap_or("").trim();
    let found = ytdlp::search(&format!("{first} {title}")).await?;
    Ok(best_match(title, artist, duration, &found).map(|i| found[i].id.clone()))
}

async fn match_track(state: &Arc<AppState>, email: &str, search: &str) -> Response {
    let title = qs(search, "title");
    let artist = qs(search, "artist");
    let duration = qs(search, "duration").parse::<i64>().unwrap_or(0);
    if title.trim().is_empty() || title.chars().count() > 200 || artist.chars().count() > 200 {
        return bad("title required");
    }
    let key = pl::match_key(&title, &artist);
    if let Some(id) = pl::get_match(&state.store, &key) {
        return json_response(200, json!({ "id": id }));
    }
    if !allow(email, "match", 40, Duration::from_secs(60)) {
        return limited();
    }
    match find_match(&title, &artist, duration).await {
        Ok(Some(id)) => {
            pl::put_match(&state.store, &key, &id);
            json_response(200, json!({ "id": id }))
        }
        Ok(None) => json_response(404, json!({ "error": "couldn't find that song" })),
        Err(e) => super::yt_error(&e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spotify_matches_skip_other_versions() {
        let found = vec![
            tr("aaaaaaaaaaa", "Blinding Lights (Live)", "The Weeknd", 200),
            tr(
                "bbbbbbbbbbb",
                "Blinding Lights (Slowed + Reverb)",
                "The Weeknd",
                201,
            ),
            tr(
                "ccccccccccc",
                "Blinding Lights (Official Audio)",
                "The Weeknd",
                202,
            ),
        ];
        let pick = |title: &str| {
            best_match(title, "The Weeknd", 200, &found).map(|i| found[i].id.as_str())
        };
        assert_eq!(pick("Blinding Lights"), Some("ccccccccccc"));
        // A live version is what the playlist has: then it is the right one.
        assert_eq!(pick("Blinding Lights - Live"), Some("aaaaaaaaaaa"));
        // Only other versions around: no match rather than the wrong one.
        assert_eq!(
            best_match("Blinding Lights", "The Weeknd", 200, &found[..2]),
            None
        );
        // A length 5 s off is a different cut.
        assert_eq!(
            best_match("Blinding Lights", "The Weeknd", 207, &found[2..]),
            None
        );
    }

    fn tr(id: &str, title: &str, artist: &str, dur: i64) -> ytdlp::Track {
        ytdlp::Track {
            id: id.into(),
            title: title.into(),
            artist: artist.into(),
            duration: dur,
        }
    }

    #[test]
    fn matches_the_right_recording() {
        let found = vec![
            tr("a", "Blinding Lights (Live)", "The Weeknd", 260),
            tr("b", "Blinding Lights", "The Weeknd", 201),
            tr("c", "Blinding Lights (Cover)", "Someone Else", 200),
            tr("d", "Other Song", "The Weeknd", 200),
        ];
        assert_eq!(
            best_match("Blinding Lights", "The Weeknd", 200, &found),
            Some(1)
        );
        assert_eq!(
            best_match("Blinding Lights", "The Weeknd", 0, &found).map(|i| found[i].id.as_str()),
            Some("b")
        );
        assert_eq!(best_match("Totally Different", "Nobody", 200, &found), None);
        assert_eq!(
            best_match("Blinding Lights", "The Weeknd", 500, &found),
            None,
            "length must agree"
        );
    }

    #[test]
    fn parses_track_lists_and_bounds_them() {
        let many: Vec<Value> = (0..250)
            .map(|i| json!({"id":"aaaaaaaaaaa","title":format!("t{i}")}))
            .collect();
        assert_eq!(
            parse_tracks(&json!({"tracks": many})).len(),
            MAX_ADD_PER_CALL
        );
        assert!(parse_tracks(&json!({})).is_empty());
        assert_eq!(index(&json!({"from": -1}), "from"), usize::MAX);
    }
}
