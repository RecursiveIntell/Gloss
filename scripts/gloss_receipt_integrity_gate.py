#!/usr/bin/env python3
import argparse, json, pathlib, re, sys

def main():
    ap=argparse.ArgumentParser(); ap.add_argument('--repo', default='.')
    args=ap.parse_args(); repo=pathlib.Path(args.repo).resolve()
    failures=[]; warnings=[]
    refs=[]
    for path in [repo/'package.json', repo/'AGENTS.md']:
        if path.exists():
            text=path.read_text(errors='ignore')
            refs += re.findall(r'(scripts/[A-Za-z0-9_./-]+\.(?:py|sh))', text)
    for r in sorted(set(refs)):
        if not (repo/r).exists(): failures.append(f'missing referenced script: {r}')
    for receipt in repo.glob('docs/codex-runs/**/FINAL_RECEIPT.json'):
        try: data=json.loads(receipt.read_text())
        except Exception as e: failures.append(f'{receipt}: invalid json {e}'); continue
        if data.get('release_ready') is True:
            for cmd in data.get('commands_run',[]):
                for r in re.findall(r'(scripts/[A-Za-z0-9_./-]+\.(?:py|sh))', cmd):
                    if not (repo/r).exists(): failures.append(f'{receipt}: release_ready=true but command references missing {r}')
            smoke=receipt.parent/'desktop_smoke/final_desktop_smoke.json'
            if smoke.exists():
                try: s=json.loads(smoke.read_text())
                except Exception as e: failures.append(f'{smoke}: invalid json {e}'); continue
                answer=(s.get('assistant_response_preview') or '')
                quotes=' '.join([(c.get('quote') or '') for c in s.get('citations',[]) if isinstance(c,dict)])
                if 'ORCHID-913' in quotes and 'ORCHID-913' in answer and 'does not contain' in answer:
                    failures.append(f'{smoke}: assistant contradicts cited source containing ORCHID-913')
                if s.get('retrieval_mode') == 'source_order_fallback':
                    failures.append(f'{smoke}: release smoke uses source_order_fallback as proof')
    out={'status':'fail' if failures else 'pass','failures':failures,'warnings':warnings}
    print(json.dumps(out, indent=2))
    return 1 if failures else 0
if __name__=='__main__': sys.exit(main())
