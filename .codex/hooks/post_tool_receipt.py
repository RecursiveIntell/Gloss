#!/usr/bin/env python3
import json, os, sys, time
from pathlib import Path

def main():
    try: data=json.load(sys.stdin)
    except Exception as e: data={"parse_error": str(e)}
    cwd=Path(data.get('cwd') or os.getcwd())
    root=cwd
    for p in [cwd]+list(cwd.parents):
        if (p/'.git').exists() or (p/'src-tauri').exists():
            root=p; break
    out=root/'docs'/'codex-runs'/'P32R3'/'tool_receipts.jsonl'
    out.parent.mkdir(parents=True, exist_ok=True)
    rec={
        'ts': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
        'hook_event_name': data.get('hook_event_name'),
        'tool_name': data.get('tool_name') or data.get('matcher'),
        'cwd': str(cwd),
        'session_id': data.get('session_id'),
        'turn_id': data.get('turn_id'),
        'permission_mode': data.get('permission_mode'),
    }
    with out.open('a', encoding='utf-8') as f:
        f.write(json.dumps(rec, sort_keys=True)+"\n")
    return 0
if __name__ == '__main__':
    raise SystemExit(main())
