#!/usr/bin/env python3
import argparse
import json
import re
from pathlib import Path


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
    lib = text(repo / "src-tauri/src/lib.rs")
    tauri = text(repo / "src/lib/tauri.ts")
    store = text(repo / "src/stores/sourceStore.ts")
    ui = text(repo / "src/components/sources/SourcesPanel.tsx")
    run_id = current_run(repo) or "__missing__"
    receipt_path = repo / "docs/codex-runs" / run_id / "URL_IMPORT_RECEIPT.json"

    for marker in [
        'key: "url"',
        'source_type: Some("url")',
        "SupportedDegraded",
        "UrlImportReceiptV1",
        "user-consented HTTP(S) fetch",
    ]:
        if marker not in capability:
            failures.append(f"missing URL capability marker: {marker}")

    if '"paste" | "url"' not in extract:
        failures.append("extract_text does not route URL sources through stored content_text")

    for marker in [
        "add_source_url",
        "UrlImportReceipt",
        "MAX_URL_IMPORT_BYTES",
        "MAX_URL_IMPORT_REDIRECTS",
        "URL_IMPORT_TIMEOUT_SECS",
        "canonical_url_for_fetch",
        "validate_url_dns_boundary",
        "is_disallowed_url_import_ip",
        "html_to_readable_text",
        "network_consent",
        "url_import_requires_consent_and_public_http_boundary",
        "url_import_extracts_bounded_readable_html_text",
    ]:
        if marker not in sources:
            failures.append(f"missing URL source boundary marker: {marker}")

    if "add_source_url" not in lib:
        failures.append("URL import command is not registered in Tauri handler")
    if "addSourceUrl" not in tauri:
        failures.append("frontend Tauri wrapper does not expose URL import")
    if "addSourceUrl" not in store:
        failures.append("source store does not expose URL import")
    for marker in [
        "URL text fetch",
        "Allow this one web fetch",
        "No crawling, credentials, localhost, intranet hosts",
    ]:
        if marker not in ui:
            failures.append(f"source panel missing URL disclosure/control marker: {marker}")

    if not receipt_path.exists():
        failures.append(f"missing URL import receipt: {receipt_path.relative_to(repo)}")
    else:
        try:
            receipt = json.loads(receipt_path.read_text())
        except Exception as exc:
            failures.append(f"invalid URL import receipt JSON: {exc}")
            receipt = {}
        if receipt.get("schema") != "UrlImportReceiptV1":
            failures.append("URL import receipt schema mismatch")
        if not receipt.get("network_consent_required"):
            failures.append("URL import receipt does not require explicit network consent")
        if not receipt.get("private_network_block_active"):
            failures.append("URL import receipt does not mark private network blocking active")
        if receipt.get("support") != "supported_degraded":
            failures.append("URL import receipt support is not supported_degraded")

    print(json.dumps({"ok": not failures, "failures": failures}, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
