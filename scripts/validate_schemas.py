#!/usr/bin/env python3
from pathlib import Path
import filecmp
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
EXPECTED = ROOT / "schemas" / "generated"

if not EXPECTED.is_dir():
    raise SystemExit("schema validation failed: schemas/generated missing")

with tempfile.TemporaryDirectory() as tmp:
    out = Path(tmp) / "generated"
    subprocess.run(
        ["cargo", "run", "-p", "scr-cli", "--", "generate-schemas", str(out)],
        cwd=ROOT,
        check=True,
        stdout=subprocess.DEVNULL,
    )
    expected_files = sorted(p.relative_to(EXPECTED) for p in EXPECTED.glob("*.json"))
    actual_files = sorted(p.relative_to(out) for p in out.glob("*.json"))
    if expected_files != actual_files:
        print("schema validation failed: generated file set differs", file=sys.stderr)
        print(f"expected={expected_files}", file=sys.stderr)
        print(f"actual={actual_files}", file=sys.stderr)
        raise SystemExit(1)
    for rel in expected_files:
        if not filecmp.cmp(EXPECTED / rel, out / rel, shallow=False):
            print(f"schema validation failed: drift in {rel}", file=sys.stderr)
            raise SystemExit(1)

print("schema validation passed")
