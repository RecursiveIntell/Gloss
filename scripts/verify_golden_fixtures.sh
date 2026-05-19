#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-.}"
cd "$ROOT"

cargo run -p scr-cli -- verify-fixtures fixtures/audit/cases fixtures/audit/expected policies/audit_policy_v1.toml >/dev/null

echo "golden fixtures verified"
