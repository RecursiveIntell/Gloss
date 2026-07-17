#!/usr/bin/env python3
"""Static gate for receipt/attempt consistency."""
from __future__ import annotations
import sys
from pathlib import Path
ROOT = Path(sys.argv[1]) if len(sys.argv)>1 else Path.cwd()
FAIL=[]

def all_text(glob):
    out=[]
    for p in ROOT.glob(glob):
        if p.is_file():
            out.append((str(p.relative_to(ROOT)), p.read_text(errors="ignore")))
    return out

chat_files=all_text("src-tauri/src/commands/chat/*.rs")
joined="\n".join(t for _,t in chat_files)

for term in ["assistant_persisted", "done_seen", "first_token_seen"]:
    if term not in joined:
        FAIL.append(f"chat attempt trace missing expected field marker: {term}")

if "terminal" not in joined.lower():
    FAIL.append("chat runtime should have explicit terminal state handling")

if "chat_attempts" not in joined and "ChatAttemptStatus" not in joined:
    FAIL.append("no durable ChatAttemptStatus/chat_attempts evidence found")

if "late" not in joined.lower() or "cancel" not in joined.lower():
    FAIL.append("late chunk/cancel terminal behavior should be explicitly handled")

if FAIL:
    print("FAILURES:")
    for f in FAIL:
        print("  -", f)
    sys.exit(1)
print("gloss_receipt_consistency_gate: PASS")
