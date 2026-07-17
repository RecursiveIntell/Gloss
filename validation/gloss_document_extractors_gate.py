#!/usr/bin/env python3
import argparse
import json
import re
from pathlib import Path


REQUIRED_FORMATS = {"pdf", "docx", "xlsx", "pptx", "epub"}
REQUIRED_LEGACY_FORMATS = {"doc", "xls", "ppt"}


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

    cargo = text(repo / "src-tauri/Cargo.toml")
    capability = text(repo / "src-tauri/src/ingestion/import_capability.rs")
    extract = text(repo / "src-tauri/src/ingestion/extract.rs")
    sources_panel = text(repo / "src/components/sources/SourcesPanel.tsx")
    run_id = current_run(repo) or "__missing__"
    receipt_path = (
        repo / "docs/codex-runs" / run_id / "DOCUMENT_EXTRACTOR_RECEIPT.json"
    )

    for dep in ["pdf-extract", "quick-xml", "zip", "wait-timeout"]:
        if dep not in cargo:
            failures.append(f"missing direct document extractor dependency: {dep}")

    for fmt in REQUIRED_FORMATS:
        if f'key: "{fmt}"' not in capability:
            failures.append(f"missing import capability key for {fmt}")
        if f'language: Some("{fmt}")' not in capability:
            failures.append(f"missing language metadata for {fmt}")
    if "DocumentExtractorReceiptV1" not in capability:
        failures.append("capability matrix does not require DocumentExtractorReceiptV1")
    for fmt in REQUIRED_LEGACY_FORMATS:
        if f'key: "{fmt}"' not in capability:
            failures.append(f"missing legacy Office capability key for {fmt}")
        if f'language: Some("{fmt}")' not in capability:
            failures.append(f"missing legacy Office language metadata for {fmt}")
    if "LegacyOfficeExtractorReceiptV1" not in capability:
        failures.append("capability matrix does not require LegacyOfficeExtractorReceiptV1")

    required_extract_markers = [
        "MAX_DOCUMENT_ARCHIVE_ENTRIES",
        "MAX_DOCUMENT_XML_BYTES",
        "MAX_PDF_BYTES",
        "MAX_DOCUMENT_TEXT_CHARS",
        "extract_pdf",
        "extract_docx",
        "extract_xlsx",
        "extract_pptx",
        "extract_epub",
        "extract_legacy_office",
        "legacy_office_extractor_for_format",
        "antiword",
        "xls2csv",
        "catppt",
        "LEGACY_OFFICE_TIMEOUT_MS",
        "MAX_LEGACY_OFFICE_BYTES",
        "DocumentExtractionMetadataV1",
        "legacy_office_extractor",
        "is_safe_zip_entry_name",
        "legacy_office_extractors_have_strict_tool_mapping",
        "legacy_office_receipt_redacts_paths_and_records_bounds",
        "malformed_pdf_fails_instead_of_widening_to_plain_text",
        "malformed_docx_fails_instead_of_widening_to_plain_text",
        "extracts_pdf_docx_pptx_xlsx_epub_from_strict_boundaries",
    ]
    for marker in required_extract_markers:
        if marker not in extract:
            failures.append(f"missing document extractor marker: {marker}")

    if "local PDF/DOCX/DOC/XLSX/XLS/PPTX/PPT/EPUB extraction" not in sources_panel:
        failures.append("sources panel does not disclose document extractor support")
    if "legacy Office CLI extraction" not in sources_panel:
        failures.append("sources panel does not disclose degraded legacy Office CLI extraction")
    for ext in ['"pdf"', '"docx"', '"doc"', '"xlsx"', '"xls"', '"pptx"', '"ppt"', '"epub"']:
        if ext not in sources_panel:
            failures.append(f"sources file picker does not include {ext}")

    if not receipt_path.exists():
        failures.append(f"missing document extractor receipt: {receipt_path.relative_to(repo)}")
    else:
        try:
            receipt = json.loads(receipt_path.read_text())
        except Exception as exc:
            failures.append(f"invalid document extractor receipt JSON: {exc}")
            receipt = {}
        if receipt.get("schema") != "DocumentExtractorReceiptV1":
            failures.append("document extractor receipt schema mismatch")
        if set(receipt.get("formats_supported_degraded", [])) != REQUIRED_FORMATS:
            failures.append("document extractor receipt format list mismatch")
        if set(receipt.get("legacy_office_supported_degraded", [])) != REQUIRED_LEGACY_FORMATS:
            failures.append("document extractor receipt legacy Office format list mismatch")
        tools = receipt.get("legacy_office_tools", {})
        for fmt, tool in {"doc": "antiword", "xls": "xls2csv", "ppt": "catppt"}.items():
            if tools.get(fmt) != tool:
                failures.append(f"document extractor receipt legacy tool mismatch for {fmt}")
        if not receipt.get("strict_zip_xml_boundary"):
            failures.append("document extractor receipt does not mark strict boundary active")
        if not receipt.get("strict_pdf_boundary"):
            failures.append("document extractor receipt does not mark strict PDF boundary active")
        if not receipt.get("strict_legacy_office_tool_boundary"):
            failures.append("document extractor receipt does not mark strict legacy Office tool boundary active")
        tests = " ".join(receipt.get("tests", []))
        if "extract -- --nocapture" not in tests:
            failures.append("document extractor receipt does not record focused extraction test")

    print(json.dumps({"ok": not failures, "failures": failures}, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
