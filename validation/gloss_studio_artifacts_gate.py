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

    studio = text(repo / "src-tauri/src/studio/mod.rs")
    commands = text(repo / "src-tauri/src/commands/studio.rs")
    command_mod = text(repo / "src-tauri/src/commands/mod.rs")
    lib = text(repo / "src-tauri/src/lib.rs")
    db = text(repo / "src-tauri/src/db/notebook_db/mod.rs")
    tauri = text(repo / "src/lib/tauri.ts")
    types = text(repo / "src/lib/types.ts")
    store = text(repo / "src/stores/studioStore.ts")
    panel = text(repo / "src/components/studio/StudioPanel.tsx")
    layout = text(repo / "src/components/layout/PanelLayout.tsx")
    contract = text(repo / "scripts/run_frontend_contract_tests.mjs")
    receipt_path = (
        repo
        / "docs/codex-runs"
        / (current_run(repo) or "__missing__")
        / "STUDIO_ARTIFACTS_RECEIPT.json"
    )

    for marker in [
        "StudioArtifactV1",
        "StudioArtifactReceiptV1",
        "deterministic_source_bound_template_v1",
        "StudioOutputKind::Report",
        "StudioOutputKind::Summary",
        "StudioOutputKind::Outline",
        "StudioOutputKind::Faq",
        "StudioOutputKind::Flashcards",
        "StudioOutputKind::Quiz",
        "StudioOutputKind::MindMap",
        "StudioOutputKind::Timeline",
        "StudioOutputKind::CompareTable",
        "StudioOutputKind::ActionPlan",
        "studio_artifacts_cover_required_output_types_with_citations",
        "explicit_studio_scope_rejects_missing_source_ids",
        "all_items_source_cited",
    ]:
        if marker not in studio:
            failures.append(f"missing Studio artifact marker: {marker}")

    for marker in [
        "generate_studio_output",
        "list_studio_outputs",
        "export_studio_output",
        "StudioExportReceipt",
        "StudioExportReceiptV1",
        "StudioExportPackageV1",
        "build_snippets",
        "generate_artifact",
        "StudioOutputView",
    ]:
        if marker not in commands:
            failures.append(f"missing Studio command marker: {marker}")

    if "pub mod studio;" not in command_mod:
        failures.append("commands::studio module is not exported")
    for command in [
        "commands::studio::list_studio_outputs",
        "commands::studio::generate_studio_output",
        "commands::studio::export_studio_output",
    ]:
        if command not in lib:
            failures.append(f"Studio Tauri command is not registered: {command}")
    for marker in [
        "insert_studio_output",
        "list_studio_outputs",
        "get_studio_output",
        "update_studio_output_file_path",
        "studio_output_from_row",
    ]:
        if marker not in db:
            failures.append(f"Notebook DB Studio persistence marker missing: {marker}")
    for marker in ["StudioOutput", "StudioExportReceipt", "listStudioOutputs", "generateStudioOutput", "exportStudioOutput"]:
        if marker not in tauri + types:
            failures.append(f"frontend Studio wrapper/type marker missing: {marker}")
    for marker in [
        "StudioPanel",
        "OUTPUT_TYPES",
        "generateOutput",
        "exportOutput",
        "Studio Output Ready",
        "Studio Export Written",
        "schema_validated",
        "all_items_source_cited",
    ]:
        if marker not in panel + store + layout:
            failures.append(f"Studio UI/export marker missing: {marker}")
    if "Studio UI exposes generation and export workflow" not in contract:
        failures.append("frontend contract test does not cover Studio UI/export workflow")

    if not receipt_path.exists():
        failures.append(f"missing Studio artifacts receipt: {receipt_path.relative_to(repo)}")
    else:
        try:
            receipt = json.loads(receipt_path.read_text())
        except Exception as exc:
            failures.append(f"invalid Studio artifacts receipt JSON: {exc}")
            receipt = {}
        if receipt.get("schema") != "StudioArtifactsImplementationReceiptV1":
            failures.append("Studio artifacts receipt schema mismatch")
        if not receipt.get("studio_artifacts_active"):
            failures.append("Studio artifacts receipt does not mark active implementation")
        expected = {
            "report",
            "summary",
            "outline",
            "faq",
            "flashcards",
            "quiz",
            "mind_map",
            "timeline",
            "compare_table",
            "action_plan",
        }
        if set(receipt.get("output_types", [])) != expected:
            failures.append("Studio artifacts receipt does not list the required output types")
        if "cargo test --manifest-path src-tauri/Cargo.toml studio -- --nocapture" not in receipt.get("tests", []):
            failures.append("Studio artifacts receipt missing focused test command")
        blockers = "\n".join(receipt.get("remaining_blockers", []))
        if "No dedicated Studio UI panel" in blockers:
            failures.append("Studio receipt still claims no dedicated Studio UI panel")
        if "export surface" in blockers or "artifact file writer" in blockers:
            failures.append("Studio receipt still claims export surface/file writer is missing")

    print(json.dumps({"ok": not failures, "failures": failures}, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
