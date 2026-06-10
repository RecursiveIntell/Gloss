#!/usr/bin/env python3
import argparse,json,pathlib

def main():
 p=argparse.ArgumentParser(); p.add_argument('--repo',default='.'); p.add_argument('--receipt',required=True); p.add_argument('--allow-non-certified',action='store_true'); a=p.parse_args()
 failures=[]; warnings=[]
 try: d=json.loads(pathlib.Path(a.receipt).read_text())
 except Exception as e:
  print(json.dumps({'ok':False,'failures':[f'cannot read receipt: {e}'],'warnings':[]}, indent=2)); return 1
 failures.extend(map(str,d.get('failures',[])))
 warnings.extend(map(str,d.get('warnings',[])))
 if not d.get('certified',False):
  msg='performance receipt is not certified'
  (warnings if a.allow_non_certified else failures).append(msg)
 m=d.get('measurements',{})
 if 'retrieval_merge_ms' in m and m['retrieval_merge_ms']>=200: failures.append(f"retrieval_merge_ms too high: {m['retrieval_merge_ms']}")
 if 'first_token_ms' in m and m['first_token_ms']>=2000: failures.append(f"first_token_ms too high: {m['first_token_ms']}")
 print(json.dumps({'ok':not failures,'failures':failures,'warnings':warnings}, indent=2))
 return 0 if not failures else 1
if __name__=='__main__': raise SystemExit(main())
