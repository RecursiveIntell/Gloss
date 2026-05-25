#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-.}"
cd "$ROOT"

pattern='static[[:space:]]+mut|lazy_static!|OnceCell<.*(Mutex|RwLock)|OnceLock<.*(Mutex|RwLock)|GLOBAL_|SINGLETON|score_cache|policy_cache'

scan_paths=()
for path in src-tauri/src src; do
  if [[ -e "$path" ]]; then
    scan_paths+=("$path")
  fi
done

if [[ "${#scan_paths[@]}" -eq 0 ]]; then
  echo "no production paths found" >&2
  exit 1
fi

if grep -RInE "$pattern" "${scan_paths[@]}" --exclude-dir=target; then
  echo "potential shadow-truth mutable global/cache pattern found" >&2
  exit 1
fi

echo "no shadow-truth mutable global/cache pattern found"
