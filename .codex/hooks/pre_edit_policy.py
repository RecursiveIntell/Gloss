#!/usr/bin/env python3
from __future__ import annotations
import json, sys


def deny(reason: str) -> int:
    print(json.dumps({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    }))
    return 0


def main() -> int:
    try:
        data = json.load(sys.stdin)
    except Exception:
        data = {}
    text = json.dumps(data, sort_keys=True)
    forbidden = ["src-tauri/vendor/", "node_modules/", "target/", "dist/", "docs/noncanonical-source-archive/"]
    hits = [p for p in forbidden if p in text]
    if hits:
        return deny("P32R3 forbids edits to generated/vendor/build/archive paths: " + ", ".join(hits))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
