#!/usr/bin/env python3
import argparse, json, pathlib, re, sys
OLD_RUN_RE = re.compile(r"(P31|P32|P33|P34|P35|P36|GLOSS_P3[1-6]|p31|p32|p33|p34|p35|p36)")
# `prompts/` is an active source-owned Studio prompt directory. The other
# directories are historical pass scaffolding and must not remain active.
FORBIDDEN_ROOT_DIRS = ["p33boot", "phase_prompts", "manual_injections"]
FORBIDDEN_SKILL_PATTERNS = ["p31", "p32", "p33", "p34", "p35", "p36"]

def read(p):
    try: return p.read_text(errors="ignore")
    except Exception: return ""

def current_run(repo):
    m = re.search(r"Current run:\s*`?([^`\n]+)`?", read(repo / "docs/codex-runs/CURRENT_RUN.md"))
    return m.group(1).strip() if m else None

def main():
    ap=argparse.ArgumentParser()
    ap.add_argument('--repo', default='.')
    args=ap.parse_args()
    repo=pathlib.Path(args.repo).resolve()
    failures=[]; warnings=[]
    run_id=current_run(repo)
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
    if not cur.exists() or not run_id:
        failures.append("CURRENT_RUN.md missing or not parseable")
    run_dir=repo/'docs'/'codex-runs'/(run_id or "__missing__")
    if not run_dir.exists():
        warnings.append(f"current run dir missing: {run_dir.relative_to(repo)}")
    manifest=run_dir/'STALE_PASS_CLEANUP_MANIFEST.json'
    if run_dir.exists() and not manifest.exists():
        failures.append(f"missing cleanup manifest: {manifest.relative_to(repo)}")
    out={"ok": not failures, "failures": failures, "warnings": warnings}
    print(json.dumps(out, indent=2))
    return 0 if not failures else 1
if __name__ == '__main__': raise SystemExit(main())
