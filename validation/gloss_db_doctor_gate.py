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

    doctor = text(repo / "src-tauri/src/db/doctor.rs")
    db_mod = text(repo / "src-tauri/src/db/mod.rs")
    commands = text(repo / "src-tauri/src/commands/notebooks.rs")
    lib = text(repo / "src-tauri/src/lib.rs")
    types = text(repo / "src/lib/types.ts")
    tauri = text(repo / "src/lib/tauri.ts")
    settings_ui = text(repo / "src/components/settings/SettingsDialog/index.tsx")
    frontend_contract_tests = text(repo / "scripts/run_frontend_contract_tests.mjs")
    receipt_path = (
        repo
        / "docs/codex-runs"
        / (current_run(repo) or "__missing__")
        / "DB_DOCTOR_RECEIPT.json"
    )

    required_markers = [
        "DbDoctorReceiptV1",
        "source_count_mismatch",
        "orphan_source_processing_state",
        "orphan_projection_status",
        "orphan_semantic_memory_links",
        "failed_import_sources",
        "quarantined_failed_import_sources",
        "quarantined_failed_import",
        "db_doctor_repair",
        "supersedes_receipt_id",
        "invalidated_by_receipt_id",
        "db_doctor_detects_orphans_without_repairing_in_check_mode",
        "db_doctor_repairs_orphans_and_supersedes_prior_receipt",
        "db_doctor_quarantines_failed_imports_without_removing_retry_state",
    ]
    for marker in required_markers:
        if marker not in doctor:
            failures.append(f"missing DB doctor marker: {marker}")

    if "pub mod doctor;" not in db_mod:
        failures.append("db doctor module is not exported")
    for marker in [
        "run_database_doctor",
        "inspect_queue_for_doctor",
        "queue_jobs_checked",
        "stale_queue_jobs",
        "repaired_stale_queue_jobs",
        "db_doctor_cancels_stale_queue_jobs_for_missing_sources",
    ]:
        if marker not in commands:
            failures.append(f"missing DB doctor queue marker: {marker}")
    if "run_database_doctor" not in lib:
        failures.append("run_database_doctor Tauri command is not registered")
    if "DbDoctorReceipt" not in types or "runDatabaseDoctor" not in tauri:
        failures.append("frontend DB doctor types/wrapper missing")
    for marker in [
        "Database doctor",
        "handleRunDatabaseDoctor",
        "dbDoctorReceipt",
        "handleCheckDatabaseDoctor",
        "handleRepairDatabaseDoctor",
        "handleRunDatabaseDoctor(false)",
        "handleRunDatabaseDoctor(true)",
        "repaired_source_count_mismatches",
        "repaired_orphan_rows",
        "failed_import_sources",
        "quarantined_failed_import_sources",
        "queue_jobs_checked",
        "repaired_stale_queue_jobs",
    ]:
        if marker not in settings_ui:
            failures.append(f"DB doctor UI marker missing: {marker}")
    for marker in [
        "DB doctor UI can run check and repair",
        "handleRunDatabaseDoctor",
        "handleCheckDatabaseDoctor",
        "handleRepairDatabaseDoctor",
        "quarantined_failed_import_sources",
        "repaired_stale_queue_jobs",
    ]:
        if marker not in frontend_contract_tests:
            failures.append(f"DB doctor frontend contract marker missing: {marker}")

    if not receipt_path.exists():
        failures.append(f"missing DB doctor receipt: {receipt_path.relative_to(repo)}")
    else:
        try:
            receipt = json.loads(receipt_path.read_text())
        except Exception as exc:
            failures.append(f"invalid DB doctor receipt JSON: {exc}")
            receipt = {}
        if receipt.get("schema") != "DbDoctorImplementationReceiptV1":
            failures.append("DB doctor implementation receipt schema mismatch")
        if not receipt.get("db_doctor_active"):
            failures.append("DB doctor receipt does not mark active implementation")
        if "cargo test --manifest-path src-tauri/Cargo.toml db_doctor -- --nocapture" not in receipt.get("tests", []):
            failures.append("DB doctor receipt missing focused test command")
        if "npm test" not in receipt.get("tests", []):
            failures.append("DB doctor receipt missing frontend contract test command")

    print(json.dumps({"ok": not failures, "failures": failures}, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
