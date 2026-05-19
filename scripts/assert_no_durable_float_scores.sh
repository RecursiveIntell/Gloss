#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-.}"
cd "$ROOT"

if grep -RInE '\b(f32|f64)\b' crates --exclude-dir=target; then
  echo "durable float score type found" >&2
  exit 1
fi

echo "no f32/f64 under crates"
