# Music player (`/music/`)

Signed-in users search for songs, play them, and watch synced lyrics. A small
card follows the user to every other page so they can pause from anywhere, and
the player can be popped out into its own window. The card's eye button hides it
without stopping the music (it comes back when a different song starts), the X
stops the music, and Preferences > Games > Music player card turns it off for
good (`_prefDisplay.musicCard`).

Code: `rust/crates/mitch-server/src/routes/music/` (API), `mitch-lib/src/music.rs`
(favorites, lyrics cache), `mitch-lib/src/music_playlists.rs` (playlists),
`mitch-lib/src/music_lyrics.rs` (lyrics formats and matching),
`webserver/music-core.js`, `music-widget.js/.css`, `webserver/music/`.

## Discover

The Discover tab (`routes/music/discover.rs`, `GET /api/music/discover`) has three
parts:

- **Browse by genre**: 18 genres (`GENRES`). A genre is one YouTube search such as
  "top hip hop songs 2026" (the year is added for the chart genres), answered from
  the same search cache as the Search tab, so repeat visits are instant.
  `GET /api/music/discover?genre=<id>` returns one genre's songs.
- **Top songs**: if `MUSIC_TOP_PLAYLIST` is set to a YouTube playlist id (an
  official chart playlist, for example), its first 25 songs, kept for six hours.
  Otherwise two YouTube searches for this year's hits, interleaved. Search results
  are only as good as YouTube's ranking, so set the chart playlist for a
  trustworthy list.
- **Popular on mitch.pro**: songs kept (in a playlist or the library) by at least
  three different accounts, most kept first. The response has the songs only: no
  account, no count, and a song one or two people keep never appears. Nothing about
  listening is recorded.

If the music source is down, Top songs is empty and the other sections still work.
Songs listed here are warmed like search results, so they start quickly.

## Playlists

Any signed-in user can make up to 100 playlists of up to 500 songs. Tables:
`music_playlists`, `music_playlist_tracks`, `music_track_matches` (all erased
with the account except the shared match cache, which holds no user data).

| | Who can open it |
| --- | --- |
| **Public** | any signed-in user; listed in the Public tab (searchable by name) |
| **Private** | only the owner |
| **Private with a share link** | anyone signed in who has the link (`/music/?playlist=<id>&t=<token>`). "Stop sharing" clears the token, which kills every old link |

Only the owner edits, renames, reorders, deletes, syncs or sees the token. A
playlist someone can't open answers 404, the same as a missing one. A viewer can
play it or **Save a copy** (always private, never linked to Spotify). The API
returns the owner's display name, photo and equipped color/badge, never their
email; list responses leave out an uploaded (data URL) photo because of its size.
Endpoints are listed at the top of `routes/music/playlists.rs`.

### Sharing over chat

"Message someone" on a playlist opens `/matrix/?mshare=<link>&mname=<name>`.
`webserver/matrix/music-share.js` (loaded by the chat page) waits for the chat
client to sync and shows a "Send playlist to a person" prompt:

1. Pick one person: people the user already has a DM with (from the `m.direct`
   account data), or a user-directory search (2+ characters). The user's own
   account is left out.
2. A draft message (`🎵 <name> <link>`) appears in an editable box. Nothing is sent
   until they press **Send**.
3. Send uses their existing DM with that person, or creates a new private,
   encrypted DM (`is_direct`, `trusted_private_chat`, `m.megolm.v1.aes-sha2`) and
   records it in `m.direct` so the chat lists it as a DM.

It never lists or posts to rooms or channels. Sending is the client's own
`sendTextMessage`, so encryption is handled by the client and the server never
sees the message. The link must be on this site's `/music/` with a `playlist`
parameter, otherwise nothing is shown. The prompt is its own overlay; the script
doesn't touch the chat UI. The recipient clicks the link and lands on the
playlist page (they need to be signed in; for a private playlist the link's token
is what lets them in).

### Spotify import and sync

"Import from Spotify" takes a link to a **public** playlist and needs no
Spotify account or API key: the server reads Spotify's public embed page for
the playlist (`routes/music/spotify.rs`; only `open.spotify.com` is fetched). So:

- only public playlists work, and Spotify shares only the first 100 songs;
- no audio comes from Spotify. Each song is matched to a YouTube video the first
  time it is played (`GET /api/music/match`: search `artist title`, accept a result
  that (1) is within 4 s of the Spotify length, (2) is by the same artist
  (`artist_match`: the Spotify artists against the video's artist and title, at
  least 0.6, so a cover by someone else is out however well the title matches),
  (3) has a title that scores at least 0.5, or an artist match of 0.9+ with the
  length within 2 s (an official upload whose title is in another script), and
  (4) isn't a different version: a result with live, cover, remix, acoustic,
  slowed, instrumental, karaoke, English/other-language, dub, fandub, lofi,
  reprise... in its title is skipped unless the Spotify title has the word too
  (`VERSION_WORDS`). With no qualifying result the song stays unmatched rather
  than getting the wrong recording. Matches are stored in
  `music_track_matches` and shared by everyone, each with the `rules_version` it
  was made under (`MATCH_RULES_VERSION`, now 3, for reference only). **Matches are
  permanent**: a song keeps the first video it was matched to, so a playlist never
  waits on a new search, and changing the rules does not discard anything (raising
  the version once made every song re-match and made playlists very slow). To redo
  one on purpose, delete its `music_track_matches` row;
- **Resync from Spotify** (in the playlist's ⋯ menu, and an automatic sync when the
  owner opens a playlist more than a day stale) makes the Spotify part of the
  playlist equal Spotify's current list and order, keeps every stored match, and keeps songs the owner added
  by hand after it. Songs removed on Spotify disappear. The response says how
  many songs are being rechecked (`rechecked`, always 0 now).
- Reading the embed page is not an official API. If Spotify changes it,
  `parse_embed` is the one place to fix. Private playlists, Liked Songs and
  playlists past 100 songs would need a registered Spotify app and OAuth, which
  this doesn't have.

## Where the audio comes from

YouTube, through [yt-dlp](https://github.com/yt-dlp/yt-dlp) on the server.
Songs people keep are saved (next section); everything else is streamed live and not stored. For a track the server asks yt-dlp for a short-lived signed
URL on Google's CDN, caches it (`ytdlp::RESOLVE_TTL`, 4 h), and proxies the
bytes to the browser. Everything goes through this server (audio, cover art,
lyrics), so a network that blocks YouTube or the lyric sites still works and no
third party sees the visitor.

### Saved songs

Every song in a user's playlists or library is downloaded once and kept, so it
plays without a YouTube lookup and its link never expires. Code:
`mitch-lib/src/music_audio.rs` (what is saved, quotas), `routes/music/saved.rs`
(downloading, serving, cleanup) and `routes/music/audio_store.rs` (where the
bytes go). Tables: `music_audio_files`, `music_audio_failures`.

- **Quota: 512 MB per user** (`USER_QUOTA_BYTES`) of distinct songs in their
  playlists and library. Files are shared, so a song several people keep is stored
  once and counts for each of them. Before a download the server checks, by
  estimated size (about 17 KB per second), that some keeper has room; after it, by
  the real size. A song nobody has room for stays unsaved and streams live as
  before; failures are retried after 1 h, 2 h, ... at most 24 h.
- **Disk cap**: all saved audio together may not pass 50 GB (`MUSIC_AUDIO_TOTAL_MB` to change), so many users can't fill the disk. Past it nothing new is saved. Urgent saves for upcoming songs run two at a time at most.
- **Saver**: a background loop (`saved::spawn`) takes three songs at a time, four
  seconds apart, in no particular order. It stops
  for 15 minutes when YouTube blocks the server. Files over 80 MB are skipped.
- **Up next**: while a song plays, the player warms the next three queue entries
  (`warmUpcoming` in `music-core.js`, from 2 s in and again when a song starts):
  Spotify songs are matched to a video, the stream link is resolved
  (`GET /api/music/prefetch`), and a song someone keeps that isn't saved yet is
  saved right away (`saved::save_soon`) instead of waiting for the loop. Each queue
  entry is warmed once.
- **Playing**: `GET /api/music/stream` serves a saved song from the store with
  `Range` support, and falls back to the live path if the store can't be read (a
  file missing from the local folder is forgotten so it is saved again).
- **Cleanup**: a file nobody keeps (playlist deleted, song removed from every
  list) is deleted within a minute or so.
- **Usage line**: the Mine tab of Playlists shows "Saved on the server: X of
  512 MB (n of m songs)" from `GET /api/music/storage`.

#### Where the files live

Chosen at startup by `MUSIC_AUDIO_STORE`, with credentials in
`MUSIC_AUDIO_STORE_USER` and `MUSIC_AUDIO_STORE_PASS`. They are ordinary
environment variables, so the secret manager injects them like the rest
(`docs/secrets-providers.md`).

| `MUSIC_AUDIO_STORE` | Backend |
| --- | --- |
| unset or `local` | `<data dir>/music-audio` |
| `local:/path` | any folder. For NFS or SMB, mount the share on the host (and into the container) and point this at it |
| `webdav:https://host/path` (http only on a private network, since the password goes with every request) | WebDAV, which covers Nextcloud: `webdav:https://cloud.example.com/remote.php/dav/files/USER/mitch-music` with a Nextcloud app password as `MUSIC_AUDIO_STORE_PASS`. The folder is created if missing |

Other schemes (`s3:`, `gdrive:`, `sftp:`) are not built; the server logs it and
uses the local folder. To add one: a new `AudioStore` variant with `put`, `read`
and `delete`, and its scheme in `AudioStore::from_env`. Nothing else changes.
Changing the backend does not move existing files: songs missing from the new
place are not found, so they are downloaded again by the saver only if their
database row is deleted (`DELETE FROM music_audio_files`).

Not verified: the WebDAV backend has not been run against a real Nextcloud (only
the local folder is tested); the saver has not run against live YouTube here.

### Keeping links ready

Playback starts fast because links are resolved before anyone clicks:

- **Search results**: as soon as a search returns, the server resolves a stream
  link for every result in the background (`ytdlp::warm`, two at a time, with its
  own queue so a click never waits behind it; it stops if the source is blocked).
- **Playlists**: opening, creating, adding to, importing or syncing a playlist
  gets its songs ready the same way (`worker::warm_playlist`). Spotify songs are
  matched to a YouTube video first, and the match is written into every playlist
  row that holds the song, so they play without a lookup.
- **Stored in the database**: every resolved link goes into `music_stream_links`
  (and an in-memory copy), so links survive a restart. A stored link is only handed
  out with at least 10 minutes of life left.
- **Kept fresh**: Google's links last about six hours and only work from the IP
  that asked, so they can't be kept forever. For every song that is in a playlist,
  a background worker (`routes/music/worker.rs`, every 5 minutes) re-resolves any
  link missing or lapsing within 45 minutes (60 per cycle, one at a time). Links
  stay as long as any playlist holds the song. Deleting a playlist removes the
  links of songs no other playlist has, and expired links nothing needs are pruned.
- A 403/410 from the CDN drops the link and resolves again once.

The play button shows a spinner (and the song's row and the card show "Loading…")
from the moment a song is chosen until sound starts, and ignores presses meanwhile.

Time from click to sound when nothing was ready: yt-dlp resolution (2 to 5 s)
plus the first bytes (about 1 s); with a ready link, under a second. The old
note follows for reference:

Time from click to sound, uncached: yt-dlp resolution (2 to 5 s) plus the first
bytes (about 1 s). A cached URL starts in under a second. While a track plays,
the next queue entry is resolved in the background (`/api/music/prefetch`), so
queue advances are near-instant. Search takes about 1.4 s (`--flat-playlist`).

### Setup the server needs

- `yt-dlp` on `PATH` (or `MUSIC_YTDLP_BIN`). The production image (`rust/Dockerfile`)
  installs a pinned release of the standalone musl build, checked against a pinned
  SHA-256, plus `nodejs`, which yt-dlp needs as its JavaScript runtime for YouTube
  (`MUSIC_YTDLP_JS_RUNTIME=node` is set in the image). YouTube changes often, so
  when playback starts failing, bump `YTDLP_VERSION` and both hashes together
  (take the hashes from the release's `SHA2-256SUMS`). Without yt-dlp every music
  endpoint that needs it answers 503 "music source is not set up on this server".
- A datacenter IP is often answered with "Sign in to confirm you're not a bot"
  (this is what the sandbox the feature was built in gets). Set
  `MUSIC_EGRESS_PROXY` to an HTTP proxy that leaves through a clean connection;
  yt-dlp and the byte proxy both use it, which they must, because the signed URL
  is bound to the IP that requested it. The existing `movies-egress` sidecar only
  allows `CONNECT` to its three Movies hosts (`egress-proxy/proxy.py`), so using it
  here needs an allowlist change (YouTube, `*.googlevideo.com`, `i.ytimg.com`),
  which is infrastructure and was not touched.
- Optional: `MUSIC_YTDLP_COOKIES_B64` (base64 of a Netscape `cookies.txt` of a
  throwaway YouTube account, from a secret manager) or `MUSIC_YTDLP_COOKIES`
  (a path to the file); see "Cookies" below. Each yt-dlp run uses its own
  temporary copy (removed afterwards), because yt-dlp rewrites the file on exit
  and runs overlap.
- Speed. Most of a cold start is yt-dlp resolving the song. Three things help:
  - **Cache** (`MUSIC_YTDLP_CACHE_DIR`, default `/tmp/mitch-ytdlp-cache`): yt-dlp
    keeps the solved YouTube player script there, so each song doesn't download and
    solve it again. This used to be switched off (`--no-cache-dir`), which made every
    uncached song slow.
  - **Warm-up**: five seconds after startup the server resolves one well-known video
    so the first real click isn't the one that fills that cache. The log line
    `[music] yt-dlp primed in N ms` also tells you yt-dlp works from this server.
  - **Extractor args** (`MUSIC_YTDLP_EXTRACTOR_ARGS`, unset by default): passed to
    yt-dlp as `--extractor-args`. Choosing one player client, for example
    `youtube:player_client=android_vr`, can cut a second or more, but which clients
    work changes with YouTube, so it is a knob to try, not a default.
  - To see where a click's time went, open the `stream` request in the browser's
    network panel: the `Server-Timing` header shows `resolve` (with `memory`, `db` or
    `yt-dlp` as the source) and `upstream` (time to the first byte from Google).
    Resolves over 2.5 s are also logged.

### Cookies

Logged-out yt-dlp use gets roughly 300 videos an hour from one IP before YouTube
answers 429 or a bot check; a signed-in session gets about 2,000. Password
login doesn't work with yt-dlp any more, so this takes cookies from a browser
session. Use a throwaway Google account, never a real one: YouTube may lock or
ban an account used this way, and automated access is against its terms.

1. Make a new Google account (it may ask for a phone number).
2. In a private/incognito browser window, sign in to youtube.com with it.
3. In that same window, open `https://www.youtube.com/robots.txt` (a page that
   doesn't rotate the session), and export youtube.com cookies in Netscape
   format with a cookies.txt extension (e.g. "Get cookies.txt LOCALLY").
4. Close the private window without signing out, so the browser never refreshes
   those cookies.
5. Give the server the cookies one of two ways:
   - **Secret manager, no file on the server** (preferred where secrets are kept
     in a secret manager): base64 the file (`base64 -w0 youtube-cookies.txt`) and
     store the result as `MUSIC_YTDLP_COOKIES_B64`. `docker-compose.yml` passes it
     through to the server containers. For each yt-dlp run the server writes a
     temporary mode-0600 copy in memory-backed `/dev/shm` (the cache directory if
     there is none) and deletes it when the run ends, so the secret is never
     stored on disk by the server. It is never logged.
   - **A file**: `MUSIC_YTDLP_COOKIES=/path/to/cookies.txt` (outside git, `chmod
     600`; each run still works on its own temporary copy).
   If both are set, `MUSIC_YTDLP_COOKIES_B64` wins. Restart the server. The cookies
   are a login credential: never commit or paste them.

When the cookies stop working (yt-dlp logs the bot check again) export a new
set the same way.

## API (all require a login; 401 otherwise)

| Endpoint | |
| --- | --- |
| `GET /api/music/search?q=` | up to 15 tracks `{id, title, artist, duration}`; artist/title are guessed from the video title and channel (`clean_youtube_title`). Live streams, mixes over 20 min and non-video ids are dropped. Cached: see *Search cache* below. |
| `GET /api/music/stream?id=` | audio, `Range` supported. An open-ended range is capped at 10 MB (`MAX_RANGE_BYTES`) because Google's CDN refuses some larger ones. One retry with a fresh URL on 403/410. |
| `GET /api/music/prefetch?id=` | resolves a URL ahead of time, 204. Also how the client finds out why the `<audio>` element failed. |
| `GET /api/music/thumb?id=` | cover art from `i.ytimg.com` |
| `GET /api/music/lyrics?id=&title=&artist=&duration=[&custom=1]` | see below |
| `GET /api/music/related?id=` | up to 20 songs YouTube's Mix suggests for a song (`watch?v=ID&list=RDID`), the seed excluded; cached 30 min. Used by smart shuffle. |
| `GET /api/music/library`, `POST library/add`, `POST library/remove` | saved tracks (max 500 per user) |

Safeguards: video ids must match `[A-Za-z0-9_-]{11}` before anything touches
yt-dlp or a URL; the stream proxy only ever fetches `https://*.googlevideo.com`
and captions only `https://www.youtube.com/api/timedtext` (so a crafted
extractor result can't turn it into an open proxy); at most 4 yt-dlp processes
run at once; per-user limits per minute: 20 searches, 20 uncached lyric lookups,
40 stream resolutions.

Storage: `music_favorites` (per user, erased with the account, listed in
`account_deletion.rs`) and `music_lyrics_cache` (shared, keyed by the video id plus a hash of the
title and artist the lookup used, stored as enhanced-LRC text, not JSON). Listening history is **not** stored
anywhere; only tracks a user saves.

### Songs only

Search and related results pass `looks_like_song` (`ytdlp.rs`): 1 to 12 minutes
long, and no whole-word match on reaction, review, episode, mix, compilation,
tutorial and similar words, or the phrases "top 10", "full album", "first time",
"side by side". If nothing passes, the list is shown unfiltered. It's a title
heuristic: YouTube's own songs-only search (`music.youtube.com/search#songs`
through yt-dlp's `youtube:music:search_url`) returned no entries when tried, so
it isn't used. Scenes and talk videos with neutral titles can still get through.

### Search cache

Results are kept in `music_search_queries` / `music_search_results` (real tables,
keyed by the lower-cased query). A query answered within 10 minutes is returned
as stored. An older one (up to 30 days; the worker prunes past that, and past
3,000 queries) is returned **at once** and a background refresh fetches the
current list for next time (one refresh per query at a time; a failed refresh
keeps the old results; an empty answer never overwrites a stored one). New
queries go to yt-dlp and take about a second. Results are warmed (stream links,
lyrics) on every response. yt-dlp runs in three pools so nothing a person waits
on queues behind background work: 4 slots for clicks (stream links, the lyrics'
caption fallback), 3 for searches, and 2 for background work (warming links,
related songs, the caption check for lyric timing). Search results are warmed by
`warm_search`: it waits 1.5 s for the search to stand still, resolves the
results in order and stops when a newer search arrives, so typing doesn't pile
up resolves. The page waits 650 ms after the last keystroke before searching. A
search over 2.5 s is logged (`slow search`), which means yt-dlp itself (startup,
network, the egress proxy, CPU) was slow. While a search is running the list
shows shimmering placeholder rows.

The page searches as you type: 650 ms after the last keystroke, for 2+
characters, skipping a query already shown; Enter searches immediately, and an
older, slower answer never replaces a newer one. The per-user limit of 20
searches a minute still applies.

## Lyrics

`lyrics.rs` asks every source at once and keeps the best result.

| Source | Timing | Notes |
| --- | --- | --- |
| LRCLIB | word (`lyricsfile` YAML), line, plain | open database, no key |
| NetEase Cloud Music | word (YRC), line | unofficial API |
| Kugou | word (KRC: XOR key, then zlib) | unofficial API |
| QQ Music | line | unofficial API |
| lyrics.ovh | plain | |
| YouTube captions | word (auto-generated), line (manual) | via yt-dlp; only if no line-timed lyrics arrived within 2 s |

Not used: anything paid or needing a private session token (Musixmatch, Apple
Music, Spotify), and Genius (needs scraping its pages, which its terms forbid).
Kuwo and Megalobiz were tried and dropped (their endpoints no longer answered).

Ranking: real word timing (4) over line timing (3) over auto-caption timing (2)
over plain text (1); within a rank, the better title/artist match. A candidate
must score at least 0.6 on `match_score`, and when both durations are known be
within 4 s, so a live version's lyrics don't land on the studio track. Auto
captions rank below line-timed lyrics because speech recognition mishears sung
words even though it times each word. Once line-timed lyrics are in hand the
remaining sources get 1.2 s to upgrade them to word timing, and nothing waits
past 10 s.

**Finding lyrics for awkward titles.** The video title is read several ways
(`query_guesses`): the obvious artist/title split, the title alone (catalogs credit
artists in other languages or spellings, so the artist is dropped), each `|` part
and each quoted part (`"Song"`, `『Song』`), and artist/title swapped. Parts that
only describe the video ("Opening 1", "Creditless", "4K", "Subtitles") are never
taken as the song. All readings are searched at once. If nothing matches by
length, a second pass accepts a recording up to four times longer than the video
(`duration_is_full_version`), which is how a 1:30 TV-size upload finds the full
song's lyrics. That result is flagged `approximate` and the page says "timing is
for the full song": the start usually lines up, the rest may drift.

**Timing check against the video's captions.** Lyric timing from a source can be
off by many seconds for a given video (a different edit, an intro). When a result
has line or word timing for this length of recording, `lyrics::align` fetches the
video's captions (auto-generated ones are timed per word) and `estimate_offset`
compares them with the lyrics: each lyric line of 3+ words is looked for among
the caption words within a minute of where the lyrics put it, and the median
difference is the correction, but only when at least 6 lines match and 60% of
them agree within 1.5 s, and it is 0.6 to 45 s. The lyrics are shifted by it
(`shift`) before they are cached. Ties between repeated choruses go to the one
nearest the current timing, so doubtful cases are left alone. Lyrics warmed in
the background are stored with `aligned = 0` and checked on their first real
play (once; concurrent plays share the work), so the check costs a yt-dlp run
only for songs somebody plays. A video without captions leaves the timing as it
was. The lyrics view also has -0.5 s / +0.5 s buttons; the viewer's correction
per song is kept in this browser (`mitch.music.lyroff`) and applied in
`MitchMusic.getLyrics`, so the follow-along card uses it too.

**Retrying for word timing.** Sources answer at different speeds, and a lookup
only waits 1.2 s for word timing once line timing is in hand, so a slow source can
be missed and a song cached as line-timed. Two fixes: (1) a cached `line` row gets
one patient background retry the next time it is played (`spawn_upgrade`,
`FetchMode::Refresh`: waits 6 s for word timing, one at a time, once per song,
recorded in `upgrade_tried`), and the cache row is replaced only if the result has
better timing (`sync_rank`); (2) "Wrong lyrics?" submitted without changing the
title or artist is sent as `custom=1&retry=1`, which runs the same patient lookup
for the song itself, shows the result and stores it for everyone if it is better
(a changed title is still a private custom search that is never cached).

**Word timing from another version.** A music video often differs from the studio
track (an intro, a repeated section), so the word-timed lyrics for the song are
a different length and are rejected for the video, leaving only line timing.
When the video's own line timing is all there is, a second search looks for
word-timed lyrics of the same song at a similar length (0.6 to 1.7 times,
`duration_is_similar`) and `graft_words` borrows their word timing: the lines of
the two versions are aligned by text (longest common subsequence, so extra or
missing lines don't throw it off), every matched line keeps the video's own start
time, and its words follow at the donor's offsets from that start. At least half
of the sung lines must match, and a line whose words wouldn't fit the time the
video gives it is skipped (it gets estimated words). Unmatched lines fall back
to `estimate_words`.

"Per word" for every synced song: a line-timed result goes through
`estimate_words`, which spreads the line's time over its words (by length; CJK
per character) and marks `estimated: true`. The page says "line timing, words
estimated" in that case.

The lyrics cache key includes the title and artist because the client supplies
them: keyed by video id alone, one user could store the wrong song's lyrics for
everyone. Now a different title gets its own entry, and the usual case (the title
the server's own search produced) is shared. Bodies over 120 KB are skipped and
the worker removes rows past their time to live.

**Caching follows the stream-link rules.** Search results get their lyrics looked
up in the background right after the search (`worker::warm_lyrics`: at most 2 at
once, a backlog of 30, songs already cached skipped), so by the time a song is
clicked its lyrics are usually there. A background lookup (`FetchMode::Ahead`)
skips the YouTube-captions fallback and only caches hits. Songs in any playlist
are **pinned**: their rows never expire and are not counted against the 5,000-row
cap for unpinned rows (30,000 overall), and the worker fills in any playlist song
that has none. Deleting a playlist deletes the lyrics of songs no other playlist
holds. `get_cached_lyrics` retries a pinned miss after 6 h like any other.

Other bounds: background link warming holds at most 40 songs at a time (the rest
of a burst is dropped; a click still resolves its own song), a request waits at
most 20 s for a yt-dlp slot (6 run at once), and the per-user limits above apply
to every endpoint that resolves, searches or looks up lyrics.

A custom search ("Wrong lyrics?") passes `custom=1`, which skips the cache in
both directions so one user's correction never replaces what others get. A
lookup where every source errored (network trouble) isn't cached as "nothing
found"; a clean miss is cached for 6 h, a hit for 30 days.

## The page

`/music/` is built around a stage: the playing song's cover art large, its lyrics
in big type, and a dock with seek, shuffle, previous/play/next and repeat
(repeat is off, all, or one) and volume. The shuffle button cycles off, random and
smart. The stage tints itself from the cover
art (a blurred copy behind a scrim, so text stays readable in both themes); the
cover grows slightly while playing. The left side is laid out like a music app: a sidebar (search field, the four
sections Search, Library, Playlists and Queue with icons, and a "My playlists"
list with cover thumbnails that opens a playlist directly), then the list for
the chosen section under a large heading ("Results for “queen”", "Library"...),
then the stage. Below 1280 px the sidebar collapses into a row of section
buttons above the list and the playlist list is hidden (the Playlists section
still has them). Rows show a moving equalizer on the playing song and reveal
their buttons on hover. On a narrow screen the stage comes first and the panel
below it. Thumbnails are
cropped to remove the black bars YouTube adds around video frames. Keys: space
plays or pauses, left and right arrows seek 5 seconds.

The follow-along card resets every `<button>` style (`all: unset`), because pages
such as the homepage style all buttons with padding and a minimum height, which
turned its icons into large empty boxes. The pop-out hides the site's wallet bar,
bell, mobile dock and credit band, which would cover its controls.

## Lyric effects and the backdrop

Beyond the left-to-right fill, `createLyricsView` marks the state of the lyrics
and `music.css` (section "lyric effects") draws it, kept gentle on purpose (an
earlier version lifted and enlarged the sung word and read as janky): every line
carries `--d`, its distance from the line being sung, and fades and softens a
little with it (no blur on touch screens); the sung line is slightly larger; and
small four-point sparkles (`.ml-spark`) float off the leading edge of the fill of
the word being sung, one every 150 ms (260 ms on touch screens), at most 12 alive,
none with reduced motion or in a hidden tab. Words themselves never move. An
instrumental break shows three dots that fill up through the break (`--gp`).
Behind the lyrics three blurred colour orbs drift slowly while a song plays
(`.mp-orbs`) and the cover backdrop pulses lightly on each new line (`onLine`,
class `mp-beat`). `prefers-reduced-motion` turns the movement off.

On a narrow screen the stage is hidden while nothing plays (the search comes
first), the cover and title sit in a row with the heart / add / pop-out buttons
under the title, and the lyrics pane is only as tall as its message when there are
no lyrics.

## Shuffle

- **Random**: a uniformly random song other than the current one; Previous steps
  back through what was actually played.
- **Smart**: plays every song in the queue once per round, in an order weighted
  against back-to-back songs by the same artist (weights x0.12 same artist as the
  last song, x0.45 as the one before) and toward songs of similar length. After
  two smart picks the player asks `related` for the playing song and slips one
  suggestion in after the current song, tagged "Suggested" in the queue.
  A failed lookup is ignored.

The mode is stored with the player state, so it survives reloads and page changes.

## Settings and playlist actions

Shuffle, repeat, crossfade and volume are kept in their own storage key
(`mitch.music.prefs`), apart from the queue, so they can be set before anything
plays and survive closing the player. With nothing playing they are applied
directly; otherwise they go to the page that is playing.

The player's stage has an **Add to playlist** button (next to the heart) that opens
the same picker as a search result's playlist button, for the song that is playing.

A playlist page has a cover (a mosaic of its first songs), a big Play and a
Shuffle button (`MitchMusic.playShuffled`: random shuffle when it was off, the
chosen mode when it was on; it starts at a random song), and one "more" menu
(share link, make public/private, rename, delete; for someone
else's playlist: save a copy, message someone) instead of a row of buttons. A
playlist imported from Spotify also has a **Resync** button next to Shuffle. New
playlist and Import from Spotify are in the + menu of the playlist list.

## Crossfade

A "Fade" button left of the transport controls cycles crossfade off, 3, 6 and 9 seconds
(stored with the player state). In the last N seconds of a song the host starts
the next song in a second `<audio>` element and fades the two against each other
(equal-power curve, driven by a timer from the wall clock). Only the current
element may change player state, so the old one finishing doesn't skip a song.
Any command that changes what plays (next, previous, jump, pause, stop) ends the
overlap at once. Songs shorter than 3x the fade, repeat-one and the last song
without repeat don't fade. iOS fixes `<audio>` volume at 100%, so there
`xfOk` is false, the button is hidden and songs just follow each other.

## Playing across pages

Pages here are separate documents, so audio can't survive a link click. Instead:

- `music-widget.js` is injected on every page (`pipeline.rs`, `inject.rs`). If the
  visitor has never started the player (no `mitch.music.v1` in localStorage) it
  returns immediately and makes no requests. Otherwise it loads `music-core.js`
  and shows the card. It isn't shown in iframes or on `/music/*`; it also checks `body.music-page` and `#mp-stage` and the page script removes any `.mm-card`, so a differently routed copy of the player page never shows two players.
- One context at a time owns the `<audio>` (the **host**). Others are **remotes**:
  they mirror the host's state and send commands over a `BroadcastChannel`
  (`mitch-music`). The host sends its state every 500 ms; a remote that hears
  nothing for 2.5 s drops to idle with the last known state.
- On navigation the old page saves `{queue, index, position, playing, owner}` and
  the next page in the same tab (`sessionStorage` tab id equals `owner`) resumes
  from the saved position, usually within half a second. If the browser blocks
  autoplay, the card says "Press play to continue".
- Pressing play in another tab makes that tab the host (`claim`), and the old
  host pauses.
- **Pop-out**: `/music/popout/` opens in a small window, takes over from the page
  and becomes the host, so playback is not interrupted by navigation in the main
  window. The card on each page then shows "Playing in the pop-out window" and
  controls it remotely. Closing the pop-out leaves the player paused at its last
  position.
- Media keys, lock-screen controls and cover art use the Media Session API.

The lyrics view (`createLyricsView`) is shared by the page, the pop-out and the
card's one-line lyric: a binary search finds the current line, and each word's
`--p` (0 to 1) fills it left to right.

## Not verified / known gaps

- Built in a sandbox where YouTube answers the bot check, so stream resolution
  was never exercised end to end; the audio path was tested with a mocked
  `/api/music/stream`. Verify on a connection YouTube accepts.
- Live-verified lyrics sources: LRCLIB (including word-synced), lyrics.ovh and
  QQ Music. NetEase, Kugou and YouTube captions are covered by fixture tests of
  their formats only (those hosts were unreachable or blocked from the sandbox).
- The chat share prompt was tested against a stub `__mxClient` (the real chat
  server isn't available locally), not the live chat client. It uses standard
  matrix-js-sdk calls (`searchUserDirectory`, `createRoom`, `setAccountData`,
  `sendTextMessage`).
- The Dockerfile change was checked piece by piece (the pinned download and its
  checksum verify, the shell logic runs) but not built end to end: no Docker
  daemon was available here and the musl binary can't run on this machine.
- Sharing between two different accounts wasn't exercised in a browser (local
  dev signs everyone in as one user); the access rules are covered by unit tests.
- No end-to-end browser test is committed; the player flow was driven by hand
  with Playwright.
