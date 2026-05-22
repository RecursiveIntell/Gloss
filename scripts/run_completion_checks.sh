#!/usr/bin/env bash
set -euo pipefail

bash scripts/run_all_checks.sh
python3 scripts/check_release_eligibility_current.py --repo .
python3 scripts/gloss_button_up_gate.py --repo .

echo "OK: Gloss completion checks passed"
