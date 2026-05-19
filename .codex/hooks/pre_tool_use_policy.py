#!/usr/bin/env python3
from __future__ import annotations
import json, re, sys


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
    tool_input = data.get("tool_input") or {}
    cmd = tool_input.get("command") if isinstance(tool_input, dict) else None
    haystack = cmd if isinstance(cmd, str) else json.dumps(tool_input, sort_keys=True)
    dangerous = [
        (r"\bgit\s+reset\s+--hard\b", "git reset --hard is forbidden in P32R3"),
        (r"\bgit\s+clean\s+-xfd\b", "git clean -xfd is forbidden in P32R3"),
        (r"\brm\s+-rf\s+(/|\$HOME|~|\.\.)", "broad rm -rf is forbidden in P32R3"),
        (r"\bnpm\s+publish\b", "npm publish is outside this repair pass"),
        (r"\bcargo\s+publish\b", "cargo publish is outside this repair pass"),
        (r"\bgh\s+secret\b", "GitHub secret inspection/mutation is outside this repair pass"),
    ]
    for pattern, reason in dangerous:
        if re.search(pattern, haystack):
            return deny(reason)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
