#!/usr/bin/env python3
import argparse, json, re
from pathlib import Path

def main():
 ap=argparse.ArgumentParser(); ap.add_argument('--repo', default='.') ; args=ap.parse_args(); root=Path(args.repo)
 p=root/'src/components/chat/ChatPanel.tsx'; txt=p.read_text(errors='ignore') if p.exists() else ''
 failures=[]
 bad_patterns=['effective_source_ids).join(", ")','selected_source_ids).join(", ")','excluded_source_ids.join(", ")','requested_source_ids.join(", ")']
 for pat in bad_patterns:
  if pat in txt: failures.append(f'normal evidence UI directly joins source ID list: {pat}')
 if 'Copy retrieval diagnostics JSON' not in txt: failures.append('missing diagnostics copy affordance')
 print(json.dumps({'ok':not failures,'failures':failures}, indent=2))
 return 0 if not failures else 1
if __name__=='__main__': raise SystemExit(main())
