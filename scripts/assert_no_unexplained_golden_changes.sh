#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-.}"
cd "$ROOT"

if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  changed="$(git diff --name-only -- fixtures/audit/expected || true)"
  if [[ -n "$changed" && ! -f POLICY_CHANGE.md ]]; then
    echo "golden fixture changes require POLICY_CHANGE.md" >&2
    echo "$changed" >&2
    exit 1
  fi
fi

echo "golden fixture change policy satisfied"
