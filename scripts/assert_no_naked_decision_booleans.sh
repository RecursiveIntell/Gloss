#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-.}"
cd "$ROOT"

if rg --hidden -g '!target' -n 'pub[[:space:]]+fn[[:space:]]+(should_allow|can_apply|is_safe|allow|decide|evaluate)[^(]*\([^)]*\)[[:space:]]*->[[:space:]]*bool\b' crates; then
  echo "naked decision boolean API detected" >&2
  exit 1
fi

echo "no naked decision boolean APIs found"
