# Holiday themes

Five seasonal looks, switched on by date (the visitor's local calendar):

| Theme | Active | Look |
| --- | --- | --- |
| Halloween | Oct 15 – Oct 31 | pumpkin orange on deep purple, rising embers |
| Thanksgiving | Nov 1 – Thanksgiving Day | warm cream and amber, falling leaves |
| Christmas | the day after Thanksgiving – Dec 26 | evergreen surfaces, red accents, snow and twinkling string lights along the top edge |
| New Year's | Dec 27 – Jan 3 | midnight navy and gold, confetti |
| Valentine's | Feb 7 – Feb 14 | rose, floating hearts |

Thanksgiving is the fourth Thursday of November (Nov 26 in 2026, so Christmas
starts Nov 27 that year).

## How it works

- `webserver/site-theme.js` (`holidayFor`, `applyHoliday`) sets
  `data-holiday="thanksgiving|christmas"` on `<html>` before first paint on every
  page that loads the site theme. The server injects it into ordinary pages;
  `/matrix/` is served by the Matrix SPA fallback, which skips that injection,
  so `webserver/matrix/index.html` loads `/site-theme.js` itself. Its Cinny
  overrides (`matrix-galaxy.css`) read the same `--t-*` tokens, so the chat
  follows the holiday palette too.
- The signed-out landing page (`index-sales.html`) and sign-in (`/enroll/`) are
  onboarding pages, which the injection also skips, so they load
  `/site-theme.js` and `/theme-tokens.css` themselves. They use their own
  `--ob-*` colors; `theme-tokens.css` maps the holiday palette onto those under
  `:root[data-holiday] body.onboarding-page`, and `onboarding-theme.js` sets
  `data-theme` from the page's light/dark toggle so the matching palette is
  used. `/appeal/` (shown to banned accounts) stays plain on purpose.
- `webserver/theme-tokens.css` has the palettes, for light and dark mode, under
  `:root[data-holiday=…]`. They override the same `--t-*` tokens the rest of the
  site uses, so pages follow without changes. A custom accent chosen in
  Preferences is an inline style on `<html>` and still wins over the holiday
  accent.
- `webserver/holiday-fx.js` draws the leaves, snow, embers, confetti or hearts on a pointer-transparent
  canvas (Christmas's string lights are CSS on `<html>`'s pseudo-elements in
  `theme-tokens.css`). It is loaded only while a holiday is active, pauses when the tab is
  hidden, and does not run with reduced motion, with Animations off in
  Preferences, on `/vms/desktop`, or on the games page (`/game-portal`, which
  `/games` redirects to).
- A pill on the homepage, right below the Private window toggle, says "Don't like
  the holiday theme?" while a holiday is in season. Clicking it turns the theme off
  on every page and the pill becomes "Bring back the holiday theme"
  (`__siteTheme.setHolidayOff`). It is the same setting as the one below.
- Preferences → Appearance → Holiday theme: Automatic or Off (stored as
  `theme_holiday` in local storage, like the other appearance settings).

## Pinning a theme for a server run

`MITCH_HOLIDAY_THEME=halloween|thanksgiving|christmas|newyear|valentines|none` (environment, see `.env.example`)
makes every page of that server use that theme regardless of the date. The server
puts `window.__holidayForce` in front of `/site-theme.js` (`inject::holiday_pin_prefix`;
unknown values are dropped). The Preferences opt-out and the homepage pill still
turn it off, and `?holiday=` in the address still wins. Example for a local test
server: `MITCH_HOLIDAY_THEME=christmas PORT=8080 mitch-server`.

## Previewing

Add `?holiday=<name>` (any theme above) or `?holiday=none` to a page's
address to force a theme for that visit. `node tests/test_holiday_theme.mjs`
checks the date windows, including years where Thanksgiving falls on the 22nd.

## Also on the homepage

- A greeting on the day itself ("Happy Thanksgiving", "Merry Christmas" on Dec 24
  and 25, "Happy New Year's Eve", "Happy New Year", "Happy Valentine's Day",
  "Happy Halloween") under the hello heading while the theme is on. Previews
  (`?holiday=`, a pinned server) show the theme's own greeting.
- "Thanksgiving break in 19 days": the next break from the district calendar
  (`webserver/rjuhsd-assets/calendar.js`, the same file `rjuhsd.school` uses),
  shown when it is within 60 days. `rjuhsd.school` lists the breaks in its
  Countdowns card from the same milestones.
- An automatic update-log entry when a theme starts: `docs/site-updates.md`.

`mitch_lib::seasons` has the same date windows as `site-theme.js`; both are tested
with the same dates.

