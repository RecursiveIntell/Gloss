#!/usr/bin/env python3
import argparse
import json
import re
from pathlib import Path

def text(path: Path) -> str:
    return path.read_text(errors="ignore") if path.exists() else ""

def current_run(repo: Path) -> str | None:
    match = re.search(r"Current run:\s*`?([^`\n]+)`?", text(repo / "docs/codex-runs/CURRENT_RUN.md"))
    return match.group(1).strip() if match else None

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default=".")
    repo = Path(parser.parse_args().repo)
    failures = []
    rust = "\n".join(text(p) for p in (repo / "src-tauri/src").rglob("*.rs"))
    ts = "\n".join(text(p) for p in (repo / "src").rglob("*.ts*"))
    receipt = repo / "docs/codex-runs" / (current_run(repo) or "__missing__") / "TIMEOUT_CHANGE_RECEIPT.json"
    if not receipt.exists():
        failures.append("TimeoutChangeReceiptV1 receipt missing for current run")
    if "continue" not in ts.lower() and "continuation" not in ts.lower():
        failures.append("continuation action UI not found")
    if "partial" not in rust.lower():
        failures.append("partial output persistence/status not found in backend")
    print(json.dumps({"ok": not failures, "failures": failures, "warnings": []}, indent=2))
    return 0 if not failures else 1

if __name__ == "__main__":
    raise SystemExit(main())
