# Site audit

The browser audit covers 62 first-party pages at 320, 390, 768, and 1440 pixels. It records screenshots, page script errors, missing local assets, and horizontal overflow in `artifacts/ui-review/site-<name>/audit.json`. Imported game libraries, the Matrix client, and noVNC retain their own interfaces.

This pass fixes the shared button and input rules that overrode component styles, hidden controls appearing prematurely, mobile overflow, duplicate font loading, inconsistent page palettes, decorative captions and arrows, the fixed legal footer, ordinary shortcuts opening inside game frames, school links losing their selected school on reload, mobile menu state, and canonical sitemap URLs.

The layout uses the games page's neutral colors and green accent. Light mode, custom accents, high contrast, reduced motion, account settings, and school identities remain supported. Custom accent buttons choose readable black or white text.

With an isolated local server and a fixture account running on port 4320:

```sh
npm run audit:site -- after
npm run test:site:browser
cargo test --manifest-path rust/Cargo.toml --workspace
```

`SITE_AUDIT_BASE_URL` selects another local port. Install the project's Playwright dependency and its Chromium browser before running the browser checks. `PLAYWRIGHT_MODULE` and `PLAYWRIGHT_EXECUTABLE_PATH` can select an existing local installation.

The interaction checks cover normal navigation with game frame preferences enabled, hidden controls, keyboard-operated settings, theme persistence, font/size/corner/motion/surface controls, accent contrast, shop search, school switching and reload, and the mobile menu. These checks use local fixture data; live purchases, private conversations, account deletion, and computer provisioning are not performed by the audit.
