#!/usr/bin/env python3
import argparse
import json
import re
from pathlib import Path


REQUIRED_KEYS = {
    "text",
    "markdown",
    "code",
    "paste",
    "csv",
    "html_file",
    "pdf",
    "docx",
    "doc",
    "xlsx",
    "xls",
    "pptx",
    "ppt",
    "epub",
    "url",
    "youtube_transcript",
    "image",
    "audio",
    "video",
}


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
    sources = text(repo / "src-tauri/src/commands/sources/mod.rs")
    lib = text(repo / "src-tauri/src/lib.rs")
    ui = text(repo / "src/components/sources/SourcesPanel.tsx")
    tauri = text(repo / "src/lib/tauri.ts")
    contract = text(repo / "schemas/tauri-contract-v1.json")
    receipt_path = (
        repo
        / "docs/codex-runs"
        / (current_run(repo) or "__missing__")
        / "IMPORT_CAPABILITY_RECEIPT.json"
    )

    for key in REQUIRED_KEYS:
        if f'"{key}"' not in capability:
            failures.append(f"missing capability matrix key: {key}")

    required_capability_markers = [
        "pub enum ImportSupport",
        "SupportedDegraded",
        "Deferred",
        "Unsupported",
        "Unknown extensions are not silently widened to text import.",
        "UnsupportedCapabilityReceiptV1",
        "broad_spec_formats_are_explicitly_deferred_or_degraded",
        "unknown_extensions_do_not_silently_import_as_text",
    ]
    for marker in required_capability_markers:
        if marker not in capability:
            failures.append(f"missing import capability marker: {marker}")

    if 'get_import_capability_matrix' not in sources or 'get_import_capability_matrix' not in lib:
        failures.append("import capability command is not registered")
    if "getImportCapabilityMatrix" not in tauri:
        try:
            contract_entry = json.loads(contract).get("commands", {}).get("get_import_capability_matrix", {})
        except json.JSONDecodeError:
            contract_entry = {}
        if not (
            contract_entry.get("operator_only") is True
            and contract_entry.get("wrapper") is None
            and contract_entry.get("caller_count") == 0
        ):
            failures.append("frontend tauri wrapper does not expose import capability matrix")
    if "walk_report.skipped_unsupported" not in sources:
        failures.append("folder scan does not carry skipped unsupported/deferred count")
    if 'classify_import_extension(&ext)' not in sources:
        failures.append("folder/direct import does not use strict capability classifier")
    if "Treat unknown extensions as plain text" in sources:
        failures.append("legacy unknown-extension text fallback remains in sources command")
    if "local PDF/DOCX/DOC/XLSX/XLS/PPTX/PPT/EPUB extraction" not in ui:
        failures.append("source panel does not disclose local document extractor support")
    if "audio metadata" not in ui:
        failures.append("source panel does not disclose audio metadata support")
    if "cached Whisper audio transcription" not in ui:
        failures.append("source panel does not disclose cached Whisper audio transcription support")
    if "URL text fetch" not in ui:
        failures.append("source panel does not disclose URL text fetch support")
    if "YouTube transcript fetch" not in ui:
        failures.append("source panel does not disclose YouTube transcript support")
    if "legacy Office CLI extraction" not in ui:
        failures.append("source panel does not disclose degraded legacy Office CLI extraction")

    if not receipt_path.exists():
        failures.append(f"missing import capability receipt: {receipt_path.relative_to(repo)}")
    else:
        try:
            receipt = json.loads(receipt_path.read_text())
        except Exception as exc:
            failures.append(f"invalid import capability receipt JSON: {exc}")
            receipt = {}
        if receipt.get("schema") != "ImportCapabilityReceiptV1":
            failures.append("import capability receipt schema mismatch")
        if not receipt.get("strict_boundary_active"):
            failures.append("import capability receipt does not mark strict boundary active")
        receipt_keys = set(receipt.get("matrix_keys", []))
        missing_receipt_keys = sorted(REQUIRED_KEYS - receipt_keys)
        if missing_receipt_keys:
            failures.append(f"receipt missing matrix keys: {missing_receipt_keys}")
        supported = "\n".join(receipt.get("policy", {}).get("supported_degraded", []))
        deferred = "\n".join(receipt.get("policy", {}).get("deferred", []))
        if "youtube transcript" not in supported.lower():
            failures.append("import capability receipt does not list YouTube transcript as degraded support")
        if "legacy office" not in supported.lower():
            failures.append("import capability receipt does not list legacy Office as degraded support")
        if "youtube_transcript" in deferred:
            failures.append("import capability receipt still lists YouTube transcript as deferred")
        for stale in ["doc", "xls", "ppt"]:
            if stale in deferred.split():
                failures.append(f"import capability receipt still lists {stale} as deferred")

    print(json.dumps({"ok": not failures, "failures": failures}, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
