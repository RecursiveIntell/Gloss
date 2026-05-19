#!/usr/bin/env python3
import json, pathlib, sys
base=pathlib.Path(sys.argv[1]) if len(sys.argv)>1 else pathlib.Path('docs/codex-runs/GLOSS_COMPLETION_AND_UX_RELEASE_CANDIDATE_P3_20260513/receipts'); errors=[]
for p in sorted(base.glob('PHASE_*.json')):
    try: d=json.loads(p.read_text())
    except Exception as e: errors.append((str(p), f'invalid json: {e}')); continue
    for k in ['run_id','phase','files_changed','commands_run','decision']:
        if k not in d: errors.append((str(p), f'missing {k}'))
print(json.dumps({'checked':len(list(base.glob('PHASE_*.json'))),'errors':errors,'decision':'pass' if not errors else 'blocked'}, indent=2)); sys.exit(0 if not errors else 1)
