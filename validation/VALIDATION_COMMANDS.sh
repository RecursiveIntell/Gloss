#!/usr/bin/env bash
set -euo pipefail

RUN_ID="GLOSS_P36_RELEASE_COMPLETION_DENSE_TQ_RELEASE_20260525"
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
run npm run check:sm-tq-profile
run python3 -m compileall -q scripts

if [ -f validation/gloss_p36_static_gate.py ]; then
  run python3 validation/gloss_p36_static_gate.py --repo .
elif [ -f scripts/gloss_p36_static_gate.py ]; then
  run python3 scripts/gloss_p36_static_gate.py --repo .
else
  echo "Missing gloss_p36_static_gate.py" | tee -a "$LOG"
  exit 1
fi

if [ -f validation/gloss_dense_tq_release_gate.py ]; then
  run python3 validation/gloss_dense_tq_release_gate.py --repo .
elif [ -f scripts/gloss_dense_tq_release_gate.py ]; then
  run python3 scripts/gloss_dense_tq_release_gate.py --repo .
else
  echo "Missing gloss_dense_tq_release_gate.py" | tee -a "$LOG"
  exit 1
fi

run python3 scripts/gloss_current_run_truth_gate.py --repo .
run python3 scripts/gloss_validator_path_gate.py --repo .
run python3 scripts/gloss_receipt_integrity_gate.py --repo .
run python3 scripts/gloss_feature_matrix_gate.py --repo .
run python3 scripts/gloss_issue_ledger_gate.py --ledger ISSUE_LEDGER.csv
run python3 scripts/gloss_retrieval_gate.py --repo .
run python3 scripts/gloss_evidence_ui_gate.py --repo .
run cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
run cargo test --manifest-path src-tauri/Cargo.toml
run cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-backend
run cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
run cargo clippy --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant --all-targets -- -D warnings
run npm run tauri:build:sm-tq
run python3 scripts/gloss_desktop_smoke_harness.py --repo . --require-live-receipt

echo "All validation commands completed" | tee -a "$LOG"
