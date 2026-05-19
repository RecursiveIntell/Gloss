#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-.}"
cd "$ROOT"

cargo run -p scr-cli -- generate-schemas schemas/generated >/dev/null
echo "schemas generated"
