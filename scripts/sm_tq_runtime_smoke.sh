#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "--print-checklist" ]]; then
  cat docs/codex-runs/SM_TQ_20260516/CHECKLISTS/MANUAL_RUNTIME_SMOKE_CHECKLIST.md
  exit 0
fi
cat <<'MSG'
This script is intentionally not a fake GUI driver.
Run the manual runtime smoke checklist and attach evidence under:
  docs/codex-runs/SM_TQ_20260516/runtime-smoke/
Skipped runtime smoke blocks release_ready=true.
MSG
