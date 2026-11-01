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

exec bun -e '
const fs = require("fs");
const [path, school] = process.argv.slice(1);
const doc = JSON.parse(fs.readFileSync(path, "utf8"));
const list = Array.isArray(doc.overrides) ? doc.overrides : (doc.date ? [{ school: "woodcreek", ...doc }] : []);
const kept = list.filter(o => (o.school || "woodcreek") !== school);
if (kept.length === list.length) { console.log("No override found for " + school + "."); process.exit(0); }
if (kept.length) fs.writeFileSync(path, JSON.stringify({ overrides: kept }, null, 2)); else fs.unlinkSync(path);
console.log("✓ Cleared the bell schedule override for " + school + ".");
' "$OVERRIDE_PATH" "$SCHOOL"
