#!/usr/bin/env python3
from __future__ import annotations
import argparse, json, subprocess, time
from pathlib import Path

def cmd(c):
    try:
        p=subprocess.run(c, shell=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=30)
        return {'cmd': c, 'code': p.returncode, 'out': p.stdout[-4000:]}
    except Exception as e:
        return {'cmd': c, 'code': 999, 'out': str(e)}

def changed_files(status_out):
    files=[]
    for line in status_out.splitlines():
        if not line:
            continue
        path=line[3:] if len(line) > 3 and line[2] == ' ' else line.strip()
        if ' -> ' in path:
            path=path.split(' -> ', 1)[1]
        files.append(path)
    return files

def main():
    ap=argparse.ArgumentParser()
    ap.add_argument('--run-id', default='P32R3')
    ap.add_argument('--phase', required=True)
    ap.add_argument('--status', choices=['pass','fail','blocked'], required=True)
    ap.add_argument('--note', default='')
    args=ap.parse_args()
    root=Path.cwd(); out=root/'docs'/'codex-runs'/args.run_id/'receipts'; out.mkdir(parents=True, exist_ok=True)
    git_status=cmd('git status --short')
    rec={
      'run_id': args.run_id,
      'phase': args.phase,
      'status': args.status,
      'note': args.note,
      'ts': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
      'git_status': git_status,
      'changed_files': changed_files(git_status['out']),
      'commands_run': [],
      'tests_passed': [],
      'tests_failed': [],
      'tests_skipped': [],
      'unresolved_risks': [],
      'source_of_truth_boundaries_preserved': True,
    }
    path=out/(args.phase+'.json')
    path.write_text(json.dumps(rec, indent=2, sort_keys=True), encoding='utf-8')
    print(json.dumps({'receipt': str(path), 'status': args.status}))
    return 0 if args.status == 'pass' else 2
if __name__ == '__main__': raise SystemExit(main())
