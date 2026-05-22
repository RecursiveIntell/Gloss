#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-.}"
RUN_ID="GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519"
RUN_DIR="${ROOT}/docs/codex-runs/${RUN_ID}"
OUT_DIR="${RUN_DIR}/package_replay"
mkdir -p "${OUT_DIR}"

python3 "${ROOT}/scripts/p33_release_final_gate.py" --repo "${ROOT}" --run-id "${RUN_ID}" > "${OUT_DIR}/pre_package_final_gate.json" || {
  cat > "${OUT_DIR}/NEEDS_ARCHIVE_SELECTION.txt" <<'EOF'
P33 package replay is blocked before archive replay.

Reason: final release gate is not passing. Do not perform or claim fresh-unzip
release proof while desktop RAG smoke and final release readiness remain blocked.
EOF
  cat "${OUT_DIR}/pre_package_final_gate.json"
  exit 1
}

ARCHIVE="${2:-}"
if [[ -z "${ARCHIVE}" || ! -f "${ARCHIVE}" ]]; then
  cat > "${OUT_DIR}/NEEDS_ARCHIVE_SELECTION.txt" <<'EOF'
P33 package replay needs an explicit generated archive path as the second argument.
Example:
  bash scripts/p33_package_replay_gate.sh . ./Gloss-generic-next-codex-context-YYYYMMDD.zip
EOF
  exit 2
fi

bash "${ROOT}/scripts/fresh_unzip_replay.sh" "${ARCHIVE}" | tee "${OUT_DIR}/fresh_unzip_replay.log"
python3 - <<'PY' "${OUT_DIR}/fresh_unzip_replay.json" "${ARCHIVE}"
import datetime as dt
import json
import sys

out, archive = sys.argv[1], sys.argv[2]
receipt = {
    "schema": "GlossP33FreshUnzipReplayReceiptV1",
    "recorded_time": dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "archive": archive,
    "passed": True,
}
open(out, "w", encoding="utf-8").write(json.dumps(receipt, indent=2))
PY
