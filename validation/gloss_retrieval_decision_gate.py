#!/usr/bin/env python3
import argparse
import json
from pathlib import Path

def text(path: Path) -> str:
    return path.read_text(errors="ignore") if path.exists() else ""

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default=".")
    repo = Path(parser.parse_args().repo)
    failures = []
    rust = "\n".join(text(p) for p in (repo / "src-tauri/src").rglob("*.rs"))
    ts = "\n".join(text(p) for p in (repo / "src").rglob("*.ts*"))
    if "struct RetrievalCapabilityDecisionV1" not in rust:
        failures.append("RetrievalCapabilityDecisionV1 Rust struct missing")
    if "RetrievalCapabilityDecisionV1" not in ts:
        failures.append("RetrievalCapabilityDecisionV1 frontend type missing")
    if rust.count("RetrievalCapabilityDecisionV1") < 2:
        failures.append("RetrievalCapabilityDecisionV1 appears declared but not used as canonical answer routing object")
    if "backend_requested" in ts and "backend_used" in ts and "RetrievalCapabilityDecisionV1" not in ts:
        failures.append("frontend still renders parallel backend requested/used fields instead of canonical decision object")
    print(json.dumps({"ok": not failures, "failures": failures, "warnings": []}, indent=2))
    return 0 if not failures else 1

if __name__ == "__main__":
    raise SystemExit(main())
