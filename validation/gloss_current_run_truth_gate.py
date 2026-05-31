#!/usr/bin/env python3
"""Gloss current run truth gate — validates CURRENT_RUN.md consistency."""
import argparse
import json
import re
import sys
from pathlib import Path

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default=".")
    args = parser.parse_args()
    repo = Path(args.repo).resolve()

    run_path = repo / "docs/codex-runs/CURRENT_RUN.md"
    if not run_path.exists():
        print(json.dumps({"ok": False, "failures": ["CURRENT_RUN.md missing"]}, indent=2))
        return 1

    text = run_path.read_text(errors="ignore")
    match = re.search(r"Current run:\s*`?([^`\n]+)`?", text)
    run_id = match.group(1).strip() if match else None

    if not run_id:
        print(json.dumps({"ok": False, "failures": ["No Current run ID found in CURRENT_RUN.md"]}, indent=2))
        return 1

    run_dir = repo / "docs/codex-runs" / run_id
    if not run_dir.exists():
        print(json.dumps({"ok": False, "failures": [f"Run dir {run_id} not found"]}, indent=2))
        return 1

    print(json.dumps({"ok": True, "current_run": run_id, "failures": []}, indent=2))
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
