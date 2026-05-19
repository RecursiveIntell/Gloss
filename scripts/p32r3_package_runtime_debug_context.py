#!/usr/bin/env python3
from __future__ import annotations
import argparse, hashlib, json, os, time, zipfile
from pathlib import Path

INCLUDE = [
 'src-tauri/src','src','package.json','package-lock.json','src-tauri/Cargo.toml','src-tauri/Cargo.lock',
 'AGENTS.md','README.md','P32R3_SOURCE_BASIS.md','P32R3_ACCEPTANCE_GATES.md','docs/codex-runs/P32R3','scripts'
]
EXCLUDE_PARTS = ['src-tauri/vendor/','node_modules/','target/','dist/','.git/','.codex/log/']

def should_include(path):
    extra_parts = [
        'docs/codex-runs/P32R3/install-backup-',
        'docs/codex-runs/P32R3/packages/',
        'src/stores/memory/',
        '__pycache__/',
    ]
    exclude_suffixes = (
        '.pyc', '.pyo', '.db', '.sqlite', '.sqlite3',
        '.hnsw.data', '.hnsw.graph',
        '.zip', '.tar', '.tar.gz', '.tgz', '.7z', '.rar',
    )
    s=str(path).replace('\\','/')
    return not any(part in s for part in EXCLUDE_PARTS + extra_parts) and not s.endswith(exclude_suffixes)

def main():
    ap=argparse.ArgumentParser(); ap.add_argument('--run-id', default='P32R3'); args=ap.parse_args()
    root=Path.cwd(); outdir=root/'docs'/'codex-runs'/args.run_id/'packages'; outdir.mkdir(parents=True, exist_ok=True)
    zpath=outdir/f'gloss-{args.run_id.lower()}-runtime-debug-context.zip'
    files=[]
    for rel in INCLUDE:
        p=root/rel
        if p.is_file(): files.append(p)
        elif p.is_dir(): files += [x for x in p.rglob('*') if x.is_file()]
    files=sorted(set(f for f in files if should_include(f.relative_to(root))))
    with zipfile.ZipFile(zpath,'w',zipfile.ZIP_DEFLATED) as z:
        for f in files:
            z.write(f, f.relative_to(root).as_posix())
    h=hashlib.sha256(zpath.read_bytes()).hexdigest()
    manifest={
      'run_id':args.run_id,
      'created_utc':time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
      'file_count':len(files),
      'total_bytes':sum(f.stat().st_size for f in files),
      'zip':str(zpath),
      'sha256':h,
      'excluded_parts':EXCLUDE_PARTS,
      'included_paths':[f.relative_to(root).as_posix() for f in files],
    }
    (outdir/'runtime-debug-context-manifest.json').write_text(json.dumps(manifest, indent=2, sort_keys=True), encoding='utf-8')
    print(json.dumps(manifest, indent=2))
if __name__ == '__main__': raise SystemExit(main())
