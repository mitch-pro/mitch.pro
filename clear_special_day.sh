#!/usr/bin/env bash
# clear_special_day.sh — Remove the bell schedule override for one school.
# Usage: ./clear_special_day.sh <school>   (woodcreek, roseville, granitebay, antelope, westpark, oakmont)
#        ./clear_special_day.sh all        (remove every school's override)

BASE_DIR=$(dirname "$0")
OVERRIDE_PATH="$BASE_DIR/data/bell_overrides.json"
SCHOOL="$1"

if [ -z "$SCHOOL" ]; then
  echo "Usage: $0 <school|all>" >&2
  exit 1
fi

if [ ! -f "$OVERRIDE_PATH" ]; then
  echo "No active bell schedule overrides found."
  exit 0
fi

if [ "$SCHOOL" = "all" ]; then
  rm "$OVERRIDE_PATH"
  echo "✓ Cleared every bell schedule override."
  exit 0
fi

exec python3 "$BASE_DIR/tools/clear_special_day.py" "$OVERRIDE_PATH" "$SCHOOL"
