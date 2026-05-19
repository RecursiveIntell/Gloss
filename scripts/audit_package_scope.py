#!/usr/bin/env python3
import json, sys, pathlib
manifest=pathlib.Path(sys.argv[1]); out=pathlib.Path(sys.argv[2]) if len(sys.argv)>2 else pathlib.Path('PACKAGE_SCOPE_AUDIT.json')
data=json.loads(manifest.read_text()); files=data.get('files',[]); allowed_top={'Gloss','Libraries'}
violations=[f.get('path','') for f in files if f.get('path','').split('/',1)[0] not in allowed_top]
result={'file_count':len(files),'top_level_violations_count':len(violations),'top_level_violations':violations[:500],'decision':'pass' if not violations else 'blocked'}
out.write_text(json.dumps(result,indent=2)+'\n'); print(json.dumps(result,indent=2)); sys.exit(0 if result['decision']=='pass' else 1)
