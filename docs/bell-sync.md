# Live bell schedules

rjuhsd.school ships bundled bell times (`webserver/bell/schedule.js`). The server
also reads each school's own bell-schedule page and the dashboard overlays
those regular-day times on the bundled ones, so a mid-year change on the school
site reaches students without a code change.

## Pipeline

1. `mitch_lib::bell_sync::parse_bell_page` parses the Finalsite page
   (`{school}.rjuhsd.us/about/bell-schedule(s)`; Antelope's path is singular).
   Each schedule is a `<table>` with a `<caption>` ("1st Lunch", "Minimum Day")
   under a group heading (`<h2 class="fsElementTitle">`, e.g. "Tuesday Schedule"
   or "(M, Tu, Th, F) Regular Day"). Rows are `Period | Start | End` in 12-hour
   times with no AM/PM; hours 1-6 are read as afternoon.
2. The parser is strict. A table is kept only when every row parses, each period
   ends after it starts, and periods do not overlap. A redesigned page therefore
   produces no tables instead of wrong bells.
3. `replace_school` stores the tables in `bell_published_periods` and records the
   attempt in `bell_sync_status` (one row per school: source URL, last check,
   last success, error, content hash) inside one transaction. An empty parse or a
   failed fetch never replaces stored rows; it only updates the status row.
4. `routes/bell_sync.rs` runs the sync: a worker refreshes all six schools every
   six hours (first pass one minute after startup, 5 s between schools).
   `GET /api/bell/published?school=<key>` also fetches inline when a school has
   never been synced, a successful sync is over 12 h old, or a failed one is
   over 15 min old. Only the six hardcoded `*.rjuhsd.us` origins are fetched.

## API

`GET /api/bell/published?school=woodcreek` is public and returns `school`,
`source_url`, `fetched_at` / `checked_at` (epoch ms), `ok`, and `tables`. Each
table has `group`, `label`, `periods` (`name`, `start`, `end` as `HH:MM`,
`duration` in minutes), and:

- `days`: weekdays 1 (Mon) to 5 (Fri) the table is the regular schedule for, or
  `null`;
- `lunch`: `"1"`, `"2"`, or `"all"` (single-lunch days such as Wednesday
  collaboration), or `null`.

A table gets `days`/`lunch` only when it is a plain "1st Lunch"/"2nd Lunch"
table under a weekday group, or its caption names its own weekday ("(Wed)
Collaboration"). Minimum days, rallies, finals and unlabeled tables are returned
with `null` and are not applied by the client.

## Client

`applyPublishedBells` in `webserver/rjuhsd-assets/app.js` replaces
`RJUHSD_SCHOOL_DATA[school].days[day][lunch]` for tables with `days` and `lunch`
and at least three periods, then the dashboard re-renders. If the endpoint is
unavailable the bundled times are used unchanged. Explicit overrides from
`/api/bell/override` and the calendar's special schedules still win, because
they are resolved afterward by `calendar.js`.

## Maintenance

If a school restyles its page and the parser stops matching, `bell_sync_status`
shows `ok = 0` with the error and the bundled/last-good times stay in effect.
Update the regexes in `bell_sync.rs` and add the new markup to its tests.
