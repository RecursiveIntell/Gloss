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
    if "SemanticMemoryRuntimeTruthV1" not in rust:
        failures.append("SemanticMemoryRuntimeTruthV1 is not implemented in Rust runtime code")
    if "SemanticMemoryRuntimeTruthV1" not in ts:
        failures.append("SemanticMemoryRuntimeTruthV1 is not exposed to frontend types/UI")
    if "runtime_truth" not in rust and "runtime truth" not in rust.lower():
        failures.append("no backend-authored semantic-memory runtime truth command/attachment found")
    print(json.dumps({"ok": not failures, "failures": failures, "warnings": []}, indent=2))
    return 0 if not failures else 1

if __name__ == "__main__":
    raise SystemExit(main())
