#!/usr/bin/env python3
"""Remove one school's bell schedule override from data/bell_overrides.json.

Usage (through ./clear_special_day.sh): clear_special_day.py <school>
Deletes the file when no overrides remain. Stdlib only.
"""
import json
import sys
from pathlib import Path

path, school = Path(sys.argv[1]), sys.argv[2]
doc = json.loads(path.read_text())
if isinstance(doc.get("overrides"), list):
    overrides = doc["overrides"]
elif doc.get("date"):
    overrides = [{"school": "woodcreek", **doc}]
else:
    overrides = []
kept = [o for o in overrides if o.get("school", "woodcreek") != school]
if len(kept) == len(overrides):
    print(f"No override found for {school}.")
elif kept:
    path.write_text(json.dumps({"overrides": kept}, indent=2))
    print(f"✓ Cleared the bell schedule override for {school}.")
else:
    path.unlink()
    print(f"✓ Cleared the bell schedule override for {school}.")
