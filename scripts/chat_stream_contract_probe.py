#!/usr/bin/env python3
"""
Static probe for the Gloss chat stream terminal contract.

This is intentionally conservative. It is not a substitute for runtime tests.
It should fail before the done-frame fix and pass only after the source has an
explicit terminalization path for provider done frames.
"""
import re
import sys
from pathlib import Path

args = sys.argv[1:]
if args[:1] == ['--repo'] and len(args) >= 2:
    ROOT = Path(args[1])
elif args:
    ROOT = Path(args[0])
else:
    ROOT = Path('.')
chat = ROOT / 'src-tauri' / 'src' / 'commands' / 'chat' / 'mod.rs'
if not chat.exists():
    print(f'FAIL missing {chat}')
    sys.exit(2)
text = chat.read_text(errors='replace')
failures = []

if '"done": false' in text:
    failures.append('chat token event still appears to hardcode done=false')

if re.search(r'if\s+done\s*\{\s*sent_done\s*=\s*true\s*;\s*\}', text):
    failures.append('done branch sets sent_done without visible terminalization/break')

if 'terminal_cause' not in text or 'provider_done_frame' not in text:
    failures.append('missing explicit provider_done_frame terminal cause')

if 'done_frame_seen' not in text:
    failures.append('missing done_frame_seen receipt/state field')

if 'eof_seen' not in text:
    failures.append('missing eof_seen receipt/state field')

if failures:
    print('FAIL chat stream contract probe')
    for f in failures:
        print(f'- {f}')
    sys.exit(1)
print('PASS chat stream contract probe')
