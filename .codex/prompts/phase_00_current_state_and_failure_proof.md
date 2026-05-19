# Phase 00 — Current-State Failure Proof

Run:

```bash
git status --short || true
find .codex -maxdepth 3 -type f | sort | sed -n '1,240p'
python -m pytest -q || true
bash scripts/run_all_checks.sh || true
python scripts/validate_codex_pack.py || true
```
