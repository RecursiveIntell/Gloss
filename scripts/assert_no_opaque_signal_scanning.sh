#!/usr/bin/env bash
set -euo pipefail

if ! command -v rg >/dev/null 2>&1; then
  echo "missing required tool: rg" >&2
  exit 2
fi

violations=0
# Signal extraction must not tokenize opaque refs/IDs. Explicit ref_kind == "signal" handling is allowed.
patterns=(
  'collect_text\(&mut values, &input\.input_id\)'
  'collect_text\(&mut values, input\.actor_ref\.ref_value\.as_str\(\)\)'
  'collect_text\(&mut values, input\.permit_ref\.ref_value\.as_str\(\)\)'
  'collect_text\(&mut values, input\.subject_ref\.ref_value\.as_str\(\)\)'
  'collect_text\(&mut values, input\.environment_ref\.ref_value\.as_str\(\)\)'
  'collect_text\(&mut values, &evidence_ref\.ref_value\)'
)
for pattern in "${patterns[@]}"; do
  if rg -n "$pattern" crates/scr-reference crates/scr-kernel >/tmp/scr_opaque_signal_scan.txt 2>/dev/null; then
    cat /tmp/scr_opaque_signal_scan.txt >&2
    violations=1
  fi
done

if [[ "$violations" -ne 0 ]]; then
  echo "opaque ref/id token scanning is forbidden" >&2
  exit 1
fi

echo "ok no opaque signal scanning"
