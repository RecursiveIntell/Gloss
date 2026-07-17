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
    migrations = text(repo / "src-tauri/src/db/migrations.rs")
    if "PromptReceiptV1" not in rust:
        failures.append("PromptReceiptV1 is not implemented in Rust runtime code")
    if "PromptReceiptV1" not in ts:
        failures.append("PromptReceiptV1 frontend type/UI missing")
    if "prompt_receipts" not in migrations:
        failures.append("prompt_receipts DB table/migration missing")
    if "context_payload_digest" not in rust:
        failures.append("context payload digest capture missing")
    print(json.dumps({"ok": not failures, "failures": failures, "warnings": []}, indent=2))
    return 0 if not failures else 1

if __name__ == "__main__":
    raise SystemExit(main())
