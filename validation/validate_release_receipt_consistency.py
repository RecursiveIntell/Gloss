#!/usr/bin/env python3
"""Gate: FINAL_RECEIPT must not contradict release gate result if both exist."""
from pathlib import Path
import json, sys
root = Path(sys.argv[1]) if len(sys.argv)>1 else Path('.')
finals = list(root.glob('**/FINAL_RECEIPT.json'))
gates = list(root.glob('**/RELEASE_CANDIDATE_GATE_RESULTS.json'))
if not finals and not gates:
    print('FAIL: neither FINAL_RECEIPT.json nor RELEASE_CANDIDATE_GATE_RESULTS.json found — expected release artifacts are completely absent')
    sys.exit(2)  # Exit 2 = missing required artifacts, distinct from contradiction (exit 1)

if not finals:
    print('WARN: FINAL_RECEIPT.json not found')
    sys.exit(2)

if not gates:
    print('WARN: RELEASE_CANDIDATE_GATE_RESULTS.json not found')
    sys.exit(2)

fail=[]
# Match by parent directory (run) to avoid cross-run contradictions
run_pairs = {}
for f in finals:
    run_dir = str(f.parent.resolve())
    run_pairs.setdefault(run_dir, {})['final'] = f
for g in gates:
    run_dir = str(g.parent.resolve())
    run_pairs.setdefault(run_dir, {})['gate'] = g

for run_dir, files in run_pairs.items():
    f = files.get('final')
    g = files.get('gate')
    if not f or not g:
        # Only flag unpaired receipts for the current run; historical runs are grandfathered
        current_run = None
        current_run_file = root / "docs/codex-runs/CURRENT_RUN.md"
        if current_run_file.exists():
            for line in current_run_file.read_text(errors="ignore").splitlines():
                if line.startswith("Current run:"):
                    current_run = line.split(":", 1)[1].strip().strip("`")
                    break
        rd_name = Path(run_dir).name
        if current_run and rd_name != current_run:
            continue  # Historical run — skip
        if not f:
            fail.append(f'{run_dir}: has gate receipt but no FINAL_RECEIPT.json')
        if not g:
            fail.append(f'{run_dir}: has FINAL_RECEIPT.json but no gate receipt')
        continue
    try: fj=json.loads(f.read_text())
    except Exception as e: fail.append(f'{f}: invalid json {e}'); continue
    try: gj=json.loads(g.read_text())
    except Exception as e: fail.append(f'{g}: invalid json {e}'); continue
    fk=fj.get('release_candidate_gate_passed')
    gk=gj.get('release_candidate_gate_passed')
    if fk is not None and gk is not None and fk != gk:
        fail.append(f'{f} says {fk}, {g} says {gk}')
if fail:
    print('FAIL: release receipt contradiction:')
    print('\n'.join(fail))
    sys.exit(1)
print('PASS: release receipt consistency gate')
