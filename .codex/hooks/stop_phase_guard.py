#!/usr/bin/env python3
from __future__ import annotations
import json, os, sys
from pathlib import Path


def main() -> int:
    try:
        data = json.load(sys.stdin)
    except Exception:
        data = {}
    cwd = Path(data.get("cwd") or os.getcwd())
    root = cwd
    for p in [cwd, *cwd.parents]:
        if (p / ".git").exists() or (p / "src-tauri").exists():
            root = p
            break
    work = root / "docs" / "codex-runs" / "P32R3"
    final = work / "FINAL_RECEIPT.json"
    finalize_marker = work / "FINALIZATION_REQUESTED"
    if finalize_marker.exists() and not final.exists():
        print(json.dumps({
            "continue": False,
            "stopReason": "P32R3 final receipt missing",
            "systemMessage": "P32R3 stop guard: FINALIZATION_REQUESTED exists but FINAL_RECEIPT.json is missing. Write the final receipt or remove the marker and report blockers."
        }))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
