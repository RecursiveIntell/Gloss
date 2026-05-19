#!/usr/bin/env python3
import json, os, sys, time
from pathlib import Path

def main():
    try: data=json.load(sys.stdin)
    except Exception: data={}
    cwd=Path(data.get('cwd') or os.getcwd())
    root=cwd
    for p in [cwd]+list(cwd.parents):
        if (p/'.git').exists() or (p/'src-tauri').exists(): root=p; break
    out=root/'docs'/'codex-runs'/'P32R3'/'prompt_receipts.jsonl'
    out.parent.mkdir(parents=True, exist_ok=True)
    rec={'ts':time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),'event':data.get('hook_event_name'),'session_id':data.get('session_id'),'cwd':str(cwd)}
    out.open('a', encoding='utf-8').write(json.dumps(rec, sort_keys=True)+"\n")
    return 0
if __name__ == '__main__': raise SystemExit(main())
