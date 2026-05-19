#!/usr/bin/env python3
import json, sys
from pathlib import Path
try:
    import jsonschema
except Exception:
    print('jsonschema unavailable; install or use another validator', file=sys.stderr)
    sys.exit(2)
schema = json.loads(Path(sys.argv[1]).read_text())
report = json.loads(Path(sys.argv[2]).read_text())
jsonschema.validate(report, schema)
print('valid')
