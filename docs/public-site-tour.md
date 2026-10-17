# Public intro and recorded tour

The public introduction is `webserver/index-sales.html`, styled by `webserver/onboarding.css`. Signed-out visitors see it at `/`; `/index-sales.html` also opens it directly, with or without a session. The player is the main content at the top, followed by a horizontal chapter strip, download link, account actions, site links, schools, and help.

Login and account creation remain at `/enroll/`, using `webserver/public/enroll/enroll.css`. Both pages share `onboarding-theme.js` and `onboarding.js`; `data-presentation="onboarding"` keeps account-shell styles out of the public pages.

## Current edit

`webserver/media/site-tour-v4.mp4` is exactly **237.000 seconds (3:57)**. It contains 1920×968 H.264 video at 30 fps and a 48 kHz stereo AAC soundtrack. MP4 metadata is at the front for playback before the entire download finishes.

The picture comes from the complete `20261017-133852.mp4` screen recording. Every section is sped up using the reproducible map in `tools/site-tour-v3.json`. Version 4 copies version 3's video stream exactly and reuses its poster. The soundtrack is the supplied `C418 - Aria Math, but it's an EPIC Soundtrack.mp3`, starting at its beginning and trimmed only to the requested 3:57 duration. Smooth dynamic normalization raises quiet passages, followed by two-pass loudness normalization targeting -14 LUFS with a -2 dBTP ceiling. The short final fade remains. The picture fades in briefly and fades out with the track's quiet ending.

Chapter changes align the coins/shop section with the first build, Matrix with the quieter passage near 1:30, and profile cosmetics and casino with the later crescendo. The source sequence is preserved. The poster is a frame of the games library from 35 seconds into the original recording.

| Output time | Section | Source time |
| --- | --- | --- |
| 0:00.0 | Dashboard | 0:00 |
| 0:08.3 | People and profiles | 0:10 |
| 0:24.7 | Games | 0:30 |
| 0:45.5 | Bell schedule | 0:55 |
| 0:59.9 | MitchCoins and shop | 1:10 |
| 1:30.3 | Matrix chat | 1:50 |
| 1:59.9 | Cloud Linux | 2:25 |
| 3:09.5 | Profile cosmetics | 3:50 |
| 3:26.3 | Casino | 4:10 |
| 3:46.7 | Blooket tools | 4:35 |

Playback starts when the visitor presses Play or selects a chapter. It never autoplays. Native volume, seeking, and fullscreen controls remain available. The central play button works from the keyboard, starts with sound, and becomes Replay at the end. The chapter strip highlights the current section.

## Reproducing and replacing the edit

With Python, ffmpeg, and ffprobe installed, use an unused version number:

```sh
python3 tools/create-site-tour.py /path/to/recording.mp4 --version 5 --poster-time 35 \
  --timing tools/site-tour-v3.json --soundtrack /path/to/soundtrack.mp3
```

The tool leaves source files untouched and refuses to overwrite published assets. A timing map specifies the output duration and source/output chapter points. The tool checks that the map only speeds up the recording and that the soundtrack covers the full edit.

To replace only the audio in an existing edit, retaining the exact video and its poster:

```sh
python3 tools/create-site-tour.py webserver/media/site-tour-v3.mp4 --version 5 \
  --copy-video --soundtrack /path/to/soundtrack.mp3
```

For a new published version, update the video and download links, public-asset list, range-supported paths, and browser check. Update the poster, social previews, duration, and chapter data when the picture or timing changes. Versions 1 through 3 remain available for cached pages. All four published MP4 versions support unauthenticated byte-range GET and HEAD requests.

## Verification

```sh
cargo fmt --manifest-path rust/Cargo.toml --all --check
cargo clippy --manifest-path rust/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path rust/Cargo.toml --workspace
cargo build --manifest-path rust/Cargo.toml -p mitch-server
node tests/test_onboarding_browser.mjs
```

The browser check starts an isolated local server and mocks account requests. It verifies the direct public intro, styles and media without a session, range responses, exact video duration, decoded audible audio, keyboard Play, replay, every chapter, both themes at five viewport sizes, and signup/login/reset flows. It creates no production accounts.

The media was also checked against its sources: frames from games, Matrix, profile cosmetics, and Blooket match the original recording at the mapped times. Versions 3 and 4 have identical video-stream SHA-256 hashes. The encoded version 4 soundtrack measures -14.14 LUFS integrated, -1.87 dBTP true peak, and 8.5 LU loudness range (previously 23.7 LU). Five-second passages starting at 0:10 and 1:40 are 22.2 dB and 17.6 dB louder respectively, without clipping. Screenshots are saved under the ignored `artifacts/ui-review/onboarding` directory.

## CI

The formatting failure in admin accounts, moderation, and Matrix routes was corrected in commit `4eb00d5`. The exact formatting, strict Clippy, and workspace-test commands used by `rust-ci` pass locally.
