#!/usr/bin/env bash
# set_special_day.sh — Set a custom bell schedule override for one school (asks which)

BASE_DIR=$(dirname "$0")
exec python3 "$BASE_DIR/tools/set_special_day.py"
