#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-.}"
cd "$ROOT"

scan_paths=()
for path in src-tauri/src src; do
  if [[ -e "$path" ]]; then
    scan_paths+=("$path")
  fi
done

if [[ "${#scan_paths[@]}" -eq 0 ]]; then
  echo "no production paths found" >&2
  exit 1
fi

if rg --hidden -g '!target' -n 'pub[[:space:]]+fn[[:space:]]+(should_allow|can_apply|is_safe|allow|decide|evaluate)[^(]*\([^)]*\)[[:space:]]*->[[:space:]]*bool\b' "${scan_paths[@]}"; then
  echo "naked decision boolean API detected" >&2
  exit 1
fi

echo "no naked decision boolean APIs found"
