#!/usr/bin/env python3
import argparse, json, pathlib, subprocess, sys
DEFAULT_BLOCKING = [
 ['python3','scripts/gloss_receipt_integrity_gate.py','--repo','.'],
 ['python3','scripts/gloss_semantic_naming_gate.py','--repo','.'],
 ['python3','scripts/gloss_source_scope_fixture_gate.py','--repo','.'],
 ['npm','run','build'],
 ['cargo','fmt','--manifest-path','src-tauri/Cargo.toml','--','--check'],
 ['cargo','test','--manifest-path','src-tauri/Cargo.toml'],
 ['cargo','test','--manifest-path','src-tauri/Cargo.toml','--features','semantic-memory-backend'],
 ['cargo','test','--manifest-path','src-tauri/Cargo.toml','--features','semantic-memory-turbo-quant'],
]
def main():
    ap=argparse.ArgumentParser(); ap.add_argument('--repo', default='.'); ap.add_argument('--run-release-blocking', action='store_true')
    args=ap.parse_args(); repo=pathlib.Path(args.repo).resolve()
    manifest={'release_blocking':DEFAULT_BLOCKING,'advisory':[],'stale_quarantined':[]}
    if not args.run_release_blocking:
        print(json.dumps(manifest, indent=2)); return 0
    results=[]; failed=False
    for cmd in DEFAULT_BLOCKING:
        path = repo/cmd[1] if len(cmd)>1 and cmd[1].startswith('scripts/') else None
        if path is not None and not path.exists():
            results.append({'cmd':cmd,'status':'fail','reason':'missing script'}); failed=True; continue
        try:
            cp=subprocess.run(cmd,cwd=repo,text=True,capture_output=True,timeout=600)
            results.append({'cmd':cmd,'status':'pass' if cp.returncode==0 else 'fail','returncode':cp.returncode,'stdout_tail':cp.stdout[-1000:],'stderr_tail':cp.stderr[-1000:]})
            if cp.returncode: failed=True
        except FileNotFoundError as e:
            results.append({'cmd':cmd,'status':'skip','reason':str(e)}); failed=True
        except subprocess.TimeoutExpired:
            results.append({'cmd':cmd,'status':'fail','reason':'timeout'}); failed=True
    print(json.dumps({'status':'fail' if failed else 'pass','results':results}, indent=2))
    return 1 if failed else 0
if __name__=='__main__': sys.exit(main())
