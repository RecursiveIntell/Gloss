#!/usr/bin/env python3
import csv, json, pathlib, sys
root=pathlib.Path(sys.argv[1]) if len(sys.argv)>1 else pathlib.Path('.'); run='GLOSS_COMPLETION_AND_UX_RELEASE_CANDIDATE_P3_20260513'; base=root/'docs'/'codex-runs'/run
fail=[]; status_file=base/'logs'/'commands.status.tsv'
if not status_file.exists(): fail.append('missing command status tsv')
else:
    bad=[r for r in csv.DictReader(status_file.open(), delimiter='\t') if r.get('status')!='pass']
    if bad: fail.append('non-passing commands: '+', '.join(r.get('name','?') for r in bad))
for name in ['PACKAGE_WARNING_REVIEW.json','PACKAGE_SCOPE_AUDIT.json']:
    if not (base/'reports'/name).exists(): fail.append('missing '+name)
print(json.dumps({'decision':'eligible' if not fail else 'blocked','failures':fail}, indent=2)); sys.exit(0 if not fail else 1)
