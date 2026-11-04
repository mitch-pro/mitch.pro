# Holiday themes

Two seasonal looks, switched on by date:

| Theme | Active | Look |
| --- | --- | --- |
| Thanksgiving | Nov 1 through Thanksgiving Day | warm cream and amber, falling leaves |
| Christmas | the day after Thanksgiving through Dec 26 | red and evergreen, falling snow |

Thanksgiving is the fourth Thursday of November (Nov 26 in 2026, so Christmas
starts Nov 27 that year). The dates use the visitor's local calendar.

## How it works

- `webserver/site-theme.js` (`holidayFor`, `applyHoliday`) sets
  `data-holiday="thanksgiving|christmas"` on `<html>` before first paint on every
  page that loads the site theme.
- `webserver/theme-tokens.css` has the palettes, for light and dark mode, under
  `:root[data-holiday=…]`. They override the same `--t-*` tokens the rest of the
  site uses, so pages follow without changes. A custom accent chosen in
  Preferences is an inline style on `<html>` and still wins over the holiday
  accent.
- `webserver/holiday-fx.js` draws the leaves or snow on a pointer-transparent
  canvas. It is loaded only while a holiday is active, pauses when the tab is
  hidden, and does not run with reduced motion, with Animations off in
  Preferences, or on `/vms/desktop`.
- Preferences → Appearance → Holiday theme: Automatic or Off (stored as
  `theme_holiday` in local storage, like the other appearance settings).

## Previewing

Add `?holiday=thanksgiving`, `?holiday=christmas` or `?holiday=none` to a page's
address to force a theme for that visit. `node tests/test_holiday_theme.mjs`
checks the date windows, including years where Thanksgiving falls on the 22nd.
