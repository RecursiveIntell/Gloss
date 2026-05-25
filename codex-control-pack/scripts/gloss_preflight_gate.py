#!/usr/bin/env python3
import argparse, json, pathlib, subprocess, sys

def main():
    ap=argparse.ArgumentParser(); ap.add_argument('--repo', default='.')
    args=ap.parse_args(); repo=pathlib.Path(args.repo).resolve()
    failures=[]; warnings=[]
    for p in ['package.json','src-tauri/Cargo.toml','src-tauri/tauri.conf.json','AGENTS.md']:
        if not (repo/p).exists(): failures.append(f'missing required file: {p}')
    if not (repo/'.git').exists(): warnings.append('no .git metadata in this source package; require SOURCE_SNAPSHOT_RECEIPT.json')
    pkg=json.loads((repo/'package.json').read_text()) if (repo/'package.json').exists() else {}
    for name, cmd in pkg.get('scripts',{}).items():
        parts=cmd.split()
        for tok in parts:
            if tok.startswith('scripts/') and not (repo/tok).exists(): failures.append(f'package.json script {name} references missing {tok}')
    if (repo/'src-tauri/vendor/crates').exists(): warnings.append('inactive-looking src-tauri/vendor/crates present; package scope must justify or exclude')
    out={'status':'fail' if failures else 'warn' if warnings else 'pass','failures':failures,'warnings':warnings}
    print(json.dumps(out, indent=2))
    return 1 if failures else 0
if __name__=='__main__': sys.exit(main())
