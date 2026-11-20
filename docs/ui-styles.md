# Site styles: mitch.pro and Tylerium

Signed-in accounts choose between two looks:

- **mitch.pro** is the default and is marked recommended. It's the site as it was before the Tylerium redesign: DM Sans, the pink accent, and the classic homepage and navigation.
- **Tylerium** is the charcoal and blue palette with Pixelify Sans and JetBrains Mono, the floating masthead, the Vanta cloud background, and its own homepage and Rankings page.

The games portal and VM Lab use the same redesigned layout in both styles. Only their colors and fonts follow the style.

Both styles follow the OS light or dark setting until a mode is picked in Preferences. Holiday themes are on by default in both, and while one is active its palette replaces either style's colors (`theme-tokens.css`, [holiday-themes.md](holiday-themes.md)).

## Where each style applies

`mitch-server/src/ui_style.rs` (`for_page`) decides for every HTML page:

| Request | Style |
| --- | --- |
| rjuhsd.school (and school mirrors), the pickle host | mitch.pro |
| Public pages: `index-sales`, `enroll`, `claim`, `password`, `magic-link`, `appeal`, `support`, `unsubscribe`, `privacy`, `use-agreement`, `cookies`, `copyright`, `rjuhsd` (first path segment, `MITCH_ONLY_SECTIONS`) | mitch.pro |
| Signed out | mitch.pro |
| Signed in, saved choice | the saved choice |
| Signed in, never chose | mitch.pro, and the chooser opens |

"Signed in" is the same full check the HTML pipeline uses: a valid, unrevoked sid with a matching password cookie.

## Storage and API

The choice is stored on the account in the `ui_style_choices` table (`email` primary key, `style` limited to `mitch` or `tylerium` by a `CHECK`, `chosen_at`). No row means the account has never chosen. The row is erased with the account (`account_deletion::EMAIL_TABLES`) and moves with an email change (`account_email::TABLES`). Storage code is in `mitch_lib::ui_style`.

- `GET /api/me/ui-style` returns `{style, chosen}`, or 401 if signed out.
- `POST /api/me/ui-style` takes `{style: "mitch" | "tylerium"}` and returns 400 for any other value. It goes through the site's normal CSRF check, so callers send `X-Mitch-Requested-With: 1`.

## The chooser

When an account has no saved choice, the pipeline adds `<script src="/ui-style.js" data-ask="1">` to ordinary signed-in pages, but not to embedded or onboarding pages or to the pages listed above. The script opens a modal with a small preview of each homepage. The previews are drawn in CSS from each style's palette and fonts, so they follow light or dark mode without loading either page. mitch.pro is selected and labelled "(recommended)". Escape doesn't close the modal: Continue saves the selection and reloads the page if the style changed. If the save fails, the modal stays open with an error.

You can change the style at any time under Preferences → Appearance → Site style. In Tylerium, the homepage's Customize dialog (`background.js`) also has a **Switch to mitch.pro** button. That control only appears for signed-in accounts, and never on rjuhsd.school. `/ui-style.js` also exposes `window.MitchUiStyle` (`current`, `status`, `save`), which Preferences uses.

## How a Tylerium page is built

The pipeline (`pipeline.rs`), for pages rendered in Tylerium:

1. Adds `data-ui="tylerium"` to the `<html>` tag (`ui_style::mark_html`). Scripts and CSS key off this attribute.
2. Links `/tylerium/tokens.css`, which holds the palettes and font tokens. Ordinary pages also get `/tylerium/tylerium.css`, which adds the masthead, larger type and 44px touch targets. The standalone games portal only gets the tokens, because it draws its own chrome.
3. Adds `/background.css` and `/background.js` (the cloud background). mitch.pro pages never get these.

The Matrix chat client (`/matrix/`) skips the pipeline, so its handler applies the style itself with `ui_style::apply_standalone`. This marks `<html>` and links `tokens.css` only, and the chat page has no cloud background. A small script in `matrix/index.html` switches its site bar to the Tylerium link order, and `matrix-galaxy.css` styles the wordmark.

Some pages linked from the homepage stay mitch.pro on purpose: Help (`/support/`) and the legal pages are public. Schedule (`/bell/`) is served on mitch.pro itself and follows the account's style; only the `/rjuhsd/bell` alias and `/bell` on the school host redirect to rjuhsd.school. Movies (`/proxy/movies/`) is a proxied third-party app with no site chrome in either style.

The handler serves the Tylerium variant of a page when `webserver/tylerium/<path>` exists (`ui_style::overlay_path`). This covers the homepage (`tylerium/index.html` with `tylerium/home.css`, `home-friends.js` and `home-dashboard.js`) and Rankings (`tylerium/leaderboard/`). `leaderboard/leaderboard.js` is shared by both styles and by the Tylerium homepage's top-three preview.

The client scripts check the same attribute:

- `site-theme.js` sets the default font (DM Sans or Pixelify Sans) and the corner radii. A font picked in Preferences applies in both styles.
- `app-shell.js` picks the navigation. Tylerium puts Games and Rankings first, uses the Rankings dock item and adds the `.mitch-masthead` class.
- `theme.js` (legacy, used by the retro games page) switches palettes and skips the mountain wallpaper rollout.

## Specificity

`tokens.css` palettes use `:root[data-ui="tylerium"][data-theme=…]:not([data-holiday])`. This outranks the light and dark blocks in `theme-tokens.css` and steps aside during a holiday theme. `site-theme.js` always sets `data-theme` to the effective mode (manual or OS), so no media-query copies are needed. `tylerium.css` wraps its rules in `:where(html[data-ui="tylerium"])`, so they don't outrank page styles, matching how they behaved when they lived in `app.css`.

## Adding to a style

- Shared tokens that markup in both styles needs (`--t-brand`, `--t-heading`, `--t-mono`, `--t-brand-tracking`, `--site-content-width`) are defined for mitch.pro in `theme-tokens.css` and overridden in `tylerium/tokens.css`.
- To give a page a Tylerium-only layout, add `webserver/tylerium/<same path>` and point its assets at `/tylerium/...`. Don't fork pages that should look the same in both styles.
- Keep both styles readable in light and dark mode.
