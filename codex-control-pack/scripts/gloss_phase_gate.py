#!/usr/bin/env python3
import argparse, json, sys
PHASES=[f'P{i:02d}' for i in range(12)]
def main():
    ap=argparse.ArgumentParser(); ap.add_argument('--phase', required=True); ap.add_argument('--repo', default='.')
    args=ap.parse_args()
    failures=[]
    if args.phase not in PHASES: failures.append(f'unknown phase {args.phase}')
    checklist=[
      'source-of-truth owner identified for touched concepts',
      'no duplicate abstraction or shadow implementation introduced',
      'no silent widening/fallback/fake compatibility introduced',
      'material operations emit receipts or are explicitly non-material',
      'bitemporal/provenance invariants preserved or degraded explicitly',
      'tests/fixtures/assertions exist for new behavior',
      'failed/skipped validation recorded with exact reason',
    ]
    print(json.dumps({'status':'manual_required' if not failures else 'fail','phase':args.phase,'failures':failures,'required_manual_evidence':checklist}, indent=2))
    return 1 if failures else 0
if __name__=='__main__': sys.exit(main())
