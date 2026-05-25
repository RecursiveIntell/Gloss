#!/usr/bin/env python3
import argparse, json
from pathlib import Path

from gloss_source_path_map import CURRENT_SOURCE_PATHS, stale_source_paths

def main():
 ap=argparse.ArgumentParser(); ap.add_argument('--repo', default='.') ; args=ap.parse_args(); root=Path(args.repo)
 failures=[]
 for p in CURRENT_SOURCE_PATHS.values():
  if not (root/p).exists(): failures.append(f'missing current source path: {p}')
 for script in (root/'scripts').glob('*.py'):
  txt=script.read_text(errors='ignore')
  for old in stale_source_paths():
   if old in txt: failures.append(f'{script}: stale hard-coded path {old}')
 print(json.dumps({'ok':not failures,'failures':failures}, indent=2))
 return 0 if not failures else 1
if __name__=='__main__': raise SystemExit(main())
