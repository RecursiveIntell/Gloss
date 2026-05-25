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

if grep -RInE '\b(f32|f64)\b' "${scan_paths[@]}" --exclude-dir=target; then
  echo "durable float score type found" >&2
  exit 1
fi

echo "no f32/f64 under production source roots"
