#!/usr/bin/env python3
import json, sys, pathlib
findings=pathlib.Path(sys.argv[1]); out=pathlib.Path(sys.argv[2]) if len(sys.argv)>2 else pathlib.Path('PACKAGE_WARNING_REVIEW.json')
data=json.loads(findings.read_text()); items=data.get('findings', data if isinstance(data,list) else [])
warnings=[]; blocking=0
for it in items:
    path=it.get('path',''); code=it.get('code','')
    if '/vendor/crates/' in path or path.startswith('Gloss/src-tauri/vendor/'): cls='third_party_vendor'; block=False
    elif path.startswith('Gloss/'): cls='first_party_gloss'; block=code.startswith('secret-content')
    elif path: cls='first_party_root'; block=True
    else: cls='unknown'; block=True
    blocking += 1 if block else 0
    warnings.append({'path':path,'code':code,'severity':it.get('severity','warning'),'detail':it.get('detail',''),'classification':cls,'blocking':block})
result={'warning_count':len(warnings),'blocking_count':blocking,'warnings':warnings,'decision':'pass' if blocking==0 else 'blocked'}
out.write_text(json.dumps(result,indent=2)+'\n'); out.with_suffix('.md').write_text('# Package Warning Review\n\n'+f"Warnings: {len(warnings)}\n\nBlocking: {blocking}\n\nDecision: {result['decision']}\n")
print(json.dumps(result,indent=2)); sys.exit(0 if result['decision']=='pass' else 1)
