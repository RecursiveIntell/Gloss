#!/usr/bin/env python3
import sys, pathlib, json
root=pathlib.Path(sys.argv[1]) if len(sys.argv)>1 else pathlib.Path('.'); run='GLOSS_COMPLETION_AND_UX_RELEASE_CANDIDATE_P3_20260513'; base=root/'docs'/'codex-runs'/run
required=['RUN_PLAN.md','MASTER_ISSUE_MATRIX.md','MASTER_ISSUE_MATRIX.csv','LOG_EVIDENCE_INDEX.md','LOG_EVIDENCE_INDEX.json','FINAL_AUDITOR_HANDOFF.md','RELEASE_ELIGIBILITY.md']
missing=[p for p in required if not (base/p).exists()]
print(json.dumps({'base':str(base),'missing':missing,'decision':'pass' if not missing else 'blocked'},indent=2)); sys.exit(0 if not missing else 1)
