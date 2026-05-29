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
    receipt = repo / "docs/codex-runs" / (current_run(repo) or "__missing__") / "LIVE_SEMANTIC_MEMORY_SMOKE_RECEIPT.json"
    if not receipt.exists():
        failures.append("LIVE_SEMANTIC_MEMORY_SMOKE_RECEIPT.json missing for current run")
        print(json.dumps({"ok": False, "failures": failures, "warnings": []}, indent=2))
        return 1
    data = json.loads(receipt.read_text())
    if data.get("backend_used") != "semantic-memory-preview":
        failures.append("live semantic-memory smoke did not use semantic-memory-preview")
    if data.get("fallback_used") is not False:
        failures.append("live semantic-memory smoke fallback_used is not false")
    print(json.dumps({"ok": not failures, "failures": failures, "warnings": []}, indent=2))
    return 0 if not failures else 1

if __name__ == "__main__":
    raise SystemExit(main())
