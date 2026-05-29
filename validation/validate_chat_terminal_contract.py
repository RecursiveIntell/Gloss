#!/usr/bin/env python3
"""Heuristic static gate: spawned chat task should not return without terminal emit/trace after stream starts."""
from pathlib import Path
import sys, re
root = Path(sys.argv[1]) if len(sys.argv)>1 else Path('.')
path = root/'src-tauri/src/commands/chat/mod.rs'
text = path.read_text(encoding='utf-8')
start = text.find('tokio::spawn(async move')
end = text.find('// Release gates', start)
if start == -1 or end == -1:
    print('FAIL: could not locate spawned chat task block')
    sys.exit(1)
block = text[start:end]
terminal_markers = ['emit_chat_done', 'emit_chat_error', 'emit_chat_cancel', 'emit_chat_partial', 'emit_chat_terminal', 'terminal.emit_done', 'terminal.emit_error', 'terminal.emit_cancelled']
violations = []
for m in re.finditer(r'(?m)^\s*return\s*;', block):
    before = block[max(0, m.start()-500):m.start()]
    if not any(marker in before for marker in terminal_markers):
        line = text[:start+m.start()].count('\n')+1
        violations.append(line)
if violations:
    print('FAIL: raw return; without nearby terminal event in spawned chat task at lines:', violations)
    sys.exit(1)
print('PASS: no raw terminal-less return detected in spawned chat task')
