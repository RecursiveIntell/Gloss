#!/usr/bin/env python3
from __future__ import annotations
import json, os, sys, time
from pathlib import Path


def find_root(cwd: Path) -> Path:
    for p in [cwd, *cwd.parents]:
        if (p / '.git').exists() or (p / 'src-tauri').exists():
            return p
    return cwd


def main() -> int:
    try:
        data = json.load(sys.stdin)
    except Exception as e:
        data = {'parse_error': str(e)}
    cwd = Path(data.get('cwd') or os.getcwd())
    root = find_root(cwd)
    out = root / 'docs' / 'codex-runs' / 'P32R3' / 'permission_requests.jsonl'
    out.parent.mkdir(parents=True, exist_ok=True)
    rec = {
        'ts': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
        'hook_event_name': data.get('hook_event_name'),
        'tool_name': data.get('tool_name'),
        'turn_id': data.get('turn_id'),
        'cwd': str(cwd),
        'permission_mode': data.get('permission_mode'),
        'tool_input': data.get('tool_input'),
    }
    with out.open('a', encoding='utf-8') as f:
        f.write(json.dumps(rec, sort_keys=True) + '\n')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
