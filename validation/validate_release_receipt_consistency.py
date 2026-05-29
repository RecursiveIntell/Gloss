#!/usr/bin/env python3
"""Gate: FINAL_RECEIPT must not contradict release gate result if both exist."""
from pathlib import Path
import json, sys
root = Path(sys.argv[1]) if len(sys.argv)>1 else Path('.')
finals = list(root.glob('**/FINAL_RECEIPT.json'))
gates = list(root.glob('**/RELEASE_CANDIDATE_GATE_RESULTS.json'))
if not finals or not gates:
    print('WARN: FINAL_RECEIPT.json or RELEASE_CANDIDATE_GATE_RESULTS.json not found; release remains blocked until produced')
    sys.exit(0)
fail=[]
for f in finals:
    try: fj=json.loads(f.read_text())
    except Exception as e: fail.append(f'{f}: invalid json {e}'); continue
    for g in gates:
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
