#!/usr/bin/env bash
set -euo pipefail
OLLAMA_URL="${OLLAMA_URL:-http://127.0.0.1:11434}"
echo "Checking Ollama tags at $OLLAMA_URL"
if command -v curl >/dev/null 2>&1; then
  curl -fsS "$OLLAMA_URL/api/tags" | head -c 2000 || {
    echo "Ollama tags check failed" >&2
    exit 1
  }
else
  echo "curl missing" >&2
  exit 2
fi
if command -v ollama >/dev/null 2>&1; then
  timeout 60s ollama run "${OLLAMA_MODEL:-cogito:3b}" "Reply with only: ok" || true
else
  echo "ollama CLI missing; HTTP check only"
fi
