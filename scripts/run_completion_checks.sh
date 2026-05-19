#!/usr/bin/env bash
set -euo pipefail

python -m pytest -q
bash scripts/run_all_checks.sh
python scripts/validate_codex_pack.py
python scripts/assert_codex_active_pack.py
mkdir -p .codex/runs/P0-completion

python .codex/tools/auto_phase_runner.py --dry-run --print-prompts --receipt .codex/runs/P0-completion/auto_phase_dry_run.json >/tmp/scr-runtime_auto_phase_prompts.txt

for phase in phase_00 phase_01 phase_02 phase_03 phase_04 phase_05 phase_06; do
  python .codex/tools/auto_phase_runner.py \
    --dry-run \
    --phase "$phase" \
    --receipt ".codex/runs/P0-completion/${phase}_dry_run_receipt.json" \
    >/tmp/scr-runtime_${phase}_phase_receipt.txt
done

test -s .codex/runs/P0-completion/auto_phase_dry_run.json
test -s .codex/runs/P0-completion/phase_00_dry_run_receipt.json
test -s .codex/runs/P0-completion/phase_01_dry_run_receipt.json
test -s .codex/runs/P0-completion/phase_02_dry_run_receipt.json
test -s .codex/runs/P0-completion/phase_03_dry_run_receipt.json
test -s .codex/runs/P0-completion/phase_04_dry_run_receipt.json
test -s .codex/runs/P0-completion/phase_05_dry_run_receipt.json
test -s .codex/runs/P0-completion/phase_06_dry_run_receipt.json

bash scripts/run_fresh_unzip_checks.sh

echo "OK: SCR P0 completion checks passed"
