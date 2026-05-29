#!/usr/bin/env bash
set -euo pipefail

bash scripts/run_all_checks.sh
python3 validation/gloss_release_candidate_gate.py --repo . --run-id CURRENT
python3 scripts/validate_codex_pack.py
python3 scripts/assert_codex_active_pack.py

echo "OK: Gloss completion checks passed"
