# Bell schedule overrides

A one-day replacement bell schedule for a single school. Admins and moderators set it
at `/admin/special-schedule/` (linked from the admin dashboard and the moderator panel).
The page picks a school, a day, a name, an optional event notice and the periods for each
lunch, starting from a preset (Non-Pack Day, Minimum Day, Finals) or a blank schedule. It
lists the active schedules with Edit and Clear. This replaces the old
`set_special_day.sh` / `clear_special_day.sh` scripts, which wrote a file the server no
longer reads.

## Storage

Tables `bell_overrides` (one row per school) and `bell_override_periods` (the periods
of each lunch, in order). Code: `rust/crates/mitch-lib/src/bell_overrides.rs`. The old
`data/bell_overrides.json` is imported once on first boot (the `{"overrides": [...]}` list or
the older single Woodcreek object), recorded in `metadata`, so clearing every override
never brings the file back. Nothing writes the file any more.

The server checks everything the page checks: a known school, a real date from today
(Los Angeles time) to 90 days ahead, a name up to 80 characters, an event notice up to
200, at most 20 periods per lunch, `HH:MM` times with each end after its start. A period's
`duration` is computed from its times. Saving a school replaces its schedule and also
drops other schools' schedules whose day has passed.

## API

- `GET /api/admin/special-schedule` returns `today` (Los Angeles), `schools`, `presets`
  and `overrides`.
- `POST /api/admin/special-schedule` with `{school, date, name, specialEvent, lunch1,
  lunch2}`, each lunch a list of `{name, start, end}`.
- `POST /api/admin/special-schedule/clear` with `{school}`, or `{school: "all"}`.

Admins and moderators only. Admins send the usual `X-Admin-Passphrase`; moderators are
exempt from the passphrase gate, as for every admin API. Saving and clearing are written
to the admin log as `set_special_schedule` and `clear_special_schedule`.

## Public read

`GET /api/bell/override?school=<key>` returns `override` (that school's entry, Woodcreek
when no school is given) and `overrides` (all entries), each
`{school, date, name, special_event, schedule: {lunch1, lunch2}}`. The rjuhsd.school page
asks for its own school and applies the entry only when the date matches today, ahead of
the slower school-info request.

Custom presets saved from the old script (`data/bell_presets.json`) are not carried over;
the page has the three built-in presets and a blank schedule.
