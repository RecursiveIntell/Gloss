#!/usr/bin/env python3
import argparse, json, pathlib, subprocess, sys, tempfile, zipfile, shutil, os

def main():
    ap=argparse.ArgumentParser(); ap.add_argument('--archive'); ap.add_argument('--repo', default='.'); ap.add_argument('--fresh-unzip')
    args=ap.parse_args(); repo=pathlib.Path(args.repo).resolve()
    failures=[]; target=None
    if args.archive:
        archive=pathlib.Path(args.archive).resolve()
        target=pathlib.Path(args.fresh_unzip or tempfile.mkdtemp(prefix='gloss-replay-')).resolve()
        if target.exists(): shutil.rmtree(target)
        target.mkdir(parents=True)
        with zipfile.ZipFile(archive) as z: z.extractall(target)
        candidates=[p for p in target.iterdir() if p.is_dir()]
        if len(candidates)==1: target=candidates[0]
    else:
        target=repo
    required=['package.json','src-tauri/Cargo.toml','src-tauri/tauri.conf.json']
    for r in required:
        if not (target/r).exists(): failures.append(f'missing {r} in replay root {target}')
    for script in ['scripts/gloss_receipt_integrity_gate.py','scripts/gloss_semantic_naming_gate.py','scripts/gloss_source_scope_fixture_gate.py']:
        if not (target/script).exists(): failures.append(f'missing release gate script {script}')
    print(json.dumps({'status':'fail' if failures else 'pass','root':str(target),'failures':failures}, indent=2))
    return 1 if failures else 0
if __name__=='__main__': sys.exit(main())
