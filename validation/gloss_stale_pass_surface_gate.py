#!/usr/bin/env python3
import argparse, json, pathlib, re, sys
RUN_ID = "GLOSS_RELEASE_PROOF_EMBEDDING_UNIFICATION_CLEANUP_20260525"
OLD_RUN_RE = re.compile(r"(P31|P32|P33|P34|P35|P36|GLOSS_P3[1-6]|p31|p32|p33|p34|p35|p36)")
FORBIDDEN_ROOT_DIRS = ["p33boot", "phase_prompts", "manual_injections", "prompts"]
FORBIDDEN_SKILL_PATTERNS = ["p31", "p32", "p33", "p34", "p35", "p36"]

def read(p):
    try: return p.read_text(errors="ignore")
    except Exception: return ""

def main():
    ap=argparse.ArgumentParser()
    ap.add_argument('--repo', default='.')
    args=ap.parse_args()
    repo=pathlib.Path(args.repo).resolve()
    failures=[]; warnings=[]
    for d in FORBIDDEN_ROOT_DIRS:
        p=repo/d
        if p.exists(): failures.append(f"stale root pass directory still active: {d}")
    for p in [repo/'AGENTS.md', repo/'README.md', repo/'ACCEPTANCE_GATES.md']:
        if p.exists():
            txt=read(p)
            if 'P36 Release Completion Pass Bundle' in txt or 'GLOSS_P36_RELEASE_COMPLETION' in txt:
                failures.append(f"{p.relative_to(repo)} still contains old P36 pass instruction text")
    skills=repo/'.agents'/'skills'
    if skills.exists():
        for child in skills.iterdir():
            if any(pat in child.name.lower() for pat in FORBIDDEN_SKILL_PATTERNS):
                failures.append(f"stale active skill remains: {child.relative_to(repo)}")
    cur=repo/'docs'/'codex-runs'/'CURRENT_RUN.md'
    if not cur.exists() or RUN_ID not in read(cur):
        failures.append(f"CURRENT_RUN.md missing or not set to {RUN_ID}")
    run_dir=repo/'docs'/'codex-runs'/RUN_ID
    if not run_dir.exists():
        warnings.append(f"current run dir missing: {run_dir.relative_to(repo)}")
    manifest=run_dir/'STALE_PASS_CLEANUP_MANIFEST.json'
    if run_dir.exists() and not manifest.exists():
        failures.append(f"missing cleanup manifest: {manifest.relative_to(repo)}")
    out={"ok": not failures, "failures": failures, "warnings": warnings}
    print(json.dumps(out, indent=2))
    return 0 if not failures else 1
if __name__ == '__main__': raise SystemExit(main())
