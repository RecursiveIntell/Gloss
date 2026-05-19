#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-.}"
cd "$ROOT"

scan_paths=()
for path in crates policies schemas/generated; do
  if [[ -e "$path" ]]; then
    scan_paths+=("$path")
  fi
done

if [[ "${#scan_paths[@]}" -eq 0 ]]; then
  echo "no production paths found"
  exit 0
fi

pattern='FEUT|EEG|theta|gamma|alpha band|intelligence field|P[[:space:]]*=[[:space:]]*NP|Clay proof|Riemann proof|universal entropy law|black.?hole echo|neuro.?calibrated'

if grep -RInE "$pattern" "${scan_paths[@]}"; then
  echo "forbidden historical terminology found in production paths" >&2
  exit 1
fi

echo "no forbidden historical terminology in production paths"
