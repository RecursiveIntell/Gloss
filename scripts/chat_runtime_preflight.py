#!/usr/bin/env python3
"""Preflight for the Gloss chat runtime fix pass."""
from __future__ import annotations
import argparse, json, os, subprocess, sys
from pathlib import Path

REQUIRED = [
    "src/App.tsx",
    "src/stores/chatStore.ts",
    "src-tauri/src/commands/chat.rs",
    "src-tauri/src/commands/settings.rs",
    "src-tauri/src/providers/mod.rs",
    "src-tauri/src/providers/ollama.rs",
    "src-tauri/src/db/app_db.rs",
    "src/lib/tauri.ts",
    "src/lib/events.ts",
]

def run(cmd, cwd):
    try:
        p = subprocess.run(cmd, cwd=cwd, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=20)
        return p.returncode, p.stdout.strip()
    except Exception as e:
        return 999, str(e)

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", default=".")
    args = ap.parse_args()
    repo = Path(args.repo).resolve()
    print(f"CHAT_RUNTIME_PREFLIGHT repo={repo}")
    missing = []
    for rel in REQUIRED:
        if not (repo/rel).exists():
            missing.append(rel)
    print("required_files_missing=" + json.dumps(missing))
    code, out = run(["git", "status", "--short"], repo)
    print(f"git_status_code={code}")
    print(out)
    current_run = repo/"docs/codex-runs/CURRENT_RUN.md"
    print(f"current_run_exists={current_run.exists()}")
    if current_run.exists():
        print("current_run_content=" + current_run.read_text(encoding='utf-8', errors='replace').strip())
    run_dir = repo/"docs/codex-runs/CHAT_RUNTIME_FIX_20260518"
    print(f"run_dir_exists={run_dir.exists()}")
    return 1 if missing else 0

if __name__ == "__main__":
    raise SystemExit(main())
