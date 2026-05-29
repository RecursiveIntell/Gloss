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
    if "DecodingSettingsReceiptV1" not in rust:
        failures.append("DecodingSettingsReceiptV1 is not implemented in Rust runtime code")
    if "DecodingSettingsReceiptV1" not in ts:
        failures.append("DecodingSettingsReceiptV1 frontend type/UI missing")
    if "temperature: 0.7" in rust:
        failures.append("chat/generation still contains hardcoded temperature 0.7")
    if "top_p" not in rust or "min_p" not in rust:
        failures.append("provider-aware top_p/min_p decoding fields missing or not wired")
    if "capability" not in rust.lower() or "unsupported" not in rust.lower():
        failures.append("provider capability guard for unsupported decoding fields not found")
    print(json.dumps({"ok": not failures, "failures": failures, "warnings": []}, indent=2))
    return 0 if not failures else 1

if __name__ == "__main__":
    raise SystemExit(main())
