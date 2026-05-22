#!/usr/bin/env bash
set -euo pipefail

if ! command -v rg >/dev/null 2>&1; then
  echo "missing required tool: rg" >&2
  exit 2
fi

violations=0
# Chat stream routing must not fall back to active-notebook-only identity.
patterns=(
  'activeNotebookId.*chat-token'
  'activeNotebookId.*chat-status'
  'activeNotebookId.*chat-error'
)
for pattern in "${patterns[@]}"; do
  if rg -n "$pattern" src >/tmp/gloss_opaque_stream_scan.txt 2>/dev/null; then
    cat /tmp/gloss_opaque_stream_scan.txt >&2
    violations=1
  fi
done

if [[ "$violations" -ne 0 ]]; then
  echo "active-notebook-only chat event routing is forbidden" >&2
  exit 1
fi

echo "ok chat events are not routed by active notebook only"
