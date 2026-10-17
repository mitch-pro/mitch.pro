# Public site and recorded tour

The public homepage is `webserver/index-sales.html`, styled by `webserver/onboarding.css`. It shows a short introduction, the recorded tour, a directory of site features, school links, and help. Account creation and login remain at `/enroll/`, with their own stylesheet at `webserver/public/enroll/enroll.css`.

Both pages use `onboarding-theme.js` for the theme cookie and `onboarding.js` for video chapters, password visibility, and keyboard tabs. Their `data-presentation="onboarding"` marker keeps the account shell's injected styles and navigation from interfering.

## Current recording

The October 17, 2026 recording is `webserver/media/site-tour-v2.mp4`, with `site-tour-poster-v2.webp` extracted at five seconds. It replaces the previous homepage tour. The source recording was `20261017-133852.mp4` (SHA-256 `013633a7b6742e74817add593511bb0876e371633a170e5a7b0a9b79d5da4c3a`).

The complete recording runs 287.73 seconds, displayed as 4:47. The web copy uses H.264 at 1920×968 and 30 fps, with MP4 metadata at the front for playback before the entire download finishes. It preserves the silent recording; no music, voiceover, or overlays were added.

| Time | Section |
| --- | --- |
| 0:00 | Dashboard |
| 0:10 | People and profiles |
| 0:30 | Games |
| 0:55 | Bell schedule |
| 1:10 | MitchCoins and shop |
| 1:50 | Matrix chat |
| 2:25 | Cloud Linux |
| 3:50 | Profile cosmetics |
| 4:10 | Casino |
| 4:35 | Blooket tools |

The player has native controls and chapter shortcuts, highlights the current chapter, and preloads only metadata. It never starts automatically. Download and social preview links use the new versioned assets.

The Rust public-asset list explicitly allows the new MP4 and poster. Both v1 and v2 videos support GET byte ranges and HEAD requests so seeking works and older cached pages retain a working player. Other static files retain their access policies. Keep v1 assets for those cached references; media is cached for a year.

## Preparing another recording

With Python, ffmpeg, and ffprobe installed, choose an unused version:

```sh
python3 tools/create-site-tour.py /path/to/recording.mp4 --version 3 --poster-time 5
```

The tool leaves the input untouched, preserves source audio if present, and produces the optimized MP4 and poster. It refuses to overwrite existing assets. Update the homepage source, poster, download links, social previews, duration, chapters, server allowlist, range-supported paths, and browser check for the new version.

## Verification

```sh
cargo test --manifest-path rust/Cargo.toml --workspace
cargo build --manifest-path rust/Cargo.toml -p mitch-server
node tests/test_onboarding_browser.mjs
```

The browser check starts an isolated local server. It verifies unauthenticated styles and media, protected account routes, video range responses, actual playback and every chapter, mobile and desktop layouts, both themes, and enrollment flows with mocked account responses. Screenshots go to the ignored `artifacts/ui-review/onboarding` directory.

School links retain the six comprehensive high schools, Roseville Pathways, and Roseville Adult School. Bell schedules and period countdowns are on `rjuhsd.school`. VM access and available capacity remain governed by the server; optional hardware and session upgrades use earned MitchCoins.
