#!/usr/bin/env python3
"""Interactive wizard that sets a one-day bell schedule override for a school.

Writes data/bell_overrides.json, which the server serves from
GET /api/bell/override (see docs/bell-overrides.md). Custom schedules can be
saved as presets in data/bell_presets.json. Run it through ./set_special_day.sh.

Stdlib only (Python 3.9+, for zoneinfo).
"""
import json
import sys
from datetime import datetime, timedelta
from pathlib import Path
from zoneinfo import ZoneInfo

DATA_DIR = Path(__file__).resolve().parent.parent / "data"
PRESETS_PATH = DATA_DIR / "bell_presets.json"
OVERRIDE_PATH = DATA_DIR / "bell_overrides.json"

SCHOOLS = {
    "woodcreek": "Woodcreek",
    "roseville": "Roseville",
    "granitebay": "Granite Bay",
    "antelope": "Antelope",
    "westpark": "West Park",
    "oakmont": "Oakmont",
}


def period(name, start, end, duration):
    return {"name": name, "start": start, "end": end, "duration": duration}


# Official WHS alternate bell schedules.
BUILTIN_PRESETS = [
    {
        "name": "Non-Pack Day",
        "schedule": {
            "lunch1": [
                period("Period 1", "08:30", "09:59", 89),
                period("Period 2", "10:07", "11:36", 89),
                period("Lunch 1", "11:36", "12:06", 30),
                period("Period 3", "12:14", "13:43", 89),
                period("Period 4", "13:51", "15:20", 89),
            ],
            "lunch2": [
                period("Period 1", "08:30", "09:59", 89),
                period("Period 2", "10:07", "11:36", 89),
                period("Period 3", "11:44", "13:13", 89),
                period("Lunch 2", "13:13", "13:43", 30),
                period("Period 4", "13:51", "15:20", 89),
            ],
        },
    },
    {
        "name": "Minimum Day",
        "schedule": {
            "lunch1": [
                period("Period 1", "08:30", "09:26", 56),
                period("Period 2", "09:34", "10:30", 56),
                period("Period 3", "10:38", "11:34", 56),
                period("Break / Lunch", "11:34", "11:46", 12),
                period("Period 4", "11:54", "12:50", 56),
            ],
            "lunch2": [
                period("Period 1", "08:30", "09:26", 56),
                period("Period 2", "09:34", "10:30", 56),
                period("Period 3", "10:38", "11:34", 56),
                period("Break / Lunch", "11:34", "11:46", 12),
                period("Period 4", "11:54", "12:50", 56),
            ],
        },
    },
    {
        "name": "Finals",
        "schedule": {
            "lunch1": [
                period("Period 1/3", "08:30", "10:30", 120),
                period("Break / Lunch", "10:30", "10:42", 12),
                period("Period 2/4", "10:50", "12:50", 120),
            ],
            "lunch2": [
                period("Period 1/3", "08:30", "10:30", 120),
                period("Break / Lunch", "10:30", "10:42", 12),
                period("Period 2/4", "10:50", "12:50", 120),
            ],
        },
    },
]


def ask(prompt):
    try:
        return input(prompt)
    except EOFError:
        print("\nInput ended. Nothing was changed.")
        sys.exit(1)


def minutes(hhmm):
    hours, mins = hhmm.strip().split(":")
    return int(hours) * 60 + int(mins)


def load_json(path, default):
    try:
        return json.loads(path.read_text())
    except FileNotFoundError:
        return default
    except (OSError, ValueError) as err:
        print(f"Error loading {path}: {err}")
        return default


def load_overrides():
    """Read bell_overrides.json: {"overrides": [...]} or the older single
    Woodcreek object, which is converted on the next write."""
    doc = load_json(OVERRIDE_PATH, {})
    if isinstance(doc, dict):
        if isinstance(doc.get("overrides"), list):
            return doc["overrides"]
        if doc.get("date"):
            return [{"school": "woodcreek", **doc}]
    return []


def save_preset(name, schedule):
    presets = load_json(PRESETS_PATH, [])
    for preset in presets:
        if preset["name"].lower() == name.lower():
            preset["schedule"] = schedule
            break
    else:
        presets.append({"name": name, "schedule": schedule})
    DATA_DIR.mkdir(parents=True, exist_ok=True)
    PRESETS_PATH.write_text(json.dumps(presets, indent=2))


def build_schedule():
    """Prompt for periods one at a time. Returns (name, schedule)."""
    name = ask("\nEnter schedule name (e.g. Rally Day, Assembly Day): ").strip() or "Special Day"
    lunch1, lunch2 = [], []
    while True:
        period_name = ask("\nPeriod Name (e.g. Period 1, Lunch 1, Rally): ").strip()
        if not period_name:
            print("Period name is required, please try again.")
            continue
        start = ask("Start Time (24h format HH:MM, e.g. 08:30): ").strip()
        end = ask("End Time (24h format HH:MM, e.g. 09:40): ").strip()
        try:
            duration = minutes(end) - minutes(start)
        except ValueError:
            print("Times must look like HH:MM. This period was skipped.")
            continue
        group = ask("Which lunch does this period apply to? [1 = Lunch 1, 2 = Lunch 2, 3 = Both]: ").strip()
        entry = period(period_name, start, end, duration)
        if group == "1":
            lunch1.append(entry)
        elif group == "2":
            lunch2.append(entry)
        else:
            lunch1.append(entry)
            lunch2.append(dict(entry))
        if ask("\nAdd another period? (y/n): ").strip().lower() != "y":
            break
    lunch1.sort(key=lambda p: minutes(p["start"]))
    lunch2.sort(key=lambda p: minutes(p["start"]))
    schedule = {"lunch1": lunch1, "lunch2": lunch2}

    if ask("\nWould you like to save this new schedule as a preset for future use? (y/n): ").strip().lower() == "y":
        preset_name = ask(f"Enter preset name [Default: {name}]: ").strip() or name
        save_preset(preset_name, schedule)
        print(f'✓ Preset "{preset_name}" successfully saved to {PRESETS_PATH}!')
        name = preset_name
    return name, schedule


def main():
    print("\n=============================================")
    print("      SPECIAL BELL SCHEDULE OVERRIDE WIZARD  ")
    print("=============================================")

    keys = list(SCHOOLS)
    print("\nWhich school is this for?")
    for index, key in enumerate(keys, 1):
        print(f"{index}) {SCHOOLS[key]}")
    choice = ask(f"Select school [1-{len(keys)}]: ").strip()
    if not (choice.isdigit() and 1 <= int(choice) <= len(keys)):
        print("Invalid school. Nothing was changed.")
        return
    school = keys[int(choice) - 1]
    print(f"\nSchool: {SCHOOLS[school]}")

    presets = BUILTIN_PRESETS + load_json(PRESETS_PATH, [])
    print("\nChoose schedule configuration option:")
    print("1) Use a Preset schedule (Built-in or Custom)")
    print("2) Create a new schedule from scratch")
    mode = ask("Select option [1-2]: ").strip()

    schedule_name, schedule = "Special Day", None
    if mode == "1":
        print("\n--- AVAILABLE PRESETS ---")
        for index, preset in enumerate(presets, 1):
            kind = "Built-in" if index <= len(BUILTIN_PRESETS) else "Custom"
            print(f"{index}) {preset['name']} ({kind})")
        pick = ask(f"Select preset [1-{len(presets)}]: ").strip()
        if pick.isdigit() and 1 <= int(pick) <= len(presets):
            schedule_name = presets[int(pick) - 1]["name"]
            schedule = presets[int(pick) - 1]["schedule"]
            print(f"\nSelected Preset: {schedule_name}")
        else:
            print("Invalid selection. Defaulting to scratch creation.")
    if schedule is None:
        print("\n--- CREATE NEW SCHEDULE FROM SCRATCH ---")
        schedule_name, schedule = build_schedule()

    print("\nWhen is this special schedule for?")
    print("1) Today")
    print("2) Tomorrow")
    day_choice = ask("Select option [1-2]: ").strip()
    la_today = datetime.now(ZoneInfo("America/Los_Angeles")).date()
    target = la_today + timedelta(days=1) if day_choice == "2" else la_today
    print(f"\nSelected Target Date: {target.isoformat()}")

    event = ask("\nEnter any special event notices (optional, e.g. Assembly in gym): ").strip()
    override = {
        "school": school,
        "date": target.isoformat(),
        "name": schedule_name,
        "special_event": event or None,
        "schedule": schedule,
    }

    # Keep other schools' overrides that haven't passed; replace this school's.
    kept = [
        o for o in load_overrides()
        if o.get("school", "woodcreek") != school and o.get("date", "") >= la_today.isoformat()
    ]
    DATA_DIR.mkdir(parents=True, exist_ok=True)
    OVERRIDE_PATH.write_text(json.dumps({"overrides": kept + [override]}, indent=2))

    print(f"\n✓ Success! Special schedule successfully loaded into {OVERRIDE_PATH}")
    print(f'{SCHOOLS[school]} will show the "{schedule_name}" schedule on {target.isoformat()}!')


if __name__ == "__main__":
    main()
