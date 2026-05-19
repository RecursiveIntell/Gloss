#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
OUT="docs/codex-runs/SM_TQ_20260516/package-replay"
mkdir -p "$OUT"
cat > "$OUT/README.md" <<'MSG'
# Package replay

Run the current package/certifier flow here when available.
If skipped, final receipt must mark package_replay as skipped and release_ready=false.
MSG
if [[ -x ./z.py ]]; then
  echo "z.py exists and is executable; run package certifier manually if desired." | tee "$OUT/status.txt"
else
  echo "No executable z.py found at repo root. Package replay not run." | tee "$OUT/status.txt"
fi
