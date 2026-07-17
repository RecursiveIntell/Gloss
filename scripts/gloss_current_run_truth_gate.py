#!/usr/bin/env python3
import argparse, json, re
from pathlib import Path

def read(p):
    return p.read_text(errors='ignore') if p.exists() else ''

def manifest_run_id(root):
    try:
        data=json.loads(read(root/'PACK_MANIFEST.json') or '{}')
    except Exception:
        return None
    value=data.get('run_id')
    return value.strip() if isinstance(value, str) and value.strip() else None

def main():
    ap=argparse.ArgumentParser()
    ap.add_argument('--repo', default='.')
    ap.add_argument('--expected-run-id', default=None)
    args=ap.parse_args()
    root=Path(args.repo)
    failures=[]; warnings=[]
    expected_run=args.expected_run_id or manifest_run_id(root)
    current=read(root/'docs/codex-runs/CURRENT_RUN.md')
    m=re.search(r'Current run:\s*`?([^`\n]+)`?', current)
    current_run=m.group(1).strip() if m else None
    agents=read(root/'AGENTS.md')
    readme=read(root/'README.md')
    pkg=json.loads(read(root/'package.json') or '{}')
    scripts=pkg.get('scripts',{})
    if not current_run: failures.append('CURRENT_RUN.md missing parseable Current run line')
    if current_run and current_run.startswith('#'): failures.append(f'CURRENT_RUN parsed as heading, not run id: {current_run!r}')
    if expected_run and current_run and current_run != expected_run:
        failures.append(f'CURRENT_RUN is {current_run}, expected {expected_run}')
    if current_run and not (root/'docs/codex-runs'/current_run).is_dir():
        failures.append(f'current run directory docs/codex-runs/{current_run} is missing')
    for label, text in [('AGENTS.md',agents),('README.md',readme)]:
        if current_run and current_run not in text:
            failures.append(f'{label} does not reference current run {current_run}')
        if 'GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519' in text and current_run != 'GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519':
            failures.append(f'{label} references P33 while CURRENT_RUN is {current_run}')
        for match in re.findall(r'scripts/(p33_[A-Za-z0-9_]+\.(?:py|sh))', text):
            if not (root/'scripts'/match).exists(): failures.append(f'{label} references missing scripts/{match}')
    for name, cmd in scripts.items():
        for match in re.findall(r'scripts/(p33_[A-Za-z0-9_]+\.(?:py|sh))', cmd):
            if not (root/'scripts'/match).exists(): failures.append(f'package.json script {name} references missing scripts/{match}')
    receipt_paths=list((root/'docs/codex-runs').glob('*/FINAL_RECEIPT.json'))
    for rp in receipt_paths:
        try: data=json.loads(read(rp))
        except Exception as e:
            failures.append(f'{rp}: invalid JSON {e}'); continue
        if data.get('release_ready') is True:
            rid=data.get('run_id') or rp.parent.name
            if current_run and rid != current_run:
                failures.append(f'{rp}: release_ready=true for {rid}, but CURRENT_RUN is {current_run}')
    out={'ok': not failures, 'current_run': current_run, 'expected_run': expected_run, 'failures': failures, 'warnings': warnings}
    print(json.dumps(out, indent=2))
    return 0 if not failures else 1
if __name__=='__main__': raise SystemExit(main())
