#!/usr/bin/env python3
import argparse, json, pathlib, re, sys

def grep(repo, rel, pat):
    p=repo/rel
    if not p.exists(): return []
    out=[]
    for i,line in enumerate(p.read_text(errors='ignore').splitlines(),1):
        if re.search(pat,line): out.append(f'{rel}:{i}:{line.strip()}')
    return out

def main():
    ap=argparse.ArgumentParser(); ap.add_argument('--repo', default='.')
    args=ap.parse_args(); repo=pathlib.Path(args.repo).resolve()
    failures=[]; warnings=[]
    failures += [f'polymorphic evidence/citations field: {x}' for x in grep(repo,'src/lib/types.ts',r'citations\?:.*\|')]
    failures += [f'hard-coded source_scope_preserved: {x}' for x in grep(repo,'src-tauri/src/commands/chat/mod.rs',r'source_scope_preserved:\s*true')]
    failures += [f'global dead_code allow: {x}' for x in grep(repo,'src-tauri/src/lib.rs',r'#!\[allow\(dead_code\)\]')]
    warnings += [f'untyped evidence/status Record: {x}' for x in grep(repo,'src/lib/types.ts',r'Record<string, unknown>')]
    warnings += [f'source text in system prompt: {x}' for x in grep(repo,'src-tauri/src/retrieval/context.rs',r'lives in the system message|Retrieved passages')]
    out={'status':'fail' if failures else 'warn' if warnings else 'pass','failures':failures,'warnings':warnings}
    print(json.dumps(out, indent=2))
    return 1 if failures else 0
if __name__=='__main__': sys.exit(main())
