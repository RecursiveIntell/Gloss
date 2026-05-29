#!/usr/bin/env python3
import subprocess
import sys

if __name__ == "__main__":
    raise SystemExit(subprocess.call([sys.executable, "scripts/gloss_current_run_truth_gate.py", *sys.argv[1:]]))
