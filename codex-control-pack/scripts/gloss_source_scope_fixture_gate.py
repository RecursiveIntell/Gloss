#!/usr/bin/env python3
import argparse, json, pathlib, re, sys

def main():
    ap=argparse.ArgumentParser(); ap.add_argument('--repo', default='.')
    args=ap.parse_args(); repo=pathlib.Path(args.repo).resolve()
    failures=[]; warnings=[]
    ss=repo/'src-tauri/src/retrieval/source_scope.rs'
    if not ss.exists(): failures.append('missing source_scope.rs')
    else:
        text=ss.read_text(errors='ignore')
        for needle in ['SourceScope::All','SourceScope::Explicit','SourceScope::None','resolves_invalid_explicit_scope_to_none_instead_of_all']:
            if needle not in text: failures.append(f'source scope fixture/variant missing: {needle}')
    fe=repo/'src/stores/sourceStore.ts'
    if fe.exists():
        text=fe.read_text(errors='ignore')
        if "return { kind: 'all' }" in text and 'stats.source_count !== sources.length' not in text:
            failures.append('frontend all-scope can widen without source-count/list parity')
    else: failures.append('missing sourceStore.ts')
    out={'status':'fail' if failures else 'pass','failures':failures,'warnings':warnings}
    print(json.dumps(out, indent=2))
    return 1 if failures else 0
if __name__=='__main__': sys.exit(main())
