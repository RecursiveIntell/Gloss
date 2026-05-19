#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-$(pwd)}"
cd "$ROOT"

echo "== semantic-memory acceptance gates =="
echo "root: $PWD"

need() {
  command -v "$1" >/dev/null 2>&1 || { echo "missing required command: $1" >&2; exit 127; }
}

need cargo
need rustc

if [[ ! -f Cargo.toml ]]; then
  echo "No Cargo.toml at root. Run from the packaged workspace root or full workspace root." >&2
  exit 2
fi

run() {
  echo
  echo "+ $*"
  "$@"
}

run cargo fmt --all --check
run cargo check --workspace --all-features
run cargo test --workspace --all-features
run cargo clippy --workspace --all-features -- -D warnings
run cargo doc --workspace --all-features --no-deps

# Feature matrix for the main package. These commands assume semantic-memory is a workspace member.
run cargo test -p semantic-memory --no-default-features --features brute-force
run cargo test -p semantic-memory --no-default-features --features hnsw
run cargo test -p semantic-memory --all-features

# Optional specific tests if present. Do not fail if a target does not exist; cargo test above is authoritative.
for test_name in \
  hardening_semantics \
  hnsw_persistence \
  hnsw_integration \
  conversation_search_tests \
  quantization \
  quantization_pipeline \
  storage_lifecycle \
  search_tests \
  import_boundary_tests \
  import_ugly_cases; do
  if [[ -f "semantic-memory/tests/${test_name}.rs" ]]; then
    run cargo test -p semantic-memory --test "$test_name" --all-features
  fi
done

# Optional archive regeneration and clean extraction test.
if [[ -f semantic-memory/z.py ]]; then
  echo
  echo "== archive regeneration =="
  run python3 semantic-memory/z.py --root semantic-memory --profile generic-rust --mode next-codex-context --strict
fi

latest_zip=""
if compgen -G "semantic-memory/semantic-memory-*.zip" > /dev/null; then
  latest_zip="$(ls -t semantic-memory/semantic-memory-*.zip | head -n 1)"
elif compgen -G "semantic-memory-*.zip" > /dev/null; then
  latest_zip="$(ls -t semantic-memory-*.zip | head -n 1)"
fi

if [[ -n "$latest_zip" ]]; then
  echo
  echo "== clean extraction test: $latest_zip =="
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  unzip -q "$latest_zip" -d "$tmp/extract"
  if [[ -f "$tmp/extract/Cargo.toml" ]]; then
    (cd "$tmp/extract" && cargo check --workspace --all-features)
    (cd "$tmp/extract" && cargo test --workspace --all-features)
    (cd "$tmp/extract" && cargo clippy --workspace --all-features -- -D warnings)
    (cd "$tmp/extract" && cargo doc --workspace --all-features --no-deps)
  else
    echo "clean extraction lacks root Cargo.toml; hermetic packaging gate failed" >&2
    exit 3
  fi
fi

echo
echo "all semantic-memory acceptance gates passed"
