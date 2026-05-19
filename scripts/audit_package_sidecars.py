#!/usr/bin/env python3
import json, sys, pathlib, hashlib
if len(sys.argv) != 5:
    print("usage: audit_package_sidecars.py PACKAGE_ZIP MANIFEST_JSON FINDINGS_JSON EXCLUDED_JSON", file=sys.stderr); sys.exit(2)
package, manifest, findings, excluded = map(pathlib.Path, sys.argv[1:])
errors=[]
if not package.exists(): errors.append("package missing")
else:
    sha = hashlib.sha256(package.read_bytes()).hexdigest()
try:
    man = json.loads(manifest.read_text())
    expected = man.get("archive_zip_byte_sha256") or man.get("report",{}).get("archive_zip_byte_sha256") or man.get("archive_sha256")
    if expected and package.exists() and expected != sha: errors.append(f"package sha mismatch: manifest {expected}, actual {sha}")
    if man.get("report",{}).get("error_count", man.get("error_count",0)) != 0: errors.append("manifest reports errors")
except Exception as e: errors.append(f"manifest parse failed: {e}")
try:
    f = json.loads(findings.read_text())
    if f.get("error_count",0) != 0: errors.append("findings reports errors")
except Exception as e: errors.append(f"findings parse failed: {e}")
try:
    json.loads(excluded.read_text())
except Exception as e: errors.append(f"excluded parse failed: {e}")
print(json.dumps({"status":"fail" if errors else "pass", "errors":errors}, indent=2))
sys.exit(1 if errors else 0)
