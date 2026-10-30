# UI Design Standards

This document establishes the UI design and frontend architectural standards for `mitchdog.com` (`mitch.pro`). All pages across the site must follow these conventions to maintain visual coherence and operational integrity.

## 1. Homepage & App-Shell Consistency

Every user-facing and internal page on the site must adhere to the design language established by the homepage and shared app-shell:

1. **Shared Masthead & App Shell:**
   - Every ordinary page must load `/app.css` and `/app-shell.js`.
   - The body tag should use `class="app-shell"` so that the unified masthead (`#app-topbar`) is rendered consistently with the brand mark, navigation links, user profile, and MitchCoins wallet.
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
     - `--t-ac`: Accent color (brand pink/magenta or theme selection)
     - `--t-ac-ink`: High-contrast text on accent buttons
     - `--t-radius`: Standard card corner radius (default: 10px)
     - `--t-radius-sm`: Input and button corner radius (default: 7px)

3. **Typography:**
   - Use `var(--t-font)` (`DM Sans`, system-ui, sans-serif).
   - Headings use bold weight (`font-weight: 700; letter-spacing: -0.03em;`).

4. **Cards, Panels & Lists:**
   - Container shells use `.app-main`, `.shell`, or `.legal-shell` with responsive max-width (`1000px` - `1200px`).
   - Cards use `.panel`, `.card`, or semantic sections with `background: var(--t-bg2); border: 1px solid var(--t-bd); border-radius: var(--t-radius);`.

## 2. No Orphan Endpoints

- **Backend + Frontend Symmetry:** Any new feature, moderation tool, or settings toggle exposed by the backend or admin API must ship with its corresponding frontend UI in the same commit.
- Leaving a note like *"there is no UI page for this yet"* is prohibited.
- If an endpoint requires administrative or moderator action (such as the Matrix outside accounts queue), add controls to the [Moderator Panel](/moderator/) and [Admin Panel](/admin/) simultaneously.

## 3. Light and Dark Mode Verification

- Always test UI changes in both dark mode (default) and light mode (`html.theme-light`).
- Contrast must be verified for all text, borders, buttons, and status indicators.
