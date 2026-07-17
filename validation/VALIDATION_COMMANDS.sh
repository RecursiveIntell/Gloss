#!/usr/bin/env bash
set -euo pipefail

RUN_ID="$(python3 - <<'PY'
import re
from pathlib import Path
text = Path('docs/codex-runs/CURRENT_RUN.md').read_text(errors='ignore')
match = re.search(r'Current run:\s*`?([^`\n]+)`?', text)
print(match.group(1).strip() if match else 'UNKNOWN_RUN')
PY
)"
RUN_DIR="docs/codex-runs/${RUN_ID}"
mkdir -p "$RUN_DIR"
LOG="$RUN_DIR/COMMANDS_RUN.log"
: > "$LOG"

run() {
  echo "
$ $*" | tee -a "$LOG"
  "$@" 2>&1 | tee -a "$LOG"
}

run npm ci
run npm run build
run npm test
run python3 -m compileall -q scripts
run python3 validation/gloss_current_run_truth_gate.py --repo .
run python3 validation/gloss_stale_pass_surface_gate.py --repo .
run python3 validation/gloss_package_scope_gate.py --repo .
run python3 validation/gloss_release_candidate_gate.py --repo . --run-id CURRENT
run python3 validation/gloss_security_egress_gate.py --repo .
run python3 validation/gloss_fastembed_download_consent_gate.py --repo .
run python3 validation/gloss_secret_store_permissions_gate.py --repo .
run python3 validation/gloss_tool_invocation_receipt_gate.py --repo .
run python3 validation/gloss_path_redaction_gate.py --repo .
run python3 validation/gloss_import_capability_gate.py --repo .
run python3 validation/gloss_document_extractors_gate.py --repo .
run python3 validation/gloss_legacy_office_extractors_gate.py --repo .
run python3 validation/gloss_audio_metadata_gate.py --repo .
run python3 validation/gloss_audio_transcription_gate.py --repo .
run python3 validation/gloss_url_import_gate.py --repo .
run python3 validation/gloss_youtube_transcript_gate.py --repo .
run python3 validation/gloss_studio_artifacts_gate.py --repo .
run python3 validation/gloss_db_doctor_gate.py --repo .
run python3 validation/gloss_failed_import_quarantine_gate.py --repo .
run python3 validation/gloss_import_performance_gate.py --repo .
run python3 validation/gloss_notebook_portability_gate.py --repo .
run npm run desktop-smoke
run python3 validation/gloss_desktop_smoke_gate.py --repo .
run python3 validation/gloss_fresh_unzip_replay_gate.py --repo .
run cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
run cargo test --manifest-path src-tauri/Cargo.toml
run cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-backend
run cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
run cargo clippy --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant --all-targets -- -D warnings

echo "All validation commands completed" | tee -a "$LOG"
