# Music player (`/music/`)

Signed-in users search for songs, play them, and watch synced lyrics. A small
card follows the user to every other page so they can pause from anywhere, and
the player can be popped out into its own window.

Code: `rust/crates/mitch-server/src/routes/music/` (API), `mitch-lib/src/music.rs`
(favorites, lyrics cache), `mitch-lib/src/music_playlists.rs` (playlists),
`mitch-lib/src/music_lyrics.rs` (lyrics formats and matching),
`webserver/music-core.js`, `music-widget.js/.css`, `webserver/music/`.

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
  whose title/artist score at least 0.5 and whose length is within 6 s). Matches
  are stored in `music_track_matches` and shared by everyone;
- **Sync from Spotify** (and an automatic sync when the owner opens a playlist
  more than a day stale) makes the Spotify part of the playlist equal Spotify's
  current list and order, keeps existing matches, and keeps songs the owner added
  by hand after it. Songs removed on Spotify disappear.
- Reading the embed page is not an official API. If Spotify changes it,
  `parse_embed` is the one place to fix. Private playlists, Liked Songs and
  playlists past 100 songs would need a registered Spotify app and OAuth, which
  this doesn't have.

## Where the audio comes from

YouTube, through [yt-dlp](https://github.com/yt-dlp/yt-dlp) on the server.
Nothing is stored. For a track the server asks yt-dlp for a short-lived signed
URL on Google's CDN, caches it (`ytdlp::RESOLVE_TTL`, 4 h), and proxies the
bytes to the browser. Everything goes through this server (audio, cover art,
lyrics), so a network that blocks YouTube or the lyric sites still works and no
third party sees the visitor.

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
  link missing or lapsing within 45 minutes (25 per cycle, one at a time). Links
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
- Optional: `MUSIC_YTDLP_COOKIES` (cookies.txt path).
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
lyrics) on every response.

The page searches as you type: 400 ms after the last keystroke, for 2+
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
cover grows slightly while playing. The left panel has search, the library,
playlists (cards with a mosaic of their first covers) and the queue. Rows show a
moving equalizer on the playing song and reveal their buttons on hover. On a
narrow screen the stage comes first and the panel below it. Thumbnails are
cropped to remove the black bars YouTube adds around video frames. Keys: space
plays or pauses, left and right arrows seek 5 seconds.

The follow-along card resets every `<button>` style (`all: unset`), because pages
such as the homepage style all buttons with padding and a minimum height, which
turned its icons into large empty boxes. The pop-out hides the site's wallet bar,
bell, mobile dock and credit band, which would cover its controls.

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
