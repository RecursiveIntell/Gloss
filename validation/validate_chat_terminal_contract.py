#!/usr/bin/env python3
"""Heuristic static gate: spawned chat task should not return without terminal emit/trace after stream starts."""
from pathlib import Path
import sys, re
root = Path(sys.argv[1]) if len(sys.argv)>1 else Path('.')
path = root/'src-tauri/src/commands/chat/mod.rs'
text = path.read_text(encoding='utf-8')

# More robust spawn detection: tolerate 'async move {' variations
spawn_match = re.search(r'tokio::spawn\s*\(\s*async\s+(?:move\s+)?\{', text)
if not spawn_match:
    print('FAIL: could not locate spawned chat task block')
    sys.exit(1)

start = spawn_match.start()
# Find end: either a comment or a reasonable boundary (150 chars after last significant return)
end_comment = text.find('// Release gates', start)
# Also look for the closing of the spawn block — match braces
brace_depth = 0
found_open = False
end_brace = start
for i, ch in enumerate(text[start:], start):
    if ch == '{':
        brace_depth += 1
        found_open = True
    elif ch == '}':
        brace_depth -= 1
        if found_open and brace_depth == 0:
            end_brace = i + 1
            break

# Prefer the earlier of comment or brace close
end = end_comment if end_comment != -1 and end_comment < end_brace else end_brace
if start == -1 or end == -1 or end <= start:
    print('FAIL: could not locate spawned chat task block end')
    sys.exit(1)

block = text[start:end]

terminal_markers = [
    'emit_chat_done', 'emit_chat_error', 'emit_chat_cancel', 'emit_chat_partial',
    'emit_chat_terminal', 'terminal.emit_done', 'terminal.emit_error', 'terminal.emit_cancelled',
    'attempt_trace', 'trace.assistant_persisted', 'trace.chat_ended', 'trace.receipts_persisted',
]

violations = []

# Check explicit `return;`
for m in re.finditer(r'(?m)^\s*return\s*;\s*$', block):
    before = block[max(0, m.start()-500):m.start()]
    # Skip if inside a nested closure (crude: check if last '{' before this is a closure)
    if not any(marker in before for marker in terminal_markers):
        line = text[:start+m.start()].count('\n')+1
        violations.append(f'line {line}: bare return;')

# Check `?` operator returns: any line ending with `?;` that's NOT preceded by emit/trace
for m in re.finditer(r'(?m)^\s*.+\?\s*;\s*$', block):
    before = block[max(0, m.start()-500):m.start()]
    if not any(marker in before for marker in terminal_markers):
        line = text[:start+m.start()].count('\n')+1
        violations.append(f'line {line}: ? operator propagation without terminal emit')

if violations:
    print('FAIL: raw terminal-less return detected in spawned chat task:')
    print('\n'.join(violations))
    sys.exit(1)
print('PASS: no raw terminal-less return detected in spawned chat task')
