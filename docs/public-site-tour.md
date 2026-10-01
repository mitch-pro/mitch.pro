# Public site and recorded tour

The public homepage, enrollment page, and help page share `webserver/onboarding.css`, `onboarding-theme.js`, and `onboarding.js`. Their `data-presentation="onboarding"` marker keeps the account shell's injected styles and navigation from interfering with these pages. The theme toggle uses the site's existing theme cookie.

The tour is `webserver/media/site-tour-v1.mp4`. It has native controls, an original instrumental soundtrack, and chapter shortcuts. Playback starts only when the visitor requests it; only video metadata preloads. The Rust server explicitly allows the two tour assets and supports single byte ranges for seeking. Account pages and VM endpoints retain their access checks.

To generate the tour from the original recording, install Python, numpy, and ffmpeg, then run:

```sh
python tools/create-site-tour.py /path/to/recording.mp4
```

The script synthesizes the instrumental without samples, preserves the input recording, and writes the optimized MP4 and poster. If replacing a published tour, increment the asset filenames and update their references and server allowlist: versioned media is cached for a year.

To check the public pages and enrollment flows locally:

```sh
cargo test --manifest-path rust/Cargo.toml --workspace
cargo build --manifest-path rust/Cargo.toml -p mitch-server
npx playwright install chromium
node tests/test_onboarding_browser.mjs
```

The browser test requires the project's Node dependencies. It starts an isolated local server, verifies public and protected routes and video ranges, checks responsive layouts and theme switching, and exercises signup, verification, resend, login, two-factor prompts, reset, and keyboard navigation using mocked account responses. It never creates production accounts. Screenshots are written to the ignored `artifacts/ui-review/onboarding` directory.

The copy describes computers as free, with availability governed by account access and capacity. Optional hardware and session upgrades use earned MitchCoins. It does not change VM eligibility, prices in virtual coins, or backend account policies.
