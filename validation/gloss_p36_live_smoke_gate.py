#!/usr/bin/env python3
import argparse, json, pathlib, sys
STATUSES={"pass","fail","not_exercised"}
REQUIRED=[
 ("app_launch","dev_mode"),("app_launch","appimage"),("provider","health"),
 ("chat","no_retrieval"),("chat","retrieval"),("chat","stop"),("chat","regenerate"),("chat","continue_partial"),("chat","notebook_switch_terminal_clear"),("chat","persist_after_reload"),
 ("portability","export"),("portability","import"),("portability","post_import_retrieval"),
]
def main():
 p=argparse.ArgumentParser(); p.add_argument('--repo', default='.'); p.add_argument('--receipt', required=True); p.add_argument('--allow-not-exercised', action='store_true'); a=p.parse_args()
 failures=[]; warnings=[]
 try: data=json.loads(pathlib.Path(a.receipt).read_text())
 except Exception as e:
  print(json.dumps({'ok':False,'failures':[f'cannot read receipt: {e}'],'warnings':[]}, indent=2)); return 1
 for k1,k2 in REQUIRED:
  v=data.get(k1,{}).get(k2)
  if v not in STATUSES: failures.append(f'{k1}.{k2} invalid/missing: {v!r}')
  elif v=='fail': failures.append(f'{k1}.{k2} failed')
  elif v=='not_exercised':
   msg=f'{k1}.{k2} not_exercised'
   (warnings if a.allow_not_exercised else failures).append(msg)
 for area in ('studio',):
  for k,v in data.get(area,{}).items():
   if v=='fail': failures.append(f'{area}.{k} failed')
   elif v=='not_exercised': warnings.append(f'{area}.{k} not_exercised')
 if data.get('failures'):
  failures.extend(str(x) for x in data['failures'])
 print(json.dumps({'ok':not failures,'failures':failures,'warnings':warnings}, indent=2))
 return 0 if not failures else 1
if __name__=='__main__': raise SystemExit(main())
