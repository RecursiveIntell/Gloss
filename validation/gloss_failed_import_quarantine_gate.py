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

    sources = text(repo / "src-tauri/src/commands/sources/mod.rs")
    lib = text(repo / "src-tauri/src/lib.rs")
    types = text(repo / "src/lib/types.ts")
    tauri = text(repo / "src/lib/tauri.ts")
    store = text(repo / "src/stores/sourceStore.ts")
    panel = text(repo / "src/components/sources/SourcesPanel.tsx")
    contract = text(repo / "scripts/run_frontend_contract_tests.mjs")
    run_id = current_run(repo) or "__missing__"
    receipt_path = repo / "docs/codex-runs" / run_id / "FAILED_IMPORT_QUARANTINE_RECEIPT.json"

    for marker in [
        "FailedImportQuarantineReceipt",
        "FailedImportQuarantineReceiptV1",
        "quarantine_failed_imports",
        "delete_failed_imports",
        "failed_import_source_ids",
        "delete_source_ids_for_notebook",
        "quarantined_failed_import",
        "failed_import_quarantine_receipt_shape_is_stable",
    ]:
        if marker not in sources:
            failures.append(f"missing failed-import backend marker: {marker}")
    for marker in ["quarantine_failed_imports", "delete_failed_imports"]:
        if marker not in lib:
            failures.append(f"missing Tauri handler marker: {marker}")
    for marker in [
        "FailedImportQuarantineReceipt",
        "quarantineFailedImports",
        "deleteFailedImports",
    ]:
        if marker not in types + tauri + store:
            failures.append(f"missing frontend type/wrapper/store marker: {marker}")
    for marker in [
        "Failed imports",
        "Review",
        "Quarantine",
        "Delete Failed",
        'setStatusFilter("error")',
        "quarantineFailedImports(notebookId)",
        "deleteFailedImports(notebookId)",
    ]:
        if marker not in panel:
            failures.append(f"missing source panel failed-import marker: {marker}")
    for marker in [
        "failed import quarantine UI exposes review/quarantine/delete workflow",
        "quarantineFailedImports",
        "deleteFailedImports",
    ]:
        if marker not in contract:
            failures.append(f"missing frontend contract marker: {marker}")

    if not receipt_path.exists():
        failures.append(f"missing failed import quarantine receipt: {receipt_path.relative_to(repo)}")
    else:
        try:
            receipt = json.loads(receipt_path.read_text())
        except Exception as exc:
            failures.append(f"invalid failed import quarantine receipt JSON: {exc}")
            receipt = {}
        if receipt.get("schema") != "FailedImportQuarantineImplementationReceiptV1":
            failures.append("failed import quarantine receipt schema mismatch")
        if not receipt.get("ui_workflow_active"):
            failures.append("failed import quarantine receipt does not mark UI workflow active")
        if not receipt.get("backend_receipt_active"):
            failures.append("failed import quarantine receipt does not mark backend receipt active")

    print(json.dumps({"ok": not failures, "failures": failures}, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
