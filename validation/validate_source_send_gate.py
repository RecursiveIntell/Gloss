#!/usr/bin/env python3
"""Static gate: sourceListStatus must not hard-disable chat send."""
from pathlib import Path
import re, sys
root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path('.')
path = root/'src/components/chat/ChatPanel.tsx'
text = path.read_text(encoding='utf-8')
violations = []
for needle in [
    'sourceListStatus === "loading"',
    'sourceListStatus === "partial"',
    'sourceListStatus === "error"',
]:
    # allowed in warning/rendering, not in handleSend early return or disabled expressions
    for m in re.finditer(re.escape(needle), text):
        window = text[max(0,m.start()-160):m.end()+160]
        if 'disabled=' in window or 'if (!input.trim()' in window or 'sourceListBlocked' in window:
            violations.append((needle, m.start()))
if violations:
    print('FAIL: sourceListStatus still appears in send/disabled blocking logic:')
    for v in violations:
        print(v)
    sys.exit(1)
print('PASS: sourceListStatus is not used as hard send/disabled gate')
