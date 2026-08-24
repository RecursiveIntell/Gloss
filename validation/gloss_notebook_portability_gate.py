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

    portable = text(repo / "src-tauri/src/db/portable.rs")
    db_mod = text(repo / "src-tauri/src/db/mod.rs")
    notebooks = text(repo / "src-tauri/src/commands/notebooks.rs")
    lib = text(repo / "src-tauri/src/lib.rs")
    types = text(repo / "src/lib/types.ts")
    tauri = text(repo / "src/lib/tauri.ts")
    sidebar = text(repo / "src/components/notebooks/NotebookSidebar.tsx")
    capabilities = text(repo / "src-tauri/capabilities/default.json")
    frontend_contract_tests = text(repo / "scripts/run_frontend_contract_tests.mjs")
    contract = text(repo / "schemas/tauri-contract-v1.json")
    receipt_path = (
        repo
        / "docs/codex-runs"
        / (current_run(repo) or "__missing__")
        / "NOTEBOOK_PORTABILITY_RECEIPT.json"
    )

    required_markers = [
        "NotebookPortableManifestV1",
        "NotebookExportReceiptV1",
        "NotebookImportReceiptV1",
        "manifest_digest",
        "validate_notebook_package",
        "validate_notebook_archive",
        "export_notebook_archive",
        "import_notebook_archive",
        "MAX_PORTABLE_ARCHIVE_FILES",
        "MAX_PORTABLE_ARCHIVE_UNPACKED_BYTES",
        "digest_manifest_entries",
        "validate_relative_package_path",
        "notebook_export_import_roundtrip_validates_hashes",
        "notebook_package_validation_rejects_tampering",
        "notebook_archive_export_import_replay_validates_hashes",
        "notebook_archive_validation_rejects_tampering",
    ]
    for marker in required_markers:
        if marker not in portable:
            failures.append(f"missing notebook portability marker: {marker}")

    if "pub mod portable;" not in db_mod:
        failures.append("portable DB module is not exported")
    for command in [
        "export_notebook",
        "export_notebook_archive",
        "validate_notebook_import_package",
        "validate_notebook_import_archive",
        "import_notebook",
        "import_notebook_archive",
    ]:
        if command not in notebooks or command not in lib:
            failures.append(f"notebook portability command not registered: {command}")
    for marker in [
        "NotebookPortableManifest",
        "NotebookExportReceipt",
        "NotebookImportReceipt",
    ]:
        if marker not in types:
            failures.append(f"frontend type missing: {marker}")
    for marker in [
        "exportNotebookArchive",
        "validateNotebookImportArchive",
        "importNotebookArchive",
    ]:
        if marker not in tauri:
            failures.append(f"frontend wrapper missing: {marker}")
    try:
        contract_commands = json.loads(contract).get("commands", {})
    except json.JSONDecodeError:
        contract_commands = {}
    for command in ["export_notebook", "validate_notebook_import_package", "import_notebook"]:
        entry = contract_commands.get(command, {})
        if not (
            entry.get("operator_only") is True
            and entry.get("wrapper") is None
            and entry.get("caller_count") == 0
        ):
            failures.append(f"directory-package command is not explicitly operator-only: {command}")
    for marker in [
        "handleExportNotebook",
        "handleImportNotebook",
        "validateNotebookImportArchive",
        "exportNotebookArchive",
        "importNotebookArchive",
        "portableDefaultName",
        ".glosspkg.tar.gz",
    ]:
        if marker not in sidebar:
            failures.append(f"notebook portability UI marker missing: {marker}")
    if sidebar.find("validateNotebookImportArchive") > sidebar.find("importNotebookArchive"):
        failures.append("notebook portability UI does not validate archive before import")
    if "dialog:allow-open" not in capabilities or "dialog:allow-save" not in capabilities:
        failures.append("notebook portability UI dialog permissions are incomplete")
    for marker in [
        "notebook portability UI validates before import",
        "notebook portability UI exposes export receipt path",
    ]:
        if marker not in frontend_contract_tests:
            failures.append(f"frontend notebook portability contract test missing: {marker}")

    if not receipt_path.exists():
        failures.append(f"missing notebook portability receipt: {receipt_path.relative_to(repo)}")
    else:
        try:
            receipt = json.loads(receipt_path.read_text())
        except Exception as exc:
            failures.append(f"invalid notebook portability receipt JSON: {exc}")
            receipt = {}
        if receipt.get("schema") != "NotebookPortabilityImplementationReceiptV1":
            failures.append("notebook portability implementation receipt schema mismatch")
        if not receipt.get("notebook_portability_active"):
            failures.append("notebook portability receipt does not mark active implementation")
        if "cargo test --manifest-path src-tauri/Cargo.toml portable -- --nocapture" not in receipt.get("tests", []):
            failures.append("notebook portability receipt missing focused test command")

    print(json.dumps({"ok": not failures, "failures": failures}, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
