#!/usr/bin/env python3
from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--quiet", action="store_true")
    args = parser.parse_args()
    cmd = [sys.executable, str(ROOT / "scripts/validate_codex_pack.py")]
    if args.quiet:
        cmd.append("--quiet")
    result = subprocess.run(cmd, cwd=ROOT)
    if result.returncode == 0 and not args.quiet:
        print("OK: active Codex pack present and current-run surfaces are valid")
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
