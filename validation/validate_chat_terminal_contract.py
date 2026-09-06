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

# An invalid rerun target must be rejected before a durable queued attempt is
# created. This is an ordering guard; Rust history tests prove membership/role
# rejection, and native workflow tests prove actual accepted rerun behavior.
send_at = text.find('pub async fn send_message(')
send_open = text.find('{', send_at)
send_end = balanced_block_end(text, send_open) if send_open >= 0 else -1
send = text[send_open:send_end] if send_at >= 0 and send_end >= 0 else ''
validation_at = send.find('let rerun_history =')
acceptance_sites = [send.find(marker) for marker in (
    'let attempt_trace =', 'let active_chat_attempt_lease =',
    'persist_chat_attempt_status(', 'state.bump_chat_grace()',
)]
if (validation_at < 0 or any(site < 0 for site in acceptance_sites)
        or validation_at >= min(acceptance_sites)
        or 'history_before_rerun(' not in send[validation_at:min(acceptance_sites)]
        or send.count('history_before_rerun(') != 1
        or 'let history = match rerun_history' not in send):
    violations.append('rerun target rejection must precede attempt acceptance and preemption')

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
