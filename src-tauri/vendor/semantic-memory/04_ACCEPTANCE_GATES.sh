#!/usr/bin/env bash
set -euo pipefail

# Codex Giga-Pass acceptance gates.
# Run from the workspace root containing semantic-memory, stack-ids,
# semantic-memory-forge, and forge-memory-bridge when packaged together.

log() {
  printf '\n\033[1;34m==> %s\033[0m\n' "$*"
}

run() {
  printf '\n$ %s\n' "$*"
  "$@"
}

log "Toolchain"
run cargo --version
run rustc --version

log "Formatting"
run cargo fmt --all --check

log "Check workspace"
run cargo check --workspace --all-targets --all-features

log "Tests"
run cargo test --workspace --all-features

log "Clippy"
run cargo clippy --workspace --all-targets --all-features -- -D warnings

log "Docs"
run cargo doc --workspace --all-features --no-deps

log "Static persistence safety checks"
if command -v rg >/dev/null 2>&1; then
  echo "Checking for persisted usize patterns in HNSW/sidecar code..."
  if rg "usize::(to_le_bytes|from_le_bytes)|size_of::<usize>" semantic-memory/src/hnsw.rs semantic-memory/src/hnsw_ops.rs 2>/dev/null; then
    echo "ERROR: possible persisted usize in HNSW sidecar path. Inspect before release."
    exit 1
  fi

  echo "Checking for deep wall-clock calls in search internals..."
  if rg "Utc::now\(|Local::now\(" semantic-memory/src/search.rs 2>/dev/null; then
    echo "WARNING: wall-clock call found in search.rs. Ensure it is only at API boundary."
  fi

  echo "Checking semantic-boundary unwrap_or_default hotspots..."
  rg "unwrap_or_default\(" semantic-memory/src semantic-memory-forge/src forge-memory-bridge/src || true
else
  echo "ripgrep not installed; skipping static rg checks."
fi

log "Done"
