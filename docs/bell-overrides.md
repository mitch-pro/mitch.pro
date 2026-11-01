# Bell schedule overrides

A one-day replacement bell schedule for a single school, set from the server
with `./set_special_day.sh` (asks which school, preset or custom periods, and
today/tomorrow) and removed with `./clear_special_day.sh <school|all>`.

`data/bell_overrides.json` is `{"overrides": [{school, date, name,
special_event, schedule: {lunch1, lunch2}}]}`. The older single-object format
still reads as a Woodcreek override and is converted the next time the script
saves. Overrides for other days are dropped when the script saves.

`GET /api/bell/override?school=<key>` returns `override` (that school's entry,
Woodcreek when no school is given) and `overrides` (all entries). The
rjuhsd.school page asks for its own school and applies the entry only when the
date matches today, ahead of the slower school-info request.
