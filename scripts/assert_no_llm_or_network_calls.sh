#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-.}"
cd "$ROOT"

pattern='reqwest|ureq|hyper::|tokio::net|std::net|TcpStream|UdpSocket|OpenAI|embedding|chat_completion|model_call'

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
  echo "LLM/model/network call surface found in production code" >&2
  exit 1
fi

echo "no LLM/model/network call surface found"
