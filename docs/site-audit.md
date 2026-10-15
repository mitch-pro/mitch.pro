# Site audit

The browser audit covers 63 first-party pages at 320, 390, 768, and 1440 pixels. It records screenshots, page script errors, missing local assets, and horizontal overflow in `artifacts/ui-review/site-<name>/audit.json`. Imported game libraries, the Matrix client, and noVNC retain their own interfaces.

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


The second refresh adds a handwritten greeting, a single hover People panel, quieter notification controls, credit bands, a video above account access, a new MitchCoin mark and wallet hub, a compact casino lobby, and a schedule-first school layout. The school page URLs, canonical tags, descriptive titles, schedule text, structured data, school directory links, and sitemaps remain intact. See [Google’s developer SEO guidance](https://developers.google.com/search/docs/fundamentals/get-started-developers).

The casino uses the public [Virtual Casino Simulator](https://github.com/andreivorobiev/virtual-casino-simulator) as a reference for a categorized lobby, separate game views, and a shared server wallet. No third-party code or artwork was copied. Existing Rust games and settlements remain authoritative. The logo is original SVG artwork, with PNG and WebP exports for existing consumers.

`node tests/test_site_refresh_browser.mjs` additionally checks fixture purchases and equipping, cosmetic colors in People and friends, safe user text, all 16 casino rooms, balance settlement, rejected wagers, credits, video order, and retirement of happy hour. It requires the isolated fixture database (admin and Casey accounts), never production data. `python3 tests/test_coin_balance_reset.py` tests the offline reset, backup, retained purchases, and repeat protection.

`tools/reset-coin-balances.py` defaults to a read-only summary. Its explicit apply mode requires stopped web writers and a private backup directory. It snapshots the SQLite database before an atomic reset of the active coin ledger, legacy user balances, and retired coin snapshots. Accounts, inventory, daily streaks, and lifetime earnings are retained. The completion marker prevents future deployments from resetting balances again. This tool is manual and is never run automatically by server startup or deploy.

Wallet links use `/coins/?v=2` to bypass browser-cached permanent redirects from the previously retired coins page.
