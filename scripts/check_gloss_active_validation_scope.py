#!/usr/bin/env python3
"""Audit active Gloss validation scope for stale cross-project references."""
from __future__ import annotations
import argparse, json, os, re, sys
from pathlib import Path

BAD_PATTERNS = [
    r"SCR_P0A",
    r"scr-kernel",
    r"SCR runtime",
    r"SCR P0",
]
MISSING_REF_PATTERNS = [
    r"\.codex/tools/auto_phase_runner\.py",
]
ACTIVE_DIRS = ["scripts", ".codex"]
IGNORE_PARTS = {"archive", "archives", "node_modules", "target", "dist", "vendor", ".git"}
IGNORE_FILES = {"check_gloss_active_validation_scope.py"}
TEXT_SUFFIXES = {".py", ".sh", ".md", ".toml", ".json", ".yaml", ".yml", ".txt"}

def is_text_file(path: Path) -> bool:
    return path.suffix in TEXT_SUFFIXES or path.name in {"hooks.json"}

def should_ignore(path: Path) -> bool:
    return path.name in IGNORE_FILES or any(part in IGNORE_PARTS for part in path.parts)

def scan_file(path: Path, repo: Path):
    findings = []
    if should_ignore(path) or not is_text_file(path):
        return findings
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except Exception as exc:
        findings.append({"severity":"warning","code":"read-failed","path":str(path.relative_to(repo)),"detail":str(exc)})
        return findings
    rel = str(path.relative_to(repo))
    for pat in BAD_PATTERNS:
        if re.search(pat, text):
            findings.append({"severity":"error","code":"cross-project-validation-contamination","path":rel,"detail":f"matched {pat}"})
    for pat in MISSING_REF_PATTERNS:
        if re.search(pat, text):
            target = repo / ".codex" / "tools" / "auto_phase_runner.py"
            if not target.exists():
                findings.append({"severity":"error","code":"missing-active-script-reference","path":rel,"detail":"references .codex/tools/auto_phase_runner.py but that file is absent"})
    return findings

def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", default=".")
    args = ap.parse_args()
    repo = Path(args.repo).resolve()
    findings = []
    for d in ACTIVE_DIRS:
        root = repo / d
        if not root.exists():
            continue
        for path in root.rglob("*"):
            if path.is_file():
                findings.extend(scan_file(path, repo))
    errors = [f for f in findings if f.get("severity") == "error"]
    result = {"ok": not errors, "error_count": len(errors), "finding_count": len(findings), "findings": findings}
    print(json.dumps(result, indent=2))
    return 0 if not errors else 1

if __name__ == "__main__":
    raise SystemExit(main())
