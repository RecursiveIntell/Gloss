#!/usr/bin/env bash
set -euo pipefail

# Gloss-specific network call audit:
# - LLM/network calls are BY DESIGN in provider modules (providers/, ingestion/, retrieval/)
# - This gate flags network/LLM calls in UNEXPECTED directories (commands/, stores/, lib/)
# - Only reqwest in providers/, retrieval/, ingestion/ is expected

ROOT="${1:-.}"
cd "$ROOT"

# Patterns that indicate network/LLM call surfaces
pattern='reqwest::|hyper::|tokio::net|std::net::TcpStream|std::net::UdpSocket|ureq::'

# Expected locations for network calls
expected_paths=(
  "src-tauri/src/providers"
  "src-tauri/src/retrieval"
  "src-tauri/src/ingestion"
  "src-tauri/src/jobs"
)

# Unexpected locations where network calls should NOT appear
unexpected_paths=()
for path in src-tauri/src/commands src-tauri/src/memory src-tauri/src/db src-tauri/src/studio; do
  if [[ -e "$path" ]]; then
    unexpected_paths+=("$path")
  fi
done

violations=()
if [[ "${#unexpected_paths[@]}" -gt 0 ]]; then
  while IFS= read -r match; do
    # Only flag if the match is NOT in an expected path
    is_expected=0
    for ep in "${expected_paths[@]}"; do
      if [[ "$match" == "$ep"* ]]; then
        is_expected=1
        break
      fi
    done
    if [[ "$is_expected" -eq 0 ]]; then
      violations+=("$match")
    fi
  done < <(grep -RlE "$pattern" "${unexpected_paths[@]}" --exclude-dir=target 2>/dev/null || true)
fi

if [[ "${#violations[@]}" -gt 0 ]]; then
  echo "FAIL: network/LLM call surface found in unexpected directories:"
  printf '  %s\n' "${violations[@]}"
  exit 1
fi

echo "PASS: no LLM/network call surface in unexpected production directories"
