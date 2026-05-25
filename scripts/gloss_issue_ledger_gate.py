#!/usr/bin/env python3
import argparse, csv, json
from pathlib import Path

def main():
 ap=argparse.ArgumentParser(); ap.add_argument('--ledger', required=True); args=ap.parse_args()
 rows=list(csv.DictReader(open(args.ledger)))
 failures=[]
 if not rows: failures.append('empty issue ledger')
 for r in rows:
  for col in ['id','severity','surface','evidence','fix_direction','acceptance_gate','status']:
   if not r.get(col): failures.append(f'{r.get("id","<unknown>")}: missing {col}')
 print(json.dumps({'ok':not failures,'issue_count':len(rows),'failures':failures[:50]}, indent=2))
 return 0 if not failures else 1
if __name__=='__main__': raise SystemExit(main())
