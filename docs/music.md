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

"Send in chat" on a playlist opens `/matrix/?mshare=<link>&mname=<name>`.
`webserver/matrix/music-share.js` (loaded by the chat page) waits for the chat
client to sync, lists the user's joined rooms, and sends `🎵 <name> <link>` with
the client's own `sendTextMessage`, so encrypted rooms stay encrypted and the
server never sees the message. The link must be on this site's `/music/` with a
`playlist` parameter, otherwise nothing is shown. The prompt is its own overlay;
the script doesn't touch the chat UI. Whoever clicks the link in chat lands on
the playlist page.

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

Time from click to sound, uncached: yt-dlp resolution (2 to 5 s) plus the first
bytes (about 1 s). A cached URL starts in under a second. While a track plays,
the next queue entry is resolved in the background (`/api/music/prefetch`), so
queue advances are near-instant. Search takes about 1.4 s (`--flat-playlist`).

### Setup the server needs

- `yt-dlp` on `PATH` (or `MUSIC_YTDLP_BIN`). Without it every music endpoint that
  needs it answers 503 "music source is not set up on this server" and the page
  shows that message. **The production image does not install it yet**: that is a
  change to `rust/Dockerfile`, which is part of what a deploy depends on, so it
  was left for the owner to approve. yt-dlp also wants a JS runtime for YouTube
  (deno, or `MUSIC_YTDLP_JS_RUNTIME=node`).
- A datacenter IP is often answered with "Sign in to confirm you're not a bot"
  (this is what the sandbox the feature was built in gets). Set
  `MUSIC_EGRESS_PROXY` to an HTTP proxy that leaves through a clean connection;
  yt-dlp and the byte proxy both use it, which they must, because the signed URL
  is bound to the IP that requested it. The existing `movies-egress` sidecar only
  allows `CONNECT` to its three Movies hosts (`egress-proxy/proxy.py`), so using it
  here needs an allowlist change (YouTube, `*.googlevideo.com`, `i.ytimg.com`),
  which is infrastructure and was not touched.
- Optional: `MUSIC_YTDLP_COOKIES` (cookies.txt path).

## API (all require a login; 401 otherwise)

| Endpoint | |
| --- | --- |
| `GET /api/music/search?q=` | up to 15 tracks `{id, title, artist, duration}`; artist/title are guessed from the video title and channel (`clean_youtube_title`). Live streams, mixes over 20 min and non-video ids are dropped. Cached 10 min. |
| `GET /api/music/stream?id=` | audio, `Range` supported. An open-ended range is capped at 10 MB (`MAX_RANGE_BYTES`) because Google's CDN refuses some larger ones. One retry with a fresh URL on 403/410. |
| `GET /api/music/prefetch?id=` | resolves a URL ahead of time, 204. Also how the client finds out why the `<audio>` element failed. |
| `GET /api/music/thumb?id=` | cover art from `i.ytimg.com` |
| `GET /api/music/lyrics?id=&title=&artist=&duration=[&custom=1]` | see below |
| `GET /api/music/library`, `POST library/add`, `POST library/remove` | saved tracks (max 500 per user) |

Safeguards: video ids must match `[A-Za-z0-9_-]{11}` before anything touches
yt-dlp or a URL; the stream proxy only ever fetches `https://*.googlevideo.com`
and captions only `https://www.youtube.com/api/timedtext` (so a crafted
extractor result can't turn it into an open proxy); at most 4 yt-dlp processes
run at once; per-user limits per minute: 20 searches, 20 uncached lyric lookups,
40 stream resolutions.

Storage: `music_favorites` (per user, erased with the account, listed in
`account_deletion.rs`) and `music_lyrics_cache` (shared, keyed by video id,
stored as enhanced-LRC text, not JSON). Listening history is **not** stored
anywhere; only tracks a user saves.

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

"Per word" for every synced song: a line-timed result goes through
`estimate_words`, which spreads the line's time over its words (by length; CJK
per character) and marks `estimated: true`. The page says "line timing, words
estimated" in that case.

A custom search ("Wrong lyrics?") passes `custom=1`, which skips the cache in
both directions so one user's correction never replaces what others get. A
lookup where every source errored (network trouble) isn't cached as "nothing
found"; a clean miss is cached for 6 h, a hit for 30 days.

## Playing across pages

Pages here are separate documents, so audio can't survive a link click. Instead:

- `music-widget.js` is injected on every page (`pipeline.rs`, `inject.rs`). If the
  visitor has never started the player (no `mitch.music.v1` in localStorage) it
  returns immediately and makes no requests. Otherwise it loads `music-core.js`
  and shows the card. It isn't shown in iframes or on `/music/*`.
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
  server isn't available locally), not the live chat client. `sendTextMessage` is
  the standard matrix-js-sdk call the bundle's client exposes.
- Sharing between two different accounts wasn't exercised in a browser (local
  dev signs everyone in as one user); the access rules are covered by unit tests.
- No end-to-end browser test is committed; the player flow was driven by hand
  with Playwright.
