#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-.}"
cd "$ROOT"

pattern='static[[:space:]]+mut|lazy_static!|OnceCell<.*(Mutex|RwLock)|OnceLock<.*(Mutex|RwLock)|GLOBAL_|SINGLETON|score_cache|policy_cache'

if grep -RInE "$pattern" crates --exclude-dir=target; then
  echo "potential shadow-truth mutable global/cache pattern found" >&2
  exit 1
fi

echo "no shadow-truth mutable global/cache pattern found"
