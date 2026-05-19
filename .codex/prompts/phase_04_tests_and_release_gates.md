# Phase 04 — Tests and Release Gates

Required commands:

```bash
python -m pytest -q
bash scripts/run_all_checks.sh
python scripts/validate_codex_pack.py
python scripts/assert_codex_active_pack.py
python .codex/tools/auto_phase_runner.py --dry-run --print-prompts --receipt .codex/runs/P0-completion/auto_phase_dry_run.json
```
