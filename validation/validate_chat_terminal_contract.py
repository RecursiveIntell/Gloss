#!/usr/bin/env python3
"""Heuristic static gate: spawned chat task should not return without terminal emit/trace after stream starts."""
from pathlib import Path
import sys, re
root = Path(sys.argv[1]) if len(sys.argv)>1 else Path('.')
path = root/'src-tauri/src/commands/chat/mod.rs'
text = path.read_text(encoding='utf-8')

def balanced_block_end(source: str, open_at: int) -> int:
    depth = 0
    for index in range(open_at, len(source)):
        if source[index] == '{':
            depth += 1
        elif source[index] == '}':
            depth -= 1
            if depth == 0:
                return index + 1
    return -1


# The current owner is `SpawnedChatAttempt::run`; older snapshots owned the
# body directly inside `tokio::spawn(async move { ... })`. Accept both shapes
# so the gate follows the lifecycle owner instead of a historical spelling.
owner_match = re.search(
    r'(?m)^\s*(?:pub\(crate\)\s+)?async\s+fn\s+run\s*\(\s*self\s*\)\s*\{',
    text,
)
spawn_match = re.search(r'tokio::spawn\s*\(\s*async\s+(?:move\s+)?\{', text)
match = owner_match or spawn_match
if not match:
    print('FAIL: could not locate spawned chat lifecycle owner')
    sys.exit(1)

start = match.start()
open_at = text.find('{', match.start())
end = balanced_block_end(text, open_at)
if open_at == -1 or end == -1 or end <= start:
    print('FAIL: could not locate spawned chat lifecycle block end')
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
