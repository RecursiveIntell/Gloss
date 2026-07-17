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
    receipt = repo / "docs/codex-runs" / (current_run(repo) or "__missing__") / "TURBOQUANT_RUNTIME_RECEIPT.json"
    if not receipt.exists():
        failures.append("TURBOQUANT_RUNTIME_RECEIPT.json missing for current run")
    else:
        try:
            data = json.loads(receipt.read_text())
        except json.JSONDecodeError as e:
            failures.append(f"TURBOQUANT_RUNTIME_RECEIPT.json is not valid JSON: {e}")
            print(json.dumps({"ok": False, "failures": failures, "warnings": []}, indent=2))
            return 1
        if data.get("runtime_claimed") is True:
            if data.get("exact_rerank") is not True:
                failures.append("TurboQuant runtime claimed without exact_rerank=true")
            if int(data.get("exact_rerank_count") or 0) <= 0:
                failures.append("TurboQuant runtime claimed without exact_rerank_count > 0")
            if not data.get("vector_artifact_manifest_digest"):
                failures.append("TurboQuant runtime claimed without vector artifact digest")
        elif "TurboQuant contribution" in text(repo / "README.md"):
            failures.append("TurboQuant contribution public claim not demoted while runtime_claimed is false/missing")
    print(json.dumps({"ok": not failures, "failures": failures, "warnings": []}, indent=2))
    return 0 if not failures else 1

if __name__ == "__main__":
    raise SystemExit(main())
