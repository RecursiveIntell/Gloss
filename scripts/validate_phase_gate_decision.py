#!/usr/bin/env python3
from __future__ import annotations
import json, sys
from pathlib import Path
try:
    import jsonschema
except Exception:
    print('jsonschema unavailable; install it or validate with another Draft 2020-12 validator', file=sys.stderr)
    sys.exit(2)
if len(sys.argv) != 3:
    print('usage: validate_phase_gate_decision.py <schema.json> <decision.json>', file=sys.stderr)
    sys.exit(2)
schema = json.loads(Path(sys.argv[1]).read_text())
decision = json.loads(Path(sys.argv[2]).read_text())
jsonschema.validate(decision, schema)
print('phase gate decision valid')
