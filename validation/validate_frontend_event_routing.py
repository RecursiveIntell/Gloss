#!/usr/bin/env python3
"""Static gate: App.tsx must not drop chat lifecycle events before chatStore receives them."""
from pathlib import Path
import re, sys
root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path('.')
path = root/'src/App.tsx'
text = path.read_text(encoding='utf-8')
fail = []
for event in ['onChatToken', 'onChatStatus', 'onChatError', 'onChatEvidence']:
    # Match both `const unlisten = {event}` and `unlisteners.push({event}`
    pattern_const = f'const unlisten = {event}'
    pattern_push = f'unlisteners.push({event}'
    idx_const = text.find(pattern_const)
    idx_push = text.find(pattern_push)
    if idx_const == -1 and idx_push == -1:
        fail.append(f'missing {event}')
        continue
    idx = idx_push if idx_push != -1 else idx_const
    block = text[idx: idx+900]
    filter_idx = block.find('activeNotebookId')
    store_idx = block.find('useChatStore.getState()')
    if filter_idx != -1 and (store_idx == -1 or filter_idx < store_idx):
        fail.append(f'{event} filters activeNotebookId before chatStore handling')
if fail:
    print('FAIL: frontend event routing can drop lifecycle events:')
    print('\n'.join(fail))
    sys.exit(1)
print('PASS: chat lifecycle events are not pre-filtered by activeNotebookId')
