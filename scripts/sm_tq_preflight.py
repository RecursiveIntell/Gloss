#!/usr/bin/env python3
from __future__ import annotations
import argparse, json, os, subprocess, sys
from pathlib import Path

def exists(root: Path, rel: str) -> bool:
    return (root / rel).exists()

def read(path: Path) -> str:
    try: return path.read_text(encoding='utf-8')
    except Exception: return ''

def git_status(root: Path) -> str:
    try:
        return subprocess.check_output(['git','status','--short'], cwd=root, text=True, stderr=subprocess.STDOUT)
    except Exception as e:
        return f'GIT_STATUS_ERROR: {e}'

def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument('--repo', default='.')
    args = ap.parse_args()
    repo = Path(args.repo).resolve()
    checks = {}
    required = [
        'src-tauri/Cargo.toml',
        'src-tauri/src/memory/semantic_memory_adapter.rs',
        'src-tauri/src/memory/backend.rs',
        'src-tauri/src/state.rs',
        'src-tauri/vendor/semantic-memory/src/config.rs',
        'src-tauri/vendor/semantic-memory/src/search.rs',
        'src-tauri/vendor/turbo-quant/src/lib.rs',
    ]
    missing = [p for p in required if not exists(repo, p)]
    checks['required_paths_missing'] = missing
    adapter = read(repo/'src-tauri/src/memory/semantic_memory_adapter.rs')
    checks['current_adapter_has_degraded_backpointer_marker'] = 'degraded-missing-exact-backpointer' in adapter
    checks['current_adapter_forces_sm_chunk_none'] = 'sm_chunk_id: Option<&str> = None' in adapter or 'let sm_chunk_id' in adapter and 'None' in adapter
    checks['git_status_short'] = git_status(repo)
    receipt = repo/'docs/codex-runs/P32R3/FINAL_RECEIPT.json'
    if receipt.exists():
        try:
            j = json.loads(receipt.read_text())
            checks['p32r3_release_ready'] = j.get('release_ready')
            checks['p32r3_status'] = j.get('status')
        except Exception as e:
            checks['p32r3_receipt_error'] = str(e)
    ok = not missing
    print(json.dumps({'ok': ok, 'checks': checks}, indent=2, sort_keys=True))
    return 0 if ok else 2

if __name__ == '__main__':
    raise SystemExit(main())
