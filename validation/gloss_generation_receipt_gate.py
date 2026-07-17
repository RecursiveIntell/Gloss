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
    if "GenerationReceiptV1" not in rust:
        failures.append("GenerationReceiptV1 is not implemented in Rust runtime code")
    if "GenerationReceiptV1" not in ts:
        failures.append("GenerationReceiptV1 frontend type/UI missing")
    if "generation_receipts" not in migrations:
        failures.append("generation_receipts DB table/migration missing")
    if "provider_request_digest" not in rust:
        failures.append("provider request digest capture missing")
    print(json.dumps({"ok": not failures, "failures": failures, "warnings": []}, indent=2))
    return 0 if not failures else 1

if __name__ == "__main__":
    raise SystemExit(main())
