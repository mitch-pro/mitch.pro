//! Lyrics lookup across every free source we could find. `find` queries all
//! providers at once and picks the best result by how precisely it is timed.
//!
//! | source | timing | notes |
//! | --- | --- | --- |
//! | LRCLIB | word (`lyricsfile`), line, plain | open database, no key |
//! | NetEase Cloud Music | word (YRC), line | unofficial API |
//! | Kugou | word (KRC) | unofficial API, XOR+zlib encoded |
//! | QQ Music | line | unofficial API |
//! | lyrics.ovh | plain | aggregator, last resort for the text |
//! | YouTube captions | word (auto-generated), line (manual) | via yt-dlp, only if nothing better turned up in 2 s |
//!
//! Left out on purpose: sources that need a paid subscription or a private
//! session token (Musixmatch, Apple Music, Spotify), and Genius (needs page
//! scraping, which its terms forbid).
//!
//! Ranking (`rank`): real word timing 4 > line timing 3 > auto-caption word
//! timing 2 > plain text 1. Auto-captions sit below line-timed lyrics
//! because speech recognition mishears sung words, even though it is timed
//! per word; a line-timed song still gets word-by-word highlighting from
//! `estimate_words`. A candidate must also match the wanted title/artist
//! (`MIN_SCORE`) and, when both durations are known, be within
//! `DURATION_TOLERANCE_MS`, so a live version's lyrics don't land on the
//! studio track.

use super::ytdlp;
use base64::Engine;
use futures_util::future::BoxFuture;
use futures_util::StreamExt;
use mitch_lib::music_lyrics::{self as ml, Lyrics, Sync};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

const MIN_SCORE: f32 = 0.6;
const DURATION_TOLERANCE_MS: u32 = 4000;
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(7);
const CAPTIONS_TIMEOUT: Duration = Duration::from_secs(14);
/// Captions are only fetched when no line-timed (or better) lyrics have
/// arrived after this long: they cost a yt-dlp run.
const CAPTIONS_GRACE: Duration = Duration::from_millis(2000);
const HARD_DEADLINE: Duration = Duration::from_secs(10);
/// The most the search for word timing to borrow may take.
const DONOR_LIMIT: Duration = Duration::from_secs(3);
/// How long after line-timed lyrics arrive the other sources may still
/// replace them with word-timed ones.
const UPGRADE_WINDOW: Duration = Duration::from_millis(1200);

#[derive(Debug, Clone)]
pub struct Query {
    pub title: String,
    pub artist: String,
    pub duration_ms: u32,
    pub video_id: String,
    /// Which recording lengths count as the same song.
    pub length: Length,
    /// Only word-timed results matter (the search for word timing to borrow).
    pub donor: bool,
    /// Also try YouTube's captions. Only the first reading of the title does,
    /// so a lookup costs at most one extra yt-dlp run.
    pub captions: bool,
}

/// How far a recording's length may be from the video's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Length {
    /// Within a few seconds: the same recording.
    Same,
    /// Up to four times longer: the full version of a TV-size or clipped upload.
    FullVersion,
    /// 0.6 to 1.7 times as long: the same song cut differently (a video with an
    /// intro, an edit), good enough to borrow word timing from.
    Similar,
}

#[derive(Debug, Clone)]
struct Cand {
    source: &'static str,
    rank: u8,
    score: f32,
    lyrics: Lyrics,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Tried {
    pub source: &'static str,
    pub status: String,
}

pub struct Found {
    pub source: &'static str,
    pub lyrics: Lyrics,
    pub rank: u8,
    score: f32,
    /// Matched by relaxed length: the timing is for a longer recording.
    pub approximate: bool,
}

fn client() -> Option<&'static reqwest::Client> {
    static C: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(ytdlp::DESKTOP_UA)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(8))
            .build()
            .ok()
    })
    .as_ref()
}

fn rank(l: &Lyrics, asr: bool) -> u8 {
    if asr {
        return 2;
    }
    match l.sync {
        Sync::Word => 4,
        Sync::Line => 3,
        Sync::None => 1,
    }
}

fn usable(mut l: Lyrics) -> Option<Lyrics> {
    ml::strip_credits(&mut l);
    (!l.is_empty()).then_some(l)
}

fn cand(source: &'static str, score: f32, asr: bool, l: Lyrics) -> Option<Cand> {
    let l = usable(l)?;
    Some(Cand {
        source,
        rank: rank(&l, asr),
        score,
        lyrics: l,
    })
}

async fn get_json(url: &str, headers: &[(&str, &str)]) -> Result<Value, String> {
    let c = client().ok_or("no http client")?;
    let mut req = c.get(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let resp = req.send().await.map_err(|e| short(&e))?;
    if !resp.status().is_success() {
        return Err(format!("http {}", resp.status().as_u16()));
    }
    let bytes = resp.bytes().await.map_err(|e| short(&e))?;
    if bytes.len() > ytdlp::MAX_BODY_BYTES {
        return Err("response too large".into());
    }
    serde_json::from_slice::<Value>(&bytes).map_err(|_| "bad response".to_string())
}

fn short(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timeout".into()
    } else if e.is_connect() {
        "unreachable".into()
    } else {
        "request failed".into()
    }
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

fn secs_to_ms(v: Option<&Value>) -> u32 {
    (v.and_then(|x| x.as_f64()).unwrap_or(0.0) * 1000.0) as u32
}

/// The provider's matches that are worth fetching lyrics for, best first.
fn best_matches<T>(q: &Query, mut items: Vec<(f32, u32, T)>, take: usize) -> Vec<(f32, T)> {
    items.retain(|(score, dur, _)| {
        let length_ok = match q.length {
            Length::Same => ml::duration_close(q.duration_ms, *dur, DURATION_TOLERANCE_MS),
            Length::FullVersion => *dur == 0 || ml::duration_is_full_version(q.duration_ms, *dur),
            Length::Similar => *dur == 0 || ml::duration_is_similar(q.duration_ms, *dur),
        };
        *score >= 0.55 && length_ok
    });
    items.sort_by(|a, b| b.0.total_cmp(&a.0));
    items
        .into_iter()
        .take(take)
        .map(|(s, _, t)| (s, t))
        .collect()
}

// ---------------------------------------------------------- LRCLIB ----

async fn lrclib(q: &Query) -> Result<Vec<Cand>, String> {
    let ua = [("User-Agent", "mitch.pro music player (https://mitch.pro)")];
    let mut url = format!("https://lrclib.net/api/search?track_name={}", enc(&q.title));
    if !q.artist.is_empty() {
        url.push_str(&format!("&artist_name={}", enc(&q.artist)));
    }
    let mut list = get_json(&url, &ua).await?;
    if list.as_array().is_none_or(|a| a.is_empty()) {
        let both = format!("{} {}", q.artist, q.title);
        list = get_json(
            &format!("https://lrclib.net/api/search?q={}", enc(both.trim())),
            &ua,
        )
        .await?;
    }
    let mut items = Vec::new();
    for item in list.as_array().into_iter().flatten() {
        if item.get("instrumental").and_then(|v| v.as_bool()) == Some(true) {
            continue;
        }
        // Looking for word timing to borrow: skip entries that have none, or
        // they crowd the ones that do out of the top results.
        if q.donor && item.get("hasWordSync").and_then(|v| v.as_bool()) != Some(true) {
            continue;
        }
        let score = ml::match_score(
            &q.title,
            &q.artist,
            &text(item, "trackName"),
            &text(item, "artistName"),
        );
        items.push((score, secs_to_ms(item.get("duration")), item.clone()));
    }
    let mut out = Vec::new();
    for (score, item) in best_matches(q, items, 6) {
        let word_sync = item.get("hasWordSync").and_then(|v| v.as_bool()) == Some(true);
        let mut lyrics = None;
        if word_sync {
            let l = ml::parse_lyricsfile(&text(&item, "lyricsfile"));
            if l.sync == Sync::Word {
                lyrics = Some(l);
            }
        }
        if lyrics.is_none() {
            let synced = text(&item, "syncedLyrics");
            if !synced.trim().is_empty() {
                lyrics = Some(ml::parse_lrc(&synced));
            }
        }
        if lyrics.is_none() {
            let plain = text(&item, "plainLyrics");
            if !plain.trim().is_empty() {
                lyrics = Some(ml::parse_plain(&plain));
            }
        }
        if let Some(c) = lyrics.and_then(|l| cand("LRCLIB", score, false, l)) {
            out.push(c);
        }
    }
    Ok(out)
}

// --------------------------------------------------------- NetEase ----

async fn netease(q: &Query) -> Result<Vec<Cand>, String> {
    let h = [
        ("Referer", "https://music.163.com/"),
        ("Cookie", "appver=2.0.2; os=pc"),
    ];
    let kw = format!("{} {}", q.title, q.artist);
    let v = get_json(
        &format!(
            "https://music.163.com/api/search/get/web?csrf_token=&s={}&type=1&offset=0&limit=6",
            enc(kw.trim())
        ),
        &h,
    )
    .await?;
    let mut items = Vec::new();
    for s in v
        .pointer("/result/songs")
        .and_then(|s| s.as_array())
        .into_iter()
        .flatten()
    {
        let Some(id) = s.get("id").and_then(|i| i.as_i64()) else {
            continue;
        };
        let artists = s
            .get("artists")
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .map(|x| text(x, "name"))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        let score = ml::match_score(&q.title, &q.artist, &text(s, "name"), &artists);
        let dur = s.get("duration").and_then(|d| d.as_u64()).unwrap_or(0) as u32;
        items.push((score, dur, id));
    }
    let mut out = Vec::new();
    for (score, id) in best_matches(q, items, 2) {
        let v = get_json(
            &format!("https://music.163.com/api/song/lyric?id={id}&lv=1&kv=-1&tv=-1&yv=1"),
            &h,
        )
        .await?;
        let yrc = v
            .pointer("/yrc/lyric")
            .and_then(|l| l.as_str())
            .unwrap_or("");
        let lrc = v
            .pointer("/lrc/lyric")
            .and_then(|l| l.as_str())
            .unwrap_or("");
        let parsed = if yrc.contains(",0)") && ml::parse_yrc(yrc).sync == Sync::Word {
            Some(ml::parse_yrc(yrc))
        } else if !lrc.trim().is_empty() {
            Some(ml::parse_lrc(lrc))
        } else {
            None
        };
        if let Some(c) = parsed.and_then(|l| cand("NetEase", score, false, l)) {
            out.push(c);
        }
    }
    Ok(out)
}

// ----------------------------------------------------------- Kugou ----

async fn kugou(q: &Query) -> Result<Vec<Cand>, String> {
    let kw = format!("{} - {}", q.artist, q.title);
    let v = get_json(
        &format!(
            "https://lyrics.kugou.com/search?ver=1&man=yes&client=pc&keyword={}&duration={}&hash=",
            enc(kw.trim_start_matches(" - ")),
            q.duration_ms
        ),
        &[],
    )
    .await?;
    let mut items = Vec::new();
    for c in v
        .get("candidates")
        .and_then(|c| c.as_array())
        .into_iter()
        .flatten()
    {
        let score = ml::match_score(&q.title, &q.artist, &text(c, "song"), &text(c, "singer"));
        let dur = c.get("duration").and_then(|d| d.as_u64()).unwrap_or(0) as u32;
        items.push((score, dur, (c.get("id").cloned(), text(c, "accesskey"))));
    }
    let mut out = Vec::new();
    for (score, (id, key)) in best_matches(q, items, 2) {
        let id = match id {
            Some(Value::String(s)) => s,
            Some(Value::Number(n)) => n.to_string(),
            _ => continue,
        };
        let v = get_json(
            &format!(
                "https://lyrics.kugou.com/download?ver=1&client=pc&id={}&accesskey={}&fmt=krc&charset=utf8",
                enc(&id),
                enc(&key)
            ),
            &[],
        )
        .await?;
        let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(text(&v, "content")) else {
            continue;
        };
        let parsed = if text(&v, "fmt") == "krc" {
            ml::decode_krc(&raw).map(|t| ml::parse_krc(&t))
        } else {
            Some(ml::parse_lrc(&String::from_utf8_lossy(&raw)))
        };
        if let Some(c) = parsed.and_then(|l| cand("Kugou", score, false, l)) {
            out.push(c);
        }
    }
    Ok(out)
}

// -------------------------------------------------------- QQ Music ----

async fn qqmusic(q: &Query) -> Result<Vec<Cand>, String> {
    let kw = format!("{} {}", q.title, q.artist);
    let v = get_json(
        &format!(
            "https://c.y.qq.com/splcloud/fcgi-bin/smartbox_new.fcg?key={}&format=json",
            enc(kw.trim())
        ),
        &[("Referer", "https://y.qq.com/")],
    )
    .await?;
    let mut items = Vec::new();
    for s in v
        .pointer("/data/song/itemlist")
        .and_then(|s| s.as_array())
        .into_iter()
        .flatten()
    {
        let score = ml::match_score(&q.title, &q.artist, &text(s, "name"), &text(s, "singer"));
        // The search box result has no duration; the lyrics' last timestamp
        // is checked instead.
        items.push((score, 0, text(s, "mid")));
    }
    let mut out = Vec::new();
    for (score, mid) in best_matches(q, items, 2) {
        let v = get_json(
            &format!(
                "https://c.y.qq.com/lyric/fcgi-bin/fcg_query_lyric_new.fcg?songmid={}&g_tk=5381&format=json&inCharset=utf8&outCharset=utf-8&nobase64=1",
                enc(&mid)
            ),
            &[("Referer", "https://y.qq.com/")],
        )
        .await?;
        let lrc = text(&v, "lyric");
        if lrc.trim().is_empty() {
            continue;
        }
        let l = ml::parse_lrc(&lrc);
        // QQ has many cover/live versions with the same name: a lyric that
        // ends long after the track does is a different recording.
        let last = l.lines.last().map(|x| x.t).unwrap_or(0);
        if q.length == Length::Same && q.duration_ms > 0 && last > q.duration_ms + 15_000 {
            continue;
        }
        if let Some(c) = cand("QQ Music", score, false, l) {
            out.push(c);
        }
    }
    Ok(out)
}

// ------------------------------------------------------ lyrics.ovh ----

async fn lyrics_ovh(q: &Query) -> Result<Vec<Cand>, String> {
    if q.artist.is_empty() || q.title.is_empty() {
        return Ok(Vec::new());
    }
    let path = |s: &str| enc(s).replace('+', "%20");
    let v = get_json(
        &format!(
            "https://api.lyrics.ovh/v1/{}/{}",
            path(&q.artist),
            path(&ml::normalize(&q.title))
        ),
        &[],
    )
    .await;
    let Ok(v) = v else {
        return Ok(Vec::new()); // a 404 means "not found"
    };
    let body = text(&v, "lyrics").replace("\r\n", "\n");
    // Plain lyrics carry no timing to verify, so the match is taken on trust
    // from the exact artist/title lookup.
    Ok(cand("lyrics.ovh", 0.7, false, ml::parse_plain(&body))
        .into_iter()
        .collect())
}

// ------------------------------------------------ YouTube captions ----

async fn youtube_captions(q: &Query) -> Result<Vec<Cand>, String> {
    let caps = ytdlp::captions(&q.video_id)
        .await
        .map_err(|e| e.public_message().to_string())?;
    let Some(caps) = caps else {
        return Ok(Vec::new());
    };
    let l = ml::parse_json3(&caps.json);
    Ok(cand("YouTube captions", 1.0, caps.auto, l)
        .into_iter()
        .collect())
}

// ---------------------------------------------------- orchestration ----

type Job = BoxFuture<'static, (&'static str, Result<Vec<Cand>, String>)>;

fn job<F>(source: &'static str, limit: Duration, fut: F) -> Job
where
    F: std::future::Future<Output = Result<Vec<Cand>, String>> + Send + 'static,
{
    Box::pin(async move {
        let r = match tokio::time::timeout(limit, fut).await {
            Ok(r) => r,
            Err(_) => Err("timeout".to_string()),
        };
        (source, r)
    })
}

/// Looks lyrics up under every plausible reading of the title (see
/// `query_guesses`), all at once, and keeps the best. If nothing matches by
/// length, tries again accepting a longer recording, which finds the full
/// song for a TV-size or clipped upload; that result is flagged `approximate`.
pub async fn find(
    title: &str,
    artist: &str,
    duration_ms: u32,
    video_id: &str,
    with_captions: bool,
) -> (Option<Found>, Vec<Tried>) {
    let guesses = ml::query_guesses(title, artist);
    let mut passes = vec![Length::Same];
    if duration_ms > 0 {
        passes.push(Length::FullVersion);
    }
    let mut first_tried: Vec<Tried> = Vec::new();
    for length in passes {
        let results = search_all(
            &guesses,
            title_query(duration_ms, video_id, length, false, with_captions),
            if length == Length::Same {
                HARD_DEADLINE
            } else {
                Duration::from_secs(6)
            },
        )
        .await;
        if first_tried.is_empty() {
            first_tried = results.first().map(|r| r.1.clone()).unwrap_or_default();
        }
        let best = best_of(results);
        if let Some(mut f) = best {
            f.approximate = length == Length::FullVersion;
            // Line timing that fits this video, but word timing exists for the
            // song at another length: keep the video's line times and take the
            // words from there.
            if f.rank == 3 && !f.approximate && duration_ms > 0 {
                let donors = search_all(
                    &guesses,
                    title_query(duration_ms, video_id, Length::Similar, true, false),
                    // Word timing to borrow is a bonus: don't hold the lyrics
                    // back long waiting for one that may not exist.
                    DONOR_LIMIT,
                )
                .await;
                if let Some(d) = best_of(donors) {
                    if let Some(grafted) = ml::graft_words(&f.lyrics, &d.lyrics) {
                        f.lyrics = grafted;
                        f.rank = 4;
                    }
                }
            }
            return (Some(f), first_tried);
        }
    }
    (None, first_tried)
}

/// Checks lyric timing against the video's own captions and shifts it when it
/// is off (see `ml::estimate_offset`). Returns the shift applied, if any. A
/// video without captions, or a failed fetch, leaves the lyrics as they are.
pub async fn align(video_id: &str, lyrics: &mut ml::Lyrics) -> Option<i64> {
    if lyrics.sync == ml::Sync::None || lyrics.lines.is_empty() {
        return None;
    }
    let caps = tokio::time::timeout(CAPTIONS_TIMEOUT, ytdlp::captions(video_id))
        .await
        .ok()?
        .ok()??;
    let reference = ml::parse_json3(&caps.json);
    let offset = ml::estimate_offset(lyrics, &reference)?;
    ml::shift(lyrics, offset);
    tracing::info!("[music] lyrics for {video_id} were {offset} ms off; shifted");
    Some(offset)
}

fn title_query(
    duration_ms: u32,
    video_id: &str,
    length: Length,
    donor: bool,
    captions: bool,
) -> Query {
    Query {
        title: String::new(),
        artist: String::new(),
        duration_ms,
        video_id: video_id.to_string(),
        length,
        donor,
        captions,
    }
}

/// Runs `find_once` for the first four readings of the title at once and
/// collects results as they arrive, without waiting on slow readings:
/// - a close word-timed match ends it at once (nothing can beat it);
/// - once line-timed (or better) lyrics are in, the rest get `UPGRADE_WINDOW`
///   to improve on them;
/// - otherwise it waits at most `limit`.
///
/// Only the first reading of a normal (not donor) pass also tries YouTube's
/// captions.
async fn search_all(
    guesses: &[(String, String)],
    base: Query,
    limit: Duration,
) -> Vec<(Option<Found>, Vec<Tried>)> {
    let mut runs: futures_util::stream::FuturesUnordered<_> = guesses
        .iter()
        .take(4)
        .enumerate()
        .map(|(i, (t, a))| {
            find_once(Query {
                title: t.clone(),
                artist: a.clone(),
                captions: base.captions && i == 0 && !base.donor && base.length == Length::Same,
                ..base.clone()
            })
        })
        .collect();
    let mut out = Vec::new();
    let mut deadline = tokio::time::Instant::now() + limit;
    let mut upgrading = false;
    while let Ok(Some(result)) = tokio::time::timeout_at(deadline, runs.next()).await {
        let (rank, score) = result
            .0
            .as_ref()
            .map(|f| (f.rank, f.score))
            .unwrap_or((0, 0.0));
        out.push(result);
        if rank == 4 && score >= 0.85 {
            break;
        }
        if rank >= 3 && !upgrading {
            upgrading = true;
            deadline = deadline.min(tokio::time::Instant::now() + UPGRADE_WINDOW);
        }
    }
    out
}

fn best_of(results: Vec<(Option<Found>, Vec<Tried>)>) -> Option<Found> {
    results
        .into_iter()
        .filter_map(|(f, _)| f)
        .max_by(|a, b| a.rank.cmp(&b.rank).then(a.score.total_cmp(&b.score)))
}

/// Queries every provider for one reading of the title and returns the best
/// lyrics, plus what each provider reported (for the "tried" list).
async fn find_once(q: Query) -> (Option<Found>, Vec<Tried>) {
    let q = Arc::new(q);
    let have_line_sync = Arc::new(AtomicBool::new(false));
    let mut jobs: futures_util::stream::FuturesUnordered<Job> = Default::default();
    macro_rules! provider {
        ($name:literal, $f:ident) => {{
            let q = q.clone();
            jobs.push(job($name, PROVIDER_TIMEOUT, async move { $f(&q).await }));
        }};
    }
    provider!("LRCLIB", lrclib);
    provider!("NetEase", netease);
    provider!("Kugou", kugou);
    provider!("QQ Music", qqmusic);
    provider!("lyrics.ovh", lyrics_ovh);
    if q.captions {
        let q = q.clone();
        let gate = have_line_sync.clone();
        jobs.push(job("YouTube captions", CAPTIONS_TIMEOUT, async move {
            tokio::time::sleep(CAPTIONS_GRACE).await;
            if gate.load(Ordering::Relaxed) {
                return Err("skipped".to_string());
            }
            youtube_captions(&q).await
        }));
    }

    let mut cands: Vec<Cand> = Vec::new();
    let mut tried: Vec<Tried> = Vec::new();
    // Nothing waits past HARD_DEADLINE; once line-timed lyrics are in hand the
    // rest get only UPGRADE_WINDOW more to deliver real word timing.
    let mut deadline = tokio::time::Instant::now() + HARD_DEADLINE;
    loop {
        let next = tokio::time::timeout_at(deadline, jobs.next()).await;
        let Ok(Some((source, result))) = next else {
            break;
        };
        let status = match result {
            Ok(list) if list.is_empty() => "no match".to_string(),
            Ok(list) => {
                let best = list.iter().map(|c| c.rank).max().unwrap_or(0);
                if best >= 3 && !have_line_sync.swap(true, Ordering::Relaxed) {
                    deadline = deadline.min(tokio::time::Instant::now() + UPGRADE_WINDOW);
                }
                cands.extend(list);
                match best {
                    4 => "word",
                    3 => "line",
                    2 => "captions",
                    _ => "plain",
                }
                .to_string()
            }
            Err(e) => e,
        };
        tried.push(Tried { source, status });
        // Word-timed lyrics from a close match can't be beaten.
        if cands.iter().any(|c| c.rank == 4 && c.score >= 0.85) {
            break;
        }
    }
    if q.donor {
        cands.retain(|c| c.rank == 4);
    }
    (choose(cands), tried)
}

fn choose(mut cands: Vec<Cand>) -> Option<Found> {
    cands.retain(|c| c.score >= MIN_SCORE);
    cands.sort_by(|a, b| b.rank.cmp(&a.rank).then(b.score.total_cmp(&a.score)));
    cands.into_iter().next().map(|c| Found {
        source: c.source,
        lyrics: c.lyrics,
        rank: c.rank,
        score: c.score,
        approximate: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lyrics(sync_text: &str) -> Lyrics {
        ml::parse_lrc(sync_text)
    }

    #[test]
    fn word_timing_beats_line_timing_but_not_a_bad_match() {
        let word = cand(
            "W",
            0.9,
            false,
            lyrics("[00:01.00]<00:01.00>a <00:01.50>b\n[00:03.00]<00:03.00>c <00:03.50>d\n"),
        )
        .unwrap();
        let line = cand("L", 1.0, false, lyrics("[00:01.00]a b\n[00:03.00]c d\n")).unwrap();
        let plain = cand("P", 1.0, false, ml::parse_plain("a b\nc d")).unwrap();
        let asr = cand(
            "C",
            1.0,
            true,
            lyrics("[00:01.00]<00:01.00>a <00:01.50>b\n[00:03.00]<00:03.00>c <00:03.50>d\n"),
        )
        .unwrap();
        assert_eq!((word.rank, line.rank, plain.rank, asr.rank), (4, 3, 1, 2));
        let best = choose(vec![plain.clone(), line.clone(), word.clone(), asr.clone()]).unwrap();
        assert_eq!(best.source, "W");
        let mut weak = word.clone();
        weak.score = 0.3;
        assert_eq!(choose(vec![weak, line.clone()]).unwrap().source, "L");
        assert_eq!(choose(vec![asr, plain]).unwrap().source, "C");
        assert!(choose(Vec::new()).is_none());
    }

    #[test]
    fn credits_only_text_is_not_usable() {
        assert!(cand("X", 1.0, false, lyrics("[00:00.10]Lyrics by：Someone\n")).is_none());
    }

    #[test]
    fn best_matches_filters_by_score_and_duration() {
        let q = Query {
            title: "t".into(),
            artist: "a".into(),
            duration_ms: 200_000,
            video_id: String::new(),
            length: Length::Same,
            donor: false,
            captions: false,
        };
        let items = vec![
            (0.9, 201_000, "good"),
            (0.9, 90_000, "wrong length"),
            (0.2, 200_000, "bad name"),
            (0.7, 0, "unknown length"),
        ];
        let got: Vec<&str> = best_matches(&q, items, 5)
            .into_iter()
            .map(|(_, t)| t)
            .collect();
        assert_eq!(got, vec!["good", "unknown length"]);
        let tv = Query {
            duration_ms: 91_000,
            length: Length::FullVersion,
            ..q.clone()
        };
        let items = vec![
            (0.9, 260_000, "full song"),
            (0.9, 500_000, "too long"),
            (0.9, 40_000, "shorter"),
        ];
        let got: Vec<&str> = best_matches(&tv, items, 5)
            .into_iter()
            .map(|(_, t)| t)
            .collect();
        assert_eq!(got, vec!["full song"]);
    }
}
