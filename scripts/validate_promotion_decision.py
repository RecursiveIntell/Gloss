#!/usr/bin/env python3
import json, sys, pathlib, re
allowed = {"not_promoted","promotable_preview","safe_to_default_candidate","blocked"}
if len(sys.argv) != 2:
    print("usage: validate_promotion_decision.py PATH_TO_DECISION_JSON_OR_MD", file=sys.stderr); sys.exit(2)
p = pathlib.Path(sys.argv[1])
text = p.read_text()
if p.suffix == ".json":
    obj = json.loads(text); decision = obj.get("decision")
    default = obj.get("default_backend_after_pass")
else:
    m = re.search(r"Promotion decision:\s*`?([a-z_]+)`?", text)
    decision = m.group(1) if m else None
    default = "gloss-local" if "gloss-local" in text else None
errors=[]
if decision not in allowed: errors.append(f"invalid or missing decision: {decision}")
if default != "gloss-local": errors.append("default backend after pass must be gloss-local")
if decision == "safe_to_default_candidate" and "default remains" not in text.lower() and p.suffix != ".json":
    errors.append("safe_to_default_candidate must still state default remains gloss-local")
if errors:
    print(json.dumps({"status":"fail","errors":errors}, indent=2)); sys.exit(1)
print(json.dumps({"status":"pass","decision":decision}, indent=2))
