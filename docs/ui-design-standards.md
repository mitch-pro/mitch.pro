# UI Design Standards

This document establishes the UI design and frontend architectural standards for `mitchdog.com` (`mitch.pro`). All pages across the site must follow these conventions to maintain visual coherence and operational integrity.

## 1. Homepage & App-Shell Consistency

Every user-facing and internal page on the site must adhere to the design language established by the homepage and shared app-shell:

1. **Shared Masthead & App Shell:**
   - Every ordinary page must load `/app.css` and `/app-shell.js`.
   - The body tag should use `class="app-shell"` so that the unified masthead (`#app-topbar`) is rendered consistently with the brand mark, navigation links, user profile, and MitchCoins wallet.
   - Mitch's masthead uses the floating layout shared with the homepage. Games and Rankings are primary destinations on desktop and in the mobile dock.
   - For standalone fullscreen experiences (e.g. game or emulator players), provide an overlay or topbar that matches the site masthead instead of inventing a custom header.

2. **Theme Tokens (`app.css` & `theme-tokens.css`):**
   - Never use raw hardcoded hex colors (e.g., `#fff`, `#000`, `#222`) for core backgrounds, borders, or text.
   - Use CSS color tokens:
     - `--t-bg`: Primary background
     - `--t-bg2`: Elevated card/panel background
     - `--t-bg3`: Hover, subtle element, or input background
     - `--t-fg`: Primary text
     - `--t-fg2`: Secondary / muted text
     - `--t-fg3`: Faint caption text
     - `--t-bd`: Standard border
     - `--t-bda`: Accentuated or hover border
     - `--t-ac`: Accent color (blue by default, with seasonal and user-selected alternatives)
     - `--t-ac-ink`: High-contrast text on accent buttons
     - `--t-radius`: Standard card corner radius (default: 10px)
     - `--t-radius-sm`: Input and button corner radius (default: 7px)

3. **Typography:**
   - Use `var(--t-font)` for copy, controls, and headings. Pixelify Sans is the default and Preferences offers other fonts.
   - Use `var(--t-brand)` for the Mitch wordmark and `var(--t-mono)` for code. JetBrains Mono and Pixelify Sans are served locally with their font licenses.

4. **Cards, Panels & Lists:**
   - Container shells use `.app-main`, `.shell`, or `.legal-shell` with responsive max-width (`1000px` - `1200px`).
   - Cards use `.panel`, `.card`, or semantic sections with `background: var(--t-bg2); border: 1px solid var(--t-bd); border-radius: var(--t-radius);`.

### Home, Games, and Rankings

The Friends strip sits directly below the masthead. The welcome panel places the floating Mitch mark to the left of the animated hello and keeps the local clock on the right. Games spans the shortcut area above Matrix, Cloud Linux, and Movies. Rankings appears below the welcome panel, and the Pages directory arranges Community, MitchCoins, and Account links in columns that stack on phones. Preferences lives in the Account column. The Rankings preview requests three rows from the same leaderboard endpoint used by the full rankings page and keeps the existing profile links and cosmetics.

The games portal uses the full viewport width and adapts the number of game columns to the available space. The player fills the viewport with a compact toolbar, and the Fullscreen button expands the game itself where the browser supports it. The portal uses the same color tokens and wordmark as the homepage. The rankings page accepts a `metric` query parameter to open a category directly.

### VM Lab

The setup panel shows the default Linux desktop specs alongside the Create Computer and Custom OS controls. Each assigned computer shows its power status, connection details, and desktop controls next to CPU, memory, and storage meters. Offline meters are marked unavailable. The connection and resource panels stack on phones.

Custom OS shows selectable preset cards with logos and prices from the existing preset catalog. ISO links, uploads, and stored ISO actions are grouped separately. The icons are bundled locally. Devicon supplies the Windows, Linux Mint, Arch Linux, Kali Linux, and Chrome marks, and Tails uses its project favicon. [Devicon icons and license](https://github.com/devicons/devicon/tree/v2.17.0), [Tails project favicon](https://tails.net/favicon.ico).

### Background

Mitch uses Tylerium's charcoal Vanta Clouds preset and locally hosted Three.js and Vanta with their upstream licenses. Customize offers dark clouds, blue clouds, a solid background, and motion controls. Saved wallpaper takes precedence until the user switches presets. The layer uses the static WebP fallback with reduced motion, disabled animation, Save-Data, or unavailable WebGL. Embedded games skip the background engine.

## 2. No Orphan Endpoints

- **Backend + Frontend Symmetry:** Any new feature, moderation tool, or settings toggle exposed by the backend or admin API must ship with its corresponding frontend UI in the same commit.
- Leaving a note like *"there is no UI page for this yet"* is prohibited.
- If an endpoint requires administrative or moderator action (such as the Matrix outside accounts queue), add controls to the [Moderator Panel](/moderator/) and [Admin Panel](/admin/) simultaneously.

## 3. Light and Dark Mode Verification

- Always test UI changes in both dark and light mode. Test light mode both ways it can be reached: the OS setting (Playwright `colorScheme: 'light'`) and the manual Preferences toggle.
- Contrast must be verified for all text, borders, buttons, and status indicators.

The shared `/site-theme.js` uses dark by default. It reads the `theme` cookie for an explicit light, dark, or system setting:

- Sets `html[data-theme="light"|"dark"]` to the effective mode for `theme-tokens.css`.
- Updates `html.theme-light` when the effective mode is light. Only the explicit system setting follows OS changes.

So page CSS can key light-mode fixes off `html.theme-light` (or `html:not(.theme-light)` for dark-only colors) and they apply on every page. Prefer not needing them at all: tokens already switch, and a status color written as `color-mix(in srgb, #dc2626 70%, var(--t-fg))` reads in both modes. The legacy `theme.js` (still loaded by a few pages such as Preferences and the admin panel) also toggles `theme-light`; both scripts read the same cookie.

## 4. MitchCoin Logo

- `webserver/mitchcoin.svg` is the source for the lavender crown used in wallets, prices, VM upgrades, and game balances. Keep its purple facets fixed across themes and avoid circular clipping or color filters.
- Use `/mitchcoin.svg?v=4` for images and CSS backgrounds. Inline images use `.mitch-coin-inline`; dynamic amounts can use `.mitch-coin-amount` so updating their text preserves the icon.
- Keep `mitchcoin.png` and `mitchcoin.webp` as transparent 256×256 exports of the SVG. After changing the source, regenerate both exports and bump the logo, shared wallet script, and stylesheet versions together, including game pages that load the wallet directly.

## 5. Full-screen pages, legal pages and the footer band

- Pages with `data-shell="off"` (canvas, game portal) don't get `#app-topbar`, so `mitch-coins.js` has nowhere to put the wallet and falls back to a "← Games" bar meant for game pages. Give the page's own toolbar a `data-coin-wallet` attribute to host the wallet there instead.
- Privacy, cookies, copyright and use agreement share `/legal.css` (hero, sticky table of contents, content column). Keep legal-page styling there, not in per-page `<style>` blocks.
- The server-injected credit band (`inject_credit` in `rust/crates/mitch-server/src/inject.rs`) is the one place the "By using mitch.pro you agree…" line appears, on every host including rjuhsd.school. The in-page `#_agree_footer` is only injected where the band doesn't carry it (pages with their own `#site-credit`). Don't add another copy of that sentence to pages.

## 6. Page conventions checklist

These came out of a consistency pass over every page linked from the homepage. Check new and edited pages against them.

- **Container and header.** Wrap content in `<main class="app-main">` (add a page class for a narrower `max-width` if the content is a single column). Put the title in `<header class="page-head"><h1>…</h1><p class="page-sub">…</p></header>` and don't restyle `.page-head` per page. The masthead is not fixed-position, so don't add top padding to clear it.
- **Body class.** Put `app-shell` on `<body>` in the markup. `app-shell.js` adds it at runtime anyway, but having it up front avoids a layout jump.
- **Font and weights.** Use the shared font tokens. Headings and strong labels use semibold or bold weight. Labels are sentence case.
- **Radii.** `var(--t-radius)` for cards, panels and dialogs; `var(--t-radius-sm)` for inputs, buttons, chips and small pills. Pills that are meant to be fully round may use `999px`. No hardcoded 6–16px radii.
- **Buttons.** Use the shared button styles in `app.css` instead of page-local `.btn`/`.btn-primary`. Text on an accent background uses `var(--t-ac-ink)`, never `#fff`: a custom light accent sets the ink to black. Hover on accent buttons uses `var(--t-ac2)` rather than glows or `filter: brightness()`.
- **Inputs.** Background `var(--t-bg)` (so they read as wells inside `--t-bg2` cards), border `var(--t-bd)`, focus `var(--t-ac)`, radius `--t-radius-sm`.
- **Status colors.** Fixed reds/greens/ambers are fine for meaning, but check them on light backgrounds. Solid danger buttons use a red dark enough for white text (`#c62828` / `#dc2626`); text-only status colors either `color-mix` toward `--t-fg` or get an `html.theme-light` variant.
- **Dialogs.** Backdrops are semi-transparent so the page stays visible behind them.
- **No inline styling for states.** Put hover and focus in the stylesheet, not in `style=""` / `onmouseover` attributes.
- **Copy.** Labels describe the content or action. No tagline lines under headings unless they say something the heading doesn't, no "Looking for more?" lead-ins, no hidden marketing blocks left in the markup.
- **Cache busting.** When you change a page's stylesheet, bump its `?v=` in the HTML. With `NODE_ENV=development`, the local server checks for static file edits on each request.
