#!/usr/bin/env python3
"""Compatibility entrypoint for the current-run Gloss release gate."""
from __future__ import annotations

import subprocess
import sys


def main() -> int:
    args = [sys.executable, "scripts/check_release_eligibility_current.py", *sys.argv[1:]]
    return subprocess.call(args)


if __name__ == "__main__":
    raise SystemExit(main())
