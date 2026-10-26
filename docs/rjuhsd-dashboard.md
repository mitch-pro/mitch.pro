# RJUHSD dashboard

The public school hub is `webserver/rjuhsd/index.html`. Its school-owned layout is
in `webserver/rjuhsd-assets/refresh.css`; the older school stylesheets are no
longer loaded by this page. Colors come from the shared site theme (`/theme-tokens.css`, the palette split out of `app.css`, plus `/site-theme.js`), so the page follows the site's pink accent, any custom accent chosen in preferences, and the saved light/dark mode. The page no longer loads the legacy `theme.js` engine, and its theme button calls `window.__siteTheme.setMode`. The hub uses neutral surfaces and the selected campus's colors.
No build or additional runtime dependency is needed.

The "by" pill in the header links to the `primary` domain from `data/site.json`
(read through `/api/site-info`); the hero and live card carry no school-logo
background art.

`webserver/bell/schedule.js` holds the published bell times;
`webserver/rjuhsd-assets/calendar.js` resolves district breaks, special schedules,
and official calendar events. `app.js` continues to consume `/api/school-info`
and `/api/bell/override`, and overlays regular-day times scraped from each
school's own site (`/api/bell/published`; see [live bell schedules](bell-sync.md)).
The lunch card also reads `/api/school-menu`; see
[school lunch menus](school-lunch-menu.md).

One Pacific-time clock updates the countdown every second. Timeline rows refresh
at minute boundaries and their active progress updates with the same clock.
Before-school, passing-period, class, lunch, dismissal, no-school and manual
preview states are separate. A manual preview never claims a live class.
Next-school-day information searches the next 45 dates through the same resolver,
including district breaks, and never invents a day outside its known calendar.
Optional Period 0, combined lunch and each campus's saved lunch remain part of
schedule resolution. Explicit overrides apply only to their target school; legacy
unscoped overrides apply to Woodcreek. Empty no-school schedules never claim a
combined lunch. Changing a school clears calendar/override state before
loading that school's information; late responses cannot overwrite a new school.

Calendar date buttons filter the agenda, and Today restores upcoming events.
The lunch dialog supports Escape, backdrop/close dismissal, focus trapping and
restoring the invoking control. Lunch selections remain local to the device.
Calendar fetch failures distinguish published weekly times from verified dates.
School links use mitchdog.com directly. Link labels and time ranges use plain
words instead of decorative arrows or dash punctuation.

The install banner in `webserver/pwa-install.js` uses the school palette and
brief installation copy. Supported browsers receive their native install prompt;
iOS receives Share / Add to Home Screen instructions. Dismissing the instructions
stays on the school page. The banner does not promise offline access or alerts.

Run the focused browser regression suite from the repository root:

```sh
node tests/test_rjuhsd_refresh_browser.mjs
```

It requires the existing `@playwright/test` development dependency and Chromium.
`PLAYWRIGHT_MODULE` can point to an externally installed ESM entry point and
`PLAYWRIGHT_EXECUTABLE_PATH` can select the browser. The suite serves a disposable
static fixture and mocks calendar/weather requests. It checks bells and exact
boundaries, midnight rollover, manual previews, lunches, overrides, holidays,
campus switching, keyboard interaction, native/manual installation, canonical links, and both themes at 320–1440 pixels.
Screenshots go to `/tmp`; `RJUHSD_FULL_SCREENSHOTS=1` also captures full pages.

The page links only `theme-tokens.css`, not `app.css`: `app.css` carries global element rules (for example `svg { max-width: 100% }`, which collapsed the theme button's icon to under 1px). The server injects `app.css` into any page that lacks it, so `refresh.css` must stay robust to those rules.

Layout is flat: sections sit on the page background and are separated by hairlines, as on the main site. The live countdown keeps a panel, and the weather is a compact line in the hero's date row whose forecast opens as a popover (it used to be a fixed card below the footer).

`html` and `body` set `background: var(--paper)` (the shared `--t-bg`) explicitly. Without it the page background was transparent and the browser painted its own canvas, which in dark mode is not the site's `#17191b`, so the school hub looked like a different site. The header and its pink-underline navigation follow the main site's top bar.

The Countdowns section lists days until the district milestones in `calendar.js` (`district.milestones`: end of the 1st term, Dec 18, and the last day of school, May 27, from the board-approved 2026-27 calendar), plus any finals, semester or term dates a school posts on its own calendar. The district PDF does not publish individual finals dates, so none are invented. Update `milestones` each year with the new calendar.
