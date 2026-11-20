//! Lyrics model, format parsers and track matching for the music player
//! (`docs/music.md`). Pure functions, no network or database: the providers
//! in `mitch-server`'s `routes/music/lyrics.rs` fetch raw text and hand it to
//! the parsers here, and `music.rs` stores the normalized result.
//!
//! Everything is normalized to [`Lyrics`]: a list of [`Line`]s, each with an
//! optional list of timed [`Word`]s. A "word" is whatever unit the source
//! times (a whole word, a syllable, or one CJK character). Its `text` carries
//! its own trailing space, so the client renders a line by concatenating word
//! texts, and syllables of one word stay glued together.
//!
//! Sync levels, best first:
//! - [`Sync::Word`]: the source times each word (enhanced LRC, NetEase YRC,
//!   Kugou KRC, YouTube auto-captions).
//! - [`Sync::Line`]: one timestamp per line (plain LRC). [`estimate_words`]
//!   can spread a line's time over its words so the player can still
//!   highlight word by word; `Lyrics::estimated` records that.
//! - [`Sync::None`]: plain text.
//!
//! The cache stores lyrics as enhanced LRC text ([`to_enhanced_lrc`]) rather
//! than a JSON blob, so a cached row is a normal lyrics file.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Word {
    /// Start, milliseconds from the start of the track.
    pub t: u32,
    /// End, milliseconds.
    pub e: u32,
    /// The text, including a trailing space when a space follows it.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Line {
    pub t: u32,
    pub e: u32,
    pub text: String,
    pub words: Vec<Word>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Sync {
    None,
    Line,
    Word,
}

impl Sync {
    pub fn as_str(self) -> &'static str {
        match self {
            Sync::None => "none",
            Sync::Line => "line",
            Sync::Word => "word",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Lyrics {
    pub sync: Sync,
    /// True when `words` were spread over line time by [`estimate_words`]
    /// instead of coming from the source.
    pub estimated: bool,
    pub lines: Vec<Line>,
}

impl Lyrics {
    pub fn empty() -> Self {
        Lyrics {
            sync: Sync::None,
            estimated: false,
            lines: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.text.trim().is_empty())
    }
}

/// Last line (or last word) with no following timestamp lasts this long.
const TAIL_MS: u32 = 4000;

// --------------------------------------------------------------- LRC ----

/// Parses `mm:ss.xx`, `mm:ss.xxx`, `mm:ss` and `mm:ss:xx` (also `h:mm:ss.xx`)
/// into milliseconds.
fn parse_ts(s: &str) -> Option<u32> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    let (h, m, rest) = match parts.as_slice() {
        [m, rest] => (0u64, m.parse::<u64>().ok()?, *rest),
        [h, m, rest] => (h.parse::<u64>().ok()?, m.parse::<u64>().ok()?, *rest),
        _ => return None,
    };
    // `mm:ss:xx` uses a colon before the hundredths; treat it like a dot.
    let rest = rest.replace(':', ".");
    let (sec, frac) = rest.split_once('.').unwrap_or((rest.as_str(), ""));
    if sec.is_empty() || !sec.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let sec: u64 = sec.parse().ok()?;
    let mut f = frac.to_string();
    f.truncate(3);
    while f.len() < 3 {
        f.push('0');
    }
    let ms: u64 = f.parse().ok()?;
    u32::try_from((h * 3600 + m * 60 + sec) * 1000 + ms).ok()
}

fn fmt_ts(ms: u32) -> String {
    format!("{:02}:{:02}.{:03}", ms / 60000, (ms / 1000) % 60, ms % 1000)
}

/// Splits `<mm:ss.xx>` word tags out of one lyrics line. Returns
/// (word start, text) pairs; text before the first tag is attributed to
/// `line_start`.
fn split_word_tags(text: &str, line_start: u32) -> Vec<(u32, String)> {
    let mut out: Vec<(u32, String)> = Vec::new();
    let mut cur_t = line_start;
    let mut cur = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let tag = after
            .find('>')
            .and_then(|close| parse_ts(&after[..close]).map(|ts| (ts, open + 1 + close + 1)));
        match tag {
            Some((ts, consumed)) => {
                cur.push_str(&rest[..open]);
                if !cur.is_empty() {
                    out.push((cur_t, std::mem::take(&mut cur)));
                }
                cur_t = ts;
                rest = &rest[consumed..];
            }
            None => {
                cur.push_str(&rest[..=open]);
                rest = &rest[open + 1..];
            }
        }
    }
    cur.push_str(rest);
    if !cur.is_empty() {
        out.push((cur_t, cur));
    }
    out
}

/// Parses LRC and enhanced LRC (word tags). Text with no timestamps at all is
/// returned as plain lyrics ([`Sync::None`]).
pub fn parse_lrc(text: &str) -> Lyrics {
    let mut offset: i64 = 0;
    let mut entries: Vec<(u32, String)> = Vec::new();
    let mut untimed: Vec<String> = Vec::new();
    for raw in text.lines() {
        let mut line = raw.trim_start_matches('\u{feff}').trim_end();
        let mut stamps: Vec<u32> = Vec::new();
        while line.starts_with('[') {
            let Some(close) = line.find(']') else { break };
            let tag = &line[1..close];
            if let Some(ts) = parse_ts(tag) {
                stamps.push(ts);
            } else if let Some(v) = tag.strip_prefix("offset:") {
                offset = v.trim().parse().unwrap_or(0);
            } else if !tag.contains(':') {
                break;
            }
            // Metadata tags such as [ar:...] are dropped either way.
            line = &line[close + 1..];
        }
        if stamps.is_empty() {
            let is_meta = raw.trim_start().starts_with('[') && raw.contains(':');
            if !is_meta && !raw.trim().is_empty() {
                untimed.push(raw.trim().to_string());
            }
            continue;
        }
        for ts in stamps {
            entries.push((ts, line.trim_start().to_string()));
        }
    }
    if entries.is_empty() {
        return plain(untimed.iter().map(String::as_str));
    }
    entries.sort_by_key(|(t, _)| *t);
    let shift = |t: u32| -> u32 { u32::try_from((i64::from(t) - offset).max(0)).unwrap_or(0) };
    let mut lines: Vec<Line> = Vec::with_capacity(entries.len());
    for (t, body) in &entries {
        let t = shift(*t);
        let parts = split_word_tags(body, 0)
            .into_iter()
            .map(|(wt, s)| (if wt == 0 { t } else { shift(wt) }, s))
            .collect::<Vec<_>>();
        let timed_words = parts.len() >= 2;
        let words = if timed_words {
            parts
                .iter()
                .map(|(wt, s)| Word {
                    t: *wt,
                    e: *wt,
                    text: s.clone(),
                })
                .collect()
        } else {
            Vec::new()
        };
        let text = parts.iter().map(|(_, s)| s.as_str()).collect::<String>();
        lines.push(Line {
            t,
            e: t,
            text: text.trim().to_string(),
            words,
        });
    }
    finish(lines)
}

fn plain<'a>(rows: impl Iterator<Item = &'a str>) -> Lyrics {
    Lyrics {
        sync: Sync::None,
        estimated: false,
        lines: rows
            .map(|r| Line {
                t: 0,
                e: 0,
                text: r.trim().to_string(),
                words: Vec::new(),
            })
            .collect(),
    }
}

/// Plain, untimed lyrics text.
pub fn parse_plain(text: &str) -> Lyrics {
    let mut rows: Vec<&str> = text.lines().collect();
    while rows.first().is_some_and(|r| r.trim().is_empty()) {
        rows.remove(0);
    }
    while rows.last().is_some_and(|r| r.trim().is_empty()) {
        rows.pop();
    }
    plain(rows.into_iter())
}

/// Fills in line and word end times, trims, and decides the sync level.
/// Input lines must be sorted by start time.
fn finish(mut lines: Vec<Line>) -> Lyrics {
    let starts: Vec<u32> = lines.iter().map(|l| l.t).collect();
    for (i, line) in lines.iter_mut().enumerate() {
        let next = starts.get(i + 1).copied();
        let last_word_start = line.words.last().map(|w| w.t).unwrap_or(line.t);
        line.e = match next {
            Some(n) if n >= line.t => n,
            _ => last_word_start.max(line.t) + TAIL_MS,
        };
        let line_end = line.e;
        let n_words = line.words.len();
        for j in 0..n_words {
            let next_start = line.words.get(j + 1).map(|w| w.t);
            let w = &mut line.words[j];
            w.e = match next_start {
                Some(n) => n.max(w.t),
                // Last word: until the next line, but a long instrumental
                // gap shouldn't keep it lit.
                None => {
                    let natural = w.t + (w.text.chars().count() as u32 * 120).max(500);
                    natural.min(line_end).max(w.t)
                }
            };
        }
    }
    let spoken: Vec<&Line> = lines.iter().filter(|l| !l.text.is_empty()).collect();
    let worded = spoken.iter().filter(|l| l.words.len() >= 2).count();
    let sync = if !spoken.is_empty() && worded * 2 >= spoken.len() {
        Sync::Word
    } else {
        Sync::Line
    };
    Lyrics {
        sync,
        estimated: false,
        lines,
    }
}

/// Serializes to enhanced LRC (plain text for [`Sync::None`]). Round-trips
/// through [`parse_lrc`] / [`parse_plain`].
pub fn to_enhanced_lrc(l: &Lyrics) -> String {
    let mut out = String::new();
    for line in &l.lines {
        if l.sync == Sync::None {
            out.push_str(&line.text);
        } else {
            out.push('[');
            out.push_str(&fmt_ts(line.t));
            out.push(']');
            if l.sync == Sync::Word && !l.estimated && line.words.len() >= 2 {
                for w in &line.words {
                    out.push('<');
                    out.push_str(&fmt_ts(w.t));
                    out.push('>');
                    out.push_str(&w.text);
                }
            } else {
                out.push_str(&line.text);
            }
        }
        out.push('\n');
    }
    out
}

/// Rebuilds [`Lyrics`] from cached text; `sync` is the stored level.
pub fn from_cached(body: &str, sync: &str, estimated: bool) -> Lyrics {
    let mut l = if sync == "none" {
        parse_plain(body)
    } else {
        parse_lrc(body)
    };
    if estimated {
        estimate_words(&mut l);
    }
    l
}

// --------------------------------------------- word-level (Chinese) ----

/// Finds `<open>a,b,c<close>` at the start of `s`. Returns (a, b, byte length
/// of the header).
fn seg_header(s: &str, open: char, close: char) -> Option<(i64, i64, usize)> {
    let inner = s.strip_prefix(open)?;
    let end = inner.find(close)?;
    let mut nums = inner[..end].split(',');
    let a = nums.next()?.trim().parse::<i64>().ok()?;
    let b = nums.next()?.trim().parse::<i64>().ok()?;
    nums.next()?.trim().parse::<i64>().ok()?;
    if nums.next().is_some() {
        return None;
    }
    Some((a, b, open.len_utf8() + end + close.len_utf8()))
}

/// Splits `(a,b,c)text(a,b,c)text...` into (a, b, text) segments.
fn timed_segments(body: &str, open: char, close: char) -> Vec<(i64, i64, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    let bytes = body.as_bytes();
    // Find the first header.
    while i < body.len() {
        if bytes[i] == open as u8 && seg_header(&body[i..], open, close).is_some() {
            break;
        }
        i += 1;
    }
    while i < body.len() {
        let Some((a, b, len)) = seg_header(&body[i..], open, close) else {
            break;
        };
        let text_start = i + len;
        let mut j = text_start;
        while j < body.len() {
            if bytes[j] == open as u8 && seg_header(&body[j..], open, close).is_some() {
                break;
            }
            j += 1;
        }
        out.push((a, b, body[text_start..j].to_string()));
        i = j;
    }
    out
}

/// Parses `[start,dur]` followed by segments, shared by YRC and KRC.
/// `relative`: segment starts are offsets from the line start (KRC) instead
/// of absolute (YRC).
fn parse_timed_lines(text: &str, open: char, close: char, relative: bool) -> Lyrics {
    let mut lines: Vec<Line> = Vec::new();
    for raw in text.lines() {
        let raw = raw.trim_start_matches('\u{feff}').trim();
        let Some(rest) = raw.strip_prefix('[') else {
            continue;
        };
        let Some(close_br) = rest.find(']') else {
            continue;
        };
        let mut nums = rest[..close_br].split(',');
        let (Some(Ok(start)), Some(Ok(dur))) = (
            nums.next().map(|v| v.trim().parse::<i64>()),
            nums.next().map(|v| v.trim().parse::<i64>()),
        ) else {
            continue;
        };
        let segs = timed_segments(&rest[close_br + 1..], open, close);
        let mut words = Vec::with_capacity(segs.len());
        for (a, b, s) in segs {
            if s.is_empty() {
                continue;
            }
            let ws = if relative { start + a } else { a };
            words.push(Word {
                t: ws.max(0) as u32,
                e: (ws + b).max(ws).max(0) as u32,
                text: s,
            });
        }
        let text: String = words.iter().map(|w| w.text.as_str()).collect();
        if text.trim().is_empty() {
            continue;
        }
        lines.push(Line {
            t: start.max(0) as u32,
            e: (start + dur).max(start).max(0) as u32,
            text: text.trim().to_string(),
            words,
        });
    }
    lines.sort_by_key(|l| l.t);
    let mut out = finish_keep_ends(lines);
    out.estimated = false;
    out
}

/// Like [`finish`] but keeps word ends the source gave us; only the line end
/// is extended to the next line's start.
fn finish_keep_ends(mut lines: Vec<Line>) -> Lyrics {
    let starts: Vec<u32> = lines.iter().map(|l| l.t).collect();
    for (i, line) in lines.iter_mut().enumerate() {
        if let Some(n) = starts.get(i + 1) {
            line.e = line.e.max(*n);
        }
    }
    let spoken = lines.iter().filter(|l| !l.text.is_empty()).count();
    let worded = lines.iter().filter(|l| l.words.len() >= 2).count();
    let sync = if spoken > 0 && worded * 2 >= spoken {
        Sync::Word
    } else {
        Sync::Line
    };
    Lyrics {
        sync,
        estimated: false,
        lines,
    }
}

/// NetEase Cloud Music `yrc`: `[start,dur](wstart,wdur,0)word...`, absolute ms.
pub fn parse_yrc(text: &str) -> Lyrics {
    parse_timed_lines(text, '(', ')', false)
}

/// Kugou `krc` text (after [`decode_krc`]): `[start,dur]<off,dur,0>word...`,
/// word offsets relative to the line start.
pub fn parse_krc(text: &str) -> Lyrics {
    parse_timed_lines(text, '<', '>', true)
}

/// Decodes a Kugou KRC download: `krc1` magic, then zlib data XOR-ed with a
/// fixed 16-byte key.
pub fn decode_krc(bytes: &[u8]) -> Option<String> {
    use std::io::Read;
    const KEY: [u8; 16] = [
        0x40, 0x47, 0x61, 0x77, 0x5e, 0x32, 0x74, 0x47, 0x51, 0x36, 0x31, 0x2d, 0xce, 0xd2, 0x6e,
        0x69,
    ];
    let body = bytes.strip_prefix(b"krc1")?;
    let xored: Vec<u8> = body
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ KEY[i % KEY.len()])
        .collect();
    let mut out = String::new();
    flate2::read::ZlibDecoder::new(xored.as_slice())
        .take(4 * 1024 * 1024)
        .read_to_string(&mut out)
        .ok()?;
    Some(out)
}

// ---------------------------------------------- YouTube captions ----

/// Parses a YouTube `json3` caption track. Auto-generated captions carry a
/// start offset per word, which gives word-level timing for any video that
/// has them.
pub fn parse_json3(v: &serde_json::Value) -> Lyrics {
    let mut lines: Vec<Line> = Vec::new();
    let events = v.get("events").and_then(|e| e.as_array());
    for ev in events.into_iter().flatten() {
        let Some(start) = ev.get("tStartMs").and_then(|t| t.as_i64()) else {
            continue;
        };
        let dur = ev.get("dDurationMs").and_then(|t| t.as_i64()).unwrap_or(0);
        let Some(segs) = ev.get("segs").and_then(|s| s.as_array()) else {
            continue;
        };
        let mut words: Vec<Word> = Vec::new();
        let mut pending_space = false;
        for seg in segs {
            let raw = seg.get("utf8").and_then(|u| u.as_str()).unwrap_or("");
            if raw.trim().is_empty() {
                pending_space = true;
                continue;
            }
            let off = seg.get("tOffsetMs").and_then(|t| t.as_i64()).unwrap_or(0);
            if pending_space || raw.starts_with(char::is_whitespace) {
                if let Some(prev) = words.last_mut() {
                    if !prev.text.ends_with(' ') {
                        prev.text.push(' ');
                    }
                }
            }
            pending_space = raw.ends_with(char::is_whitespace);
            let t = (start + off).max(0) as u32;
            words.push(Word {
                t,
                e: t,
                text: raw.trim().to_string(),
            });
        }
        if words.is_empty() {
            continue;
        }
        let text: String = words.iter().map(|w| w.text.as_str()).collect();
        let text = text.trim().to_string();
        if lines.last().is_some_and(|l| l.text == text) {
            continue; // rolling captions repeat the previous line
        }
        let line_t = start.max(0) as u32;
        let mut line = Line {
            t: line_t,
            e: (start + dur).max(start).max(0) as u32,
            text,
            words: if words.len() >= 2 { words } else { Vec::new() },
        };
        // A single-word event is a line-level caption.
        if line.words.is_empty() {
            line.e = line.e.max(line.t);
        }
        lines.push(line);
    }
    lines.sort_by_key(|l| l.t);
    finish(lines)
}

// ---------------------------------------------------- LRCLIB YAML ----

/// A YAML scalar: plain, `'single'` (with `''` as an escaped quote) or
/// `"double"` (JSON escapes).
fn yaml_scalar(v: &str) -> String {
    let v = v.trim();
    if let Some(inner) = v.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')) {
        return inner.replace("''", "'");
    }
    if v.starts_with('"') {
        if let Ok(s) = serde_json::from_str::<String>(v) {
            return s;
        }
    }
    v.to_string()
}

/// Parses LRCLIB's `lyricsfile` (a small YAML document) for tracks that have
/// word timing. Only the fixed shape LRCLIB emits is understood:
/// `lines:` of `text`/`start_ms`/`end_ms` with an optional `words:` list of
/// `text`/`start_ms`.
pub fn parse_lyricsfile(yaml: &str) -> Lyrics {
    let mut lines: Vec<Line> = Vec::new();
    let mut in_lines = false;
    let mut in_words = false;
    for raw in yaml.lines() {
        if raw.trim().is_empty() {
            continue;
        }
        if !raw.starts_with(' ') && !raw.starts_with('-') {
            in_lines = raw.trim_end() == "lines:";
            in_words = false;
            continue;
        }
        if !in_lines {
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        let body = raw.trim_start();
        let (is_item, body) = match body.strip_prefix("- ") {
            Some(b) => (true, b),
            None => (false, body),
        };
        let Some((key, val)) = body.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let val = val.trim();
        if is_item && indent == 0 && key == "text" {
            in_words = false;
            lines.push(Line {
                t: 0,
                e: 0,
                text: yaml_scalar(val),
                words: Vec::new(),
            });
        } else if key == "words" && !is_item {
            in_words = true;
        } else if is_item && in_words && key == "text" {
            if let Some(line) = lines.last_mut() {
                line.words.push(Word {
                    t: 0,
                    e: 0,
                    text: yaml_scalar(val),
                });
            }
        } else if key == "start_ms" || key == "end_ms" {
            let n = val.parse::<u32>().unwrap_or(0);
            if let Some(line) = lines.last_mut() {
                if in_words {
                    if let Some(w) = line.words.last_mut() {
                        if key == "start_ms" {
                            w.t = n;
                        } else {
                            w.e = n;
                        }
                    }
                } else if key == "start_ms" {
                    line.t = n;
                } else {
                    line.e = n;
                }
            }
        }
    }
    lines.retain(|l| !l.text.trim().is_empty() || l.t > 0);
    for line in &mut lines {
        line.text = line.text.trim().to_string();
        if line.words.len() < 2 {
            line.words.clear();
        }
    }
    lines.sort_by_key(|l| l.t);
    finish(lines)
}

// -------------------------------------------------- word estimation ----

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xac00..=0xd7af | 0xf900..=0xfaff)
}

/// Spreads each untimed-word line's time over its words (by length), so a
/// line-synced song can still be highlighted word by word. Lines that already
/// have words are left alone. A line never stretches past
/// `max(1.2 s, 280 ms per character)`, so an instrumental gap after it
/// doesn't slow the highlight to a crawl.
pub fn estimate_words(l: &mut Lyrics) {
    if l.sync == Sync::None {
        return;
    }
    let mut changed = false;
    for line in &mut l.lines {
        if !line.words.is_empty() || line.text.trim().is_empty() {
            continue;
        }
        let units = split_units(&line.text);
        if units.len() < 2 {
            continue;
        }
        let chars: u32 = units.iter().map(|u| u.trim().chars().count() as u32).sum();
        let span = line
            .e
            .saturating_sub(line.t)
            .min((chars * 280).max(1200))
            .max(1);
        let total_weight: u32 = units
            .iter()
            .map(|u| u.trim().chars().count().max(1) as u32)
            .sum();
        let mut acc = 0u32;
        let mut words = Vec::with_capacity(units.len());
        for u in units {
            let weight = u.trim().chars().count().max(1) as u32;
            let start = line.t + span * acc / total_weight;
            acc += weight;
            let end = line.t + span * acc / total_weight;
            words.push(Word {
                t: start,
                e: end,
                text: u,
            });
        }
        line.words = words;
        changed = true;
    }
    if changed {
        l.estimated = true;
    }
}

/// True when some sung line has no word timing, so `estimate_words` would
/// change it (used to decide whether a cached copy needs it on read).
pub fn has_unworded_lines(l: &Lyrics) -> bool {
    l.sync != Sync::None
        && l.lines.iter().any(|x| {
            !x.text.trim().is_empty() && x.words.is_empty() && split_units(&x.text).len() >= 2
        })
}

/// Takes word timing from `donor` (word-synced lyrics of the same song, often
/// a different length: the studio cut when the video has an intro or repeats a
/// section) and puts it on `base` (line-synced lyrics that fit this video).
/// Line starts stay as the video's own; each matched line gets the donor's
/// words at the same offsets from the line start.
///
/// Lines are aligned by their text with a longest-common-subsequence match, so
/// lines the two versions don't share (an extra chorus, a missing ad-lib line)
/// don't throw off the rest. Lines that don't match keep no words here, and
/// `estimate_words` fills them in. Returns `None` unless at least half of the
/// sung lines matched, so a different song never gets grafted.
pub fn graft_words(base: &Lyrics, donor: &Lyrics) -> Option<Lyrics> {
    const MAX_LINES: usize = 600;
    if base.sync != Sync::Line || donor.sync != Sync::Word {
        return None;
    }
    if base.lines.len() > MAX_LINES || donor.lines.len() > MAX_LINES {
        return None;
    }
    let bk: Vec<String> = base.lines.iter().map(|l| normalize(&l.text)).collect();
    let dk: Vec<String> = donor
        .lines
        .iter()
        .map(|l| {
            if l.words.len() >= 2 {
                normalize(&l.text)
            } else {
                String::new()
            }
        })
        .collect();
    let same = |i: usize, j: usize| -> bool {
        !bk[i].is_empty()
            && !dk[j].is_empty()
            && (bk[i] == dk[j] || similarity(&bk[i], &dk[j]) >= 0.85)
    };
    let (n, m) = (bk.len(), dk.len());
    // lcs[i][j]: best alignment of the first i base lines with the first j donor lines.
    let mut lcs = vec![vec![0u16; m + 1]; n + 1];
    for i in 1..=n {
        for j in 1..=m {
            lcs[i][j] = if same(i - 1, j - 1) {
                lcs[i - 1][j - 1] + 1
            } else {
                lcs[i - 1][j].max(lcs[i][j - 1])
            };
        }
    }
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 && j > 0 {
        if same(i - 1, j - 1) && lcs[i][j] == lcs[i - 1][j - 1] + 1 {
            pairs.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if lcs[i - 1][j] >= lcs[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    let mut out = base.clone();
    let sung = bk.iter().filter(|k| !k.is_empty()).count();
    let mut grafted = 0usize;
    for (bi, dj) in pairs {
        let line = &mut out.lines[bi];
        let d = &donor.lines[dj];
        let first = d.words[0].t;
        let span = d
            .words
            .last()
            .map(|w| w.e)
            .unwrap_or(first)
            .saturating_sub(first);
        let room = line.e.saturating_sub(line.t);
        // Must fit in the time this video gives the line (a little slack).
        if u64::from(span) > u64::from(room) * 13 / 10 + 500 {
            continue;
        }
        line.words = d
            .words
            .iter()
            .map(|w| Word {
                t: line.t + w.t.saturating_sub(first),
                e: (line.t + w.e.saturating_sub(first)).min(line.e.max(line.t + 1)),
                text: w.text.clone(),
            })
            .collect();
        grafted += 1;
    }
    if sung == 0 || grafted * 2 < sung {
        return None;
    }
    out.sync = Sync::Word;
    out.estimated = false;
    Some(out)
}

/// Whether a recording's length is close enough to the video's to be the same
/// song for borrowing word timing: between 0.6 and 1.7 times as long.
pub fn duration_is_similar(video_ms: u32, got_ms: u32) -> bool {
    video_ms > 0
        && got_ms > 0
        && u64::from(got_ms) * 10 >= u64::from(video_ms) * 6
        && u64::from(got_ms) * 10 <= u64::from(video_ms) * 17
}

/// Splits a line into highlight units: words with their trailing space, or
/// single characters for CJK text.
fn split_units(text: &str) -> Vec<String> {
    let mut units: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in text.trim().chars() {
        if is_cjk(c) {
            if !cur.is_empty() {
                units.push(std::mem::take(&mut cur));
            }
            units.push(c.to_string());
        } else if c.is_whitespace() {
            if cur.is_empty() {
                if let Some(last) = units.last_mut() {
                    if !last.ends_with(' ') {
                        last.push(' ');
                    }
                }
            } else {
                cur.push(' ');
                units.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        units.push(cur);
    }
    units
}

/// Drops the credit lines some catalogs put before the lyrics ("Lyrics by:
/// ...", "作曲 : ...", "Title - Artist"). Only leading lines in the first 20
/// seconds are touched.
pub fn strip_credits(l: &mut Lyrics) {
    const KEYS: &[&str] = &[
        "lyrics", "lyricist", "composer", "composed", "produced", "producer", "written",
        "arranged", "music by", "words by", "作词", "作曲", "编曲", "制作", "词", "曲", "歌手",
        "作詞", "作曲",
    ];
    let credit = |line: &Line| -> bool {
        let t = line.text.to_lowercase();
        (t.contains(':') || t.contains('：')) && KEYS.iter().any(|k| t.trim_start().starts_with(k))
    };
    let mut cut = 0;
    for (i, line) in l.lines.iter().enumerate() {
        // A "Title - Artist" header line counts when credits follow it.
        let header = i == 0 && line.text.contains(" - ") && l.lines.get(1).is_some_and(credit);
        if line.t <= 20_000 && (header || credit(line) || line.text.is_empty()) {
            cut += 1;
        } else {
            break;
        }
    }
    l.lines.drain(..cut);
}

// --------------------------------------------------------- matching ----

/// Lowercases and strips what differs between catalogs for the same song:
/// bracketed asides, "feat. ...", "remastered", punctuation.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0u32;
    for c in s.chars() {
        match c {
            '(' | '[' | '{' | '（' | '【' | '「' => depth += 1,
            ')' | ']' | '}' | '）' | '】' | '」' => depth = depth.saturating_sub(1),
            _ if depth > 0 => {}
            c if c.is_alphanumeric() => out.extend(c.to_lowercase()),
            _ => out.push(' '),
        }
    }
    let mut tokens: Vec<&str> = out.split_whitespace().collect();
    if let Some(i) = tokens
        .iter()
        .position(|t| matches!(*t, "feat" | "ft" | "featuring"))
    {
        tokens.truncate(i);
    }
    tokens.retain(|t| !matches!(*t, "remastered" | "remaster" | "official" | "lyrics"));
    tokens.join(" ")
}

/// 0.0 to 1.0: how alike two already-normalized strings are.
fn similarity(a: &str, b: &str) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    if a.contains(b) || b.contains(a) {
        return 0.85;
    }
    let ta: std::collections::HashSet<&str> = a.split(' ').collect();
    let tb: std::collections::HashSet<&str> = b.split(' ').collect();
    let common = ta.intersection(&tb).count() as f32;
    common / ta.len().max(tb.len()) as f32
}

/// How well a candidate's title and artist match the wanted ones, 0.0 to 1.0.
/// Artist counts less than title because catalogs spell artists (and
/// "A & B" credits) many ways. An empty wanted artist is ignored.
pub fn match_score(want_title: &str, want_artist: &str, got_title: &str, got_artist: &str) -> f32 {
    let title = similarity(&normalize(want_title), &normalize(got_title));
    let wa = normalize(want_artist);
    if wa.is_empty() {
        return title;
    }
    let artist = similarity(&wa, &normalize(got_artist));
    title * 0.7 + artist * 0.3
}

/// Whether two durations (ms) are plausibly the same recording. Unknown
/// (zero) on either side passes.
pub fn duration_close(want_ms: u32, got_ms: u32, tolerance_ms: u32) -> bool {
    want_ms == 0 || got_ms == 0 || want_ms.abs_diff(got_ms) <= tolerance_ms
}

/// Guesses (artist, title) from a YouTube video title and channel name.
/// "Artist - Title (Official Video)" is the common case; auto-generated
/// "Artist - Topic" channels carry the artist with the title untouched.
pub fn clean_youtube_title(title: &str, channel: &str) -> (String, String) {
    let t = strip_noise_brackets(title);
    let ch = channel
        .trim()
        .trim_end_matches(" - Topic")
        .trim_end_matches("VEVO")
        .trim_end_matches("Vevo")
        .trim_end_matches(" Official")
        .trim()
        .to_string();
    for sep in [" - ", " – ", " — ", " ~ ", " | "] {
        if let Some((a, b)) = t.split_once(sep) {
            let (a, b) = (a.trim(), b.trim());
            if a.is_empty() || b.is_empty() {
                continue;
            }
            // "Series Opening 1 | Song" or "Song | Creditless | 4K": when some
            // parts only describe the video, the rest is the song, with no
            // artist named in the title.
            if sep == " | " {
                if let Some(song) = pipe_song(&t) {
                    return (ch, song);
                }
            }
            return (a.to_string(), song_part(b));
        }
    }
    (ch, song_part(t.trim()))
}

/// Words that describe a video rather than name a song.
const DESCRIPTOR_WORDS: &[&str] = &[
    "opening",
    "ending",
    "op",
    "ed",
    "ost",
    "theme",
    "creditless",
    "lyrics",
    "romaji",
    "english",
    "subtitles",
    "subbed",
    "amv",
    "full",
    "4k",
    "60fps",
    "hd",
    "mv",
    "pv",
    "tv",
    "size",
    "ver",
    "version",
    "audio",
    "music",
    "video",
    "official",
    "season",
];

/// True when `s` is made of descriptor words and numbers only, or contains
/// "opening"/"ending" style markers.
fn is_descriptor(s: &str) -> bool {
    let n = normalize(s);
    let words: Vec<&str> = n.split(' ').filter(|w| !w.is_empty()).collect();
    !words.is_empty()
        && words.iter().any(|w| {
            matches!(
                *w,
                "opening" | "ending" | "op" | "ed" | "ost" | "creditless" | "amv"
            )
        })
}

/// From "Opening 1 | Redo | 4K" style text keep the part that is the song.
fn song_part(title: &str) -> String {
    let title = title.split(" #").next().unwrap_or(title).trim().to_string();
    let parts: Vec<&str> = title
        .split(" | ")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() < 2 {
        return title;
    }
    let plain: Vec<&&str> = parts
        .iter()
        .filter(|p| !is_descriptor(p) && !only_descriptor_words(p))
        .collect();
    match plain.last() {
        Some(p)
            if parts
                .iter()
                .any(|p| is_descriptor(p) || only_descriptor_words(p)) =>
        {
            p.to_string()
        }
        _ => parts[0].to_string(),
    }
}

/// The song among `|`-separated parts when at least one part is only a
/// description of the video; `None` when nothing looks like a description.
fn pipe_song(title: &str) -> Option<String> {
    let parts: Vec<&str> = title
        .split(" | ")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let song: Vec<&&str> = parts
        .iter()
        .filter(|p| !is_descriptor(p) && !only_descriptor_words(p))
        .collect();
    (parts.len() >= 2 && !song.is_empty() && song.len() < parts.len())
        .then(|| song[song.len() - 1].to_string())
}

fn only_descriptor_words(s: &str) -> bool {
    let n = normalize(s);
    let mut any = false;
    for w in n.split(' ').filter(|w| !w.is_empty()) {
        any = true;
        if !(DESCRIPTOR_WORDS.contains(&w) || w.chars().all(|c| c.is_ascii_digit())) {
            return false;
        }
    }
    any
}

/// Text inside quotes or Japanese brackets: `"Redo"`, `『Redo』`, `「Redo」`.
fn quoted_parts(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (open, close) in [('「', '」'), ('『', '』'), ('“', '”'), ('"', '"')] {
        let mut rest = s;
        while let Some(i) = rest.find(open) {
            let after = &rest[i + open.len_utf8()..];
            let Some(j) = after.find(close) else { break };
            let inner = after[..j].trim();
            if inner.chars().count() >= 2 {
                out.push(inner.to_string());
            }
            rest = &after[j + close.len_utf8()..];
        }
    }
    out
}

/// Other (title, artist) readings of a video title, best first, for when the
/// obvious one finds no lyrics: the same title without the artist (catalogs
/// credit artists in other languages or spellings), each `|` part and each
/// quoted part as a title, and the artist and title swapped.
pub fn query_guesses(title: &str, artist: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![(title.to_string(), artist.to_string())];
    if !artist.is_empty() {
        out.push((title.to_string(), String::new()));
    }
    let mut segments: Vec<String> = quoted_parts(title);
    segments.extend(
        title
            .split(" | ")
            .map(|p| p.trim().to_string())
            .filter(|p| p.chars().count() >= 2),
    );
    for seg in segments {
        if !is_descriptor(&seg) && !only_descriptor_words(&seg) {
            out.push((seg, String::new()));
        }
    }
    if !artist.is_empty() {
        out.push((artist.to_string(), title.to_string()));
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|(t, a)| {
        let key = (normalize(t), normalize(a));
        !key.0.is_empty() && seen.insert(key)
    });
    out.truncate(5);
    out
}

/// A recording much longer than the video is plausibly the full version of a
/// TV-size or clipped upload: up to four times as long, never shorter by more
/// than the usual tolerance.
pub fn duration_is_full_version(video_ms: u32, got_ms: u32) -> bool {
    video_ms > 0
        && got_ms > 0
        && got_ms + 4000 >= video_ms
        && u64::from(got_ms) <= u64::from(video_ms) * 4
}

fn strip_noise_brackets(s: &str) -> String {
    const NOISE: &[&str] = &[
        "official",
        "video",
        "audio",
        "lyric",
        "lyrics",
        "hd",
        "4k",
        "hq",
        "visualizer",
        "music video",
        "mv",
        "m/v",
        "full song",
        "explicit",
    ];
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        let close = match c {
            '(' => ')',
            '[' => ']',
            '【' => '】',
            _ => {
                out.push(c);
                continue;
            }
        };
        let mut inner = String::new();
        let mut closed = false;
        for n in chars.by_ref() {
            if n == close {
                closed = true;
                break;
            }
            inner.push(n);
        }
        let lower = inner.to_lowercase();
        let noisy = NOISE
            .iter()
            .any(|w| lower.split_whitespace().any(|t| t == *w));
        if !closed || !noisy {
            out.push(c);
            out.push_str(&inner);
            if closed {
                out.push(close);
            }
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_line_lrc_with_multiple_stamps_and_offset() {
        let l =
            parse_lrc("[ar:Someone]\n[offset:500]\n[00:10.00][00:30.50]La la\n[00:20.00]Na na\n");
        assert_eq!(l.sync, Sync::Line);
        let starts: Vec<u32> = l.lines.iter().map(|x| x.t).collect();
        assert_eq!(starts, vec![9500, 19500, 30000]);
        assert_eq!(l.lines[0].text, "La la");
        assert_eq!(l.lines[0].e, 19500);
    }

    #[test]
    fn parses_enhanced_lrc_words() {
        let l = parse_lrc("[00:01.00]<00:01.00>Hel<00:01.30>lo <00:01.80>world\n[00:04.00]<00:04.00>Next <00:04.50>one\n");
        assert_eq!(l.sync, Sync::Word);
        assert_eq!(l.lines[0].text, "Hello world");
        assert_eq!(l.lines[0].words.len(), 3);
        assert_eq!(l.lines[0].words[1].t, 1300);
        assert_eq!(l.lines[0].words[0].e, 1300);
    }

    #[test]
    fn untimed_text_is_plain() {
        let l = parse_lrc("one\ntwo\n\nthree");
        assert_eq!(l.sync, Sync::None);
        assert_eq!(l.lines.len(), 3);
        assert_eq!(parse_plain("\n\nA\nB\n\n").lines.len(), 2);
    }

    #[test]
    fn enhanced_lrc_round_trips() {
        let src = parse_lrc("[00:01.000]<00:01.000>Hel<00:01.300>lo <00:01.800>world\n[00:04.000]<00:04.000>Next <00:04.500>one\n");
        let text = to_enhanced_lrc(&src);
        let back = parse_lrc(&text);
        assert_eq!(back.sync, Sync::Word);
        assert_eq!(back.lines.len(), 2);
        assert_eq!(back.lines[0].words, src.lines[0].words);
        let plain = parse_plain("a\nb");
        assert_eq!(from_cached(&to_enhanced_lrc(&plain), "none", false), plain);
    }

    #[test]
    fn parses_netease_yrc() {
        let yrc = "{\"t\":0,\"c\":[{\"tx\":\"credits\"}]}\n[1000,2000](1000,500,0)Hel(1500,500,0)lo (2000,1000,0)you\n[5000,1500](5000,700,0)Again (5700,800,0)(oh)\n";
        let l = parse_yrc(yrc);
        assert_eq!(l.sync, Sync::Word);
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.lines[0].text, "Hello you");
        assert_eq!(l.lines[0].words[2].t, 2000);
        assert_eq!(l.lines[0].words[2].e, 3000);
        assert_eq!(l.lines[1].words[1].text, "(oh)");
    }

    #[test]
    fn decodes_and_parses_krc() {
        use std::io::Write;
        let krc = "[id:$00000000]\n[language:e30=]\n[1000,2000]<0,500,0>Hel<500,500,0>lo <1000,1000,0>you\n";
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(krc.as_bytes()).unwrap();
        let comp = z.finish().unwrap();
        const KEY: [u8; 16] = [
            0x40, 0x47, 0x61, 0x77, 0x5e, 0x32, 0x74, 0x47, 0x51, 0x36, 0x31, 0x2d, 0xce, 0xd2,
            0x6e, 0x69,
        ];
        let mut blob = b"krc1".to_vec();
        blob.extend(comp.iter().enumerate().map(|(i, b)| b ^ KEY[i % 16]));
        let text = decode_krc(&blob).unwrap();
        let l = parse_krc(&text);
        assert_eq!(l.sync, Sync::Word);
        assert_eq!(l.lines[0].words[1].t, 1500);
        assert_eq!(l.lines[0].text, "Hello you");
        assert!(decode_krc(b"nope").is_none());
    }

    #[test]
    fn parses_youtube_auto_caption_json3() {
        let v = serde_json::json!({"events":[
            {"tStartMs":1000,"dDurationMs":3000,"segs":[
                {"utf8":"hello","tOffsetMs":0},{"utf8":" there","tOffsetMs":600},{"utf8":" friend","tOffsetMs":1200}]},
            {"tStartMs":4000,"dDurationMs":100,"segs":[{"utf8":"\n"}]},
            {"tStartMs":5000,"dDurationMs":2000,"segs":[{"utf8":"manual line"}]}
        ]});
        let l = parse_json3(&v);
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.lines[0].text, "hello there friend");
        assert_eq!(l.lines[0].words[1].t, 1600);
        assert!(l.lines[1].words.is_empty());
    }

    #[test]
    fn estimates_words_inside_the_line_window() {
        let mut l = parse_lrc("[00:01.00]one two three\n[00:30.00]end\n");
        estimate_words(&mut l);
        assert!(l.estimated);
        let w = &l.lines[0].words;
        assert_eq!(w.len(), 3);
        assert_eq!(w[0].t, 1000);
        // 11 chars => 3.08 s window, nowhere near the 29 s gap.
        assert!(w[2].e < 5000, "{}", w[2].e);
        assert!(w.windows(2).all(|p| p[0].e == p[1].t));
        let mut cjk = parse_lrc("[00:01.00]你好世界\n[00:05.00]x\n");
        estimate_words(&mut cjk);
        assert_eq!(cjk.lines[0].words.len(), 4);
    }

    #[test]
    fn parses_lrclib_lyricsfile() {
        let y = "version: '1.0'\nmetadata:\n  title: T\nlines:\n- text: We were both young\n  start_ms: 16290\n  end_ms: 19585\n  words:\n  - text: 'We '\n    start_ms: 16290\n  - text: 'were '\n    start_ms: 16547\n  - text: young\n    start_ms: 17277\n- text: I'm standing\n  start_ms: 23610\n  end_ms: 25318\n  words:\n  - text: 'I''m '\n    start_ms: 23610\n  - text: standing\n    start_ms: 23778\n";
        let l = parse_lyricsfile(y);
        assert_eq!(l.sync, Sync::Word);
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.lines[0].words[1].text, "were ");
        assert_eq!(l.lines[0].words[1].t, 16547);
        assert_eq!(l.lines[0].words[0].e, 16547);
        assert_eq!(l.lines[1].words[0].text, "I'm ");
        assert_eq!(l.lines[1].text, "I'm standing");
    }

    #[test]
    fn strips_leading_credit_lines() {
        let mut l = parse_lrc("[00:00.00]Song - Queen\n[00:00.09]Lyrics by：Freddie\n[00:00.18]Composed by：Freddie\n[00:12.00]Is this the real life\n[00:15.00]Composed by: later\n");
        strip_credits(&mut l);
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.lines[0].text, "Is this the real life");
        let mut l = parse_lrc("[00:00.09]Lyrics by：Freddie\n[00:00.18]作曲 : Freddie\n[00:12.00]Is this the real life\n");
        strip_credits(&mut l);
        assert_eq!(l.lines.len(), 1);
        assert_eq!(l.lines[0].text, "Is this the real life");
    }

    #[test]
    fn matching_ignores_decoration() {
        assert_eq!(
            normalize("Bohemian Rhapsody (Remastered 2011)"),
            "bohemian rhapsody"
        );
        assert_eq!(normalize("Song feat. Other Guy"), "song");
        assert!(
            match_score(
                "Bohemian Rhapsody",
                "Queen",
                "Bohemian Rhapsody - Remastered",
                "Queen"
            ) > 0.8
        );
        assert!(
            match_score(
                "Bohemian Rhapsody",
                "Queen",
                "Another One Bites the Dust",
                "Queen"
            ) < 0.5
        );
        assert!(duration_close(354_000, 356_500, 3000));
        assert!(!duration_close(354_000, 200_000, 3000));
        assert!(duration_close(0, 200_000, 3000));
    }

    #[test]
    fn handles_series_and_descriptor_titles() {
        assert_eq!(
            clean_youtube_title(
                "Re:ZERO - Starting Life in Another World Opening 1 | Redo",
                "Crunchyroll"
            ),
            ("Re:ZERO".into(), "Redo".into())
        );
        assert_eq!(
            clean_youtube_title(
                "Starting Life in Another World Opening 1 | Redo",
                "Crunchyroll"
            ),
            ("Crunchyroll".into(), "Redo".into())
        );
        assert_eq!(
            clean_youtube_title(
                "Redo | Creditless | 4K | English / Romaji Subtitles",
                "Bruh Clan ++"
            ),
            ("Bruh Clan ++".into(), "Redo".into())
        );
        assert_eq!(
            clean_youtube_title("Artist - Song Name | Live #shorts", "x").1,
            "Song Name"
        );
        let g = query_guesses("Redo", "Re:ZERO");
        assert_eq!(g[0], ("Redo".to_string(), "Re:ZERO".to_string()));
        assert!(g.contains(&("Redo".to_string(), String::new())));
        let g = query_guesses("Zvlh \"Redo\" Special", "X");
        assert!(g.contains(&("Redo".to_string(), String::new())));
        let g = query_guesses("『Redo』4K 60FPS", "Re:Zero OPENING 1");
        assert!(g.contains(&("Redo".to_string(), String::new())));
        assert!(g.len() <= 5);
        assert!(duration_is_full_version(91_000, 260_000));
        assert!(!duration_is_full_version(91_000, 500_000));
        assert!(!duration_is_full_version(260_000, 91_000));
        assert!(!duration_is_full_version(0, 91_000));
    }

    #[test]
    fn grafts_word_timing_onto_a_longer_videos_lines() {
        // Video has a 30 s intro the studio cut doesn't.
        let base = parse_lrc(
            "[00:40.00]Hello there my friend\n[00:45.00]Second line here\n[00:50.00]Last one now\n",
        );
        let donor = parse_lrc("[00:10.00]<00:10.00>Hello <00:10.50>there <00:11.00>my <00:11.50>friend\n[00:15.00]<00:15.00>Second <00:15.60>line <00:16.20>here\n[00:20.00]<00:20.00>Last <00:20.50>one <00:21.00>now\n");
        assert_eq!(base.sync, Sync::Line);
        let g = graft_words(&base, &donor).unwrap();
        assert_eq!(g.sync, Sync::Word);
        assert_eq!(g.lines[0].t, 40_000, "line starts stay the video's own");
        assert_eq!(g.lines[0].words[1].t, 40_500);
        assert_eq!(g.lines[1].words[2].t, 46_200);
        assert_eq!(g.lines[0].words[0].text, "Hello ");
        // A different song doesn't graft.
        let other = parse_lrc("[00:10.00]<00:10.00>Totally <00:10.50>different <00:11.00>words\n");
        assert!(graft_words(&base, &other).is_none());
        // Words that can't fit in the line's time are skipped, and with too few matches it is refused.
        let tight = parse_lrc(
            "[00:40.00]Hello there my friend\n[00:40.50]Second line here\n[00:41.00]Last one now\n",
        );
        assert!(graft_words(&tight, &donor).is_none());
        // An extra section in the video's version doesn't break the alignment.
        let longer = parse_lrc("[00:40.00]Hello there my friend\n[00:45.00]An extra line only the video has\n[00:50.00]Second line here\n[00:55.00]Last one now\n");
        let g = graft_words(&longer, &donor).unwrap();
        assert!(
            g.lines[1].words.is_empty(),
            "the extra line gets no borrowed words"
        );
        assert_eq!(g.lines[2].words[1].t, 50_600);
        assert_eq!(g.lines[3].words[1].t, 55_500);
        assert!(duration_is_similar(239_000, 206_000));
        assert!(!duration_is_similar(239_000, 90_000));
        assert!(!duration_is_similar(90_000, 260_000));
    }

    #[test]
    fn cleans_youtube_titles() {
        assert_eq!(
            clean_youtube_title(
                "Queen – Bohemian Rhapsody (Official Video Remastered)",
                "Queen Official"
            ),
            ("Queen".into(), "Bohemian Rhapsody".into())
        );
        assert_eq!(
            clean_youtube_title("Blinding Lights", "The Weeknd - Topic"),
            ("The Weeknd".into(), "Blinding Lights".into())
        );
        assert_eq!(
            clean_youtube_title("Song Name [Lyrics] (feat. Guest)", "SomeVEVO"),
            ("Some".into(), "Song Name (feat. Guest)".into())
        );
    }
}
