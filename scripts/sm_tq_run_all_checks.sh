#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
mkdir -p docs/codex-runs/SM_TQ_20260516/logs
LOGDIR="docs/codex-runs/SM_TQ_20260516/logs"

run() {
  local name="$1"; shift
  echo "==> $name"
  ( "$@" ) >"$LOGDIR/${name}.log" 2>&1
  echo "PASS $name"
}

run preflight python3 scripts/sm_tq_preflight.py --repo .
run static_validator python3 scripts/sm_tq_static_validator.py --repo .
run semantic_memory_fmt bash -lc 'cd src-tauri/vendor/semantic-memory && cargo fmt --check'
run semantic_memory_test_tq bash -lc 'cd src-tauri/vendor/semantic-memory && cargo test --features turbo-quant-codec'
run semantic_memory_clippy_tq bash -lc 'cd src-tauri/vendor/semantic-memory && cargo clippy --all-targets --features turbo-quant-codec -- -D warnings'
run gloss_tauri_fmt bash -lc 'cd src-tauri && cargo fmt --check'
run gloss_semantic_memory_tests bash -lc 'cd src-tauri && cargo test --features semantic-memory-backend'
run gloss_semantic_memory_tq_tests bash -lc 'cd src-tauri && cargo test --features semantic-memory-turbo-quant'
run gloss_semantic_memory_tq_clippy bash -lc 'cd src-tauri && cargo clippy --all-targets --features semantic-memory-turbo-quant -- -D warnings'
run npm_build npm run build

echo "All scripted checks passed. Logs: $LOGDIR"
