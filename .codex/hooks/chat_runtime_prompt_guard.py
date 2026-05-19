#!/usr/bin/env python3
"""Conservative prompt guard for the chat runtime fix pass.
Reads JSON from stdin when Codex provides it; otherwise exits 0.
"""
import json, sys, re
try:
    data = json.load(sys.stdin)
except Exception:
    sys.exit(0)
text = json.dumps(data).lower()
forbidden = ["release_ready=true", "disable turboquant", "remove exact rerank", "semantic-memory default-ready"]
violations = [v for v in forbidden if v in text]
if violations:
    print("CHAT_RUNTIME_GUARD_BLOCKED: " + ", ".join(violations), file=sys.stderr)
    sys.exit(2)
sys.exit(0)
