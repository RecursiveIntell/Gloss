#!/usr/bin/env python3
import argparse
import json
import re
import shutil
from pathlib import Path


REQUIRED_TOOLS = {"doc": "antiword", "xls": "xls2csv", "ppt": "catppt"}


def text(path: Path) -> str:
    return path.read_text(errors="ignore") if path.exists() else ""


def current_run(repo: Path) -> str | None:
    match = re.search(
        r"Current run:\s*`?([^`\n]+)`?",
        text(repo / "docs/codex-runs/CURRENT_RUN.md"),
    )
    return match.group(1).strip() if match else None


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default=".")
    args = parser.parse_args()
    repo = Path(args.repo).resolve()
    failures: list[str] = []

    capability = text(repo / "src-tauri/src/ingestion/import_capability.rs")
    extract = text(repo / "src-tauri/src/ingestion/extract.rs")
    sources = text(repo / "src-tauri/src/commands/sources/mod.rs")
    ui = text(repo / "src/components/sources/SourcesPanel.tsx")
    run_id = current_run(repo) or "__missing__"
    receipt_path = repo / "docs/codex-runs" / run_id / "LEGACY_OFFICE_EXTRACTOR_RECEIPT.json"

    for fmt, tool in REQUIRED_TOOLS.items():
        if f'key: "{fmt}"' not in capability:
            failures.append(f"missing legacy Office capability key: {fmt}")
        if f'language: Some("{fmt}")' not in capability:
            failures.append(f"missing legacy Office language metadata: {fmt}")
        if tool not in extract:
            failures.append(f"missing extractor tool marker for {fmt}: {tool}")
        if shutil.which(tool) is None:
            failures.append(f"legacy Office extractor binary unavailable on PATH: {tool}")
    if "LegacyOfficeExtractorReceiptV1" not in capability:
        failures.append("capability matrix does not expose LegacyOfficeExtractorReceiptV1")
    for marker in [
        "extract_legacy_office",
        "LEGACY_OFFICE_TIMEOUT_MS",
        "MAX_LEGACY_OFFICE_BYTES",
        "wait_timeout",
        "argv_redacted",
        "source_path_redacted",
        "DocumentExtractionMetadataV1",
        "document_extraction",
        "legacy_office_extractor",
    ]:
        haystack = extract + "\n" + sources
        if marker not in haystack:
            failures.append(f"missing legacy Office runtime marker: {marker}")

    if "legacy Office CLI extraction" not in ui:
        failures.append("source panel does not disclose degraded legacy Office CLI extraction")
    for ext in ['"doc"', '"xls"', '"ppt"']:
        if ext not in ui:
            failures.append(f"sources file picker does not include legacy extension {ext}")

    if not receipt_path.exists():
        failures.append(f"missing legacy Office receipt: {receipt_path.relative_to(repo)}")
    else:
        try:
            receipt = json.loads(receipt_path.read_text())
        except Exception as exc:
            failures.append(f"invalid legacy Office receipt JSON: {exc}")
            receipt = {}
        if receipt.get("schema") != "LegacyOfficeExtractorImplementationReceiptV1":
            failures.append("legacy Office receipt schema mismatch")
        if receipt.get("support") != "supported_degraded":
            failures.append("legacy Office receipt support is not supported_degraded")
        if receipt.get("runtime_receipt_schema") != "LegacyOfficeExtractorReceiptV1":
            failures.append("legacy Office runtime receipt schema mismatch")
        if receipt.get("tools") != REQUIRED_TOOLS:
            failures.append("legacy Office receipt tool map mismatch")
        if not receipt.get("strict_tool_boundary"):
            failures.append("legacy Office receipt does not mark strict tool boundary")
        if not receipt.get("metadata_persistence"):
            failures.append("legacy Office receipt does not mark metadata persistence")

    print(json.dumps({"ok": not failures, "failures": failures}, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
