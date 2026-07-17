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
    types = text(repo / "src/lib/types.ts")
    app = text(repo / "src/App.tsx")
    receipt_path = (
        repo
        / "docs/codex-runs"
        / (current_run(repo) or "__missing__")
        / "IMPORT_PERFORMANCE_RECEIPT.json"
    )

    for marker in [
        "ImportBatchPerformanceReceiptV1",
        "import_batch_performance_receipt",
        "scan_ms",
        "source_create_ms",
        "ingestion_ms",
        "index_save_ms",
        "found_per_second",
        "created_per_second",
        "ingested_ready_per_second",
        "import_batch_timing_receipt_records_throughput",
    ]:
        if marker not in sources:
            failures.append(f"missing import performance marker: {marker}")

    for marker in [
        "ImportBatchPerformanceReceipt",
        "performance?: ImportBatchPerformanceReceipt | null",
        'schema?: "ImportBatchReceiptV1"',
    ]:
        if marker not in types:
            failures.append(f"missing frontend import performance type marker: {marker}")
    for marker in ["payload.performance", "perf.elapsed_ms"]:
        if marker not in app:
            failures.append(f"missing frontend import performance display marker: {marker}")

    if not receipt_path.exists():
        failures.append(f"missing import performance receipt: {receipt_path.relative_to(repo)}")
    else:
        try:
            receipt = json.loads(receipt_path.read_text())
        except Exception as exc:
            failures.append(f"invalid import performance receipt JSON: {exc}")
            receipt = {}
        if receipt.get("schema") != "ImportPerformanceImplementationReceiptV1":
            failures.append("import performance receipt schema mismatch")
        if not receipt.get("import_performance_receipts_active"):
            failures.append("import performance receipt does not mark active implementation")
        if "cargo test --manifest-path src-tauri/Cargo.toml import_batch_timing_receipt -- --nocapture" not in receipt.get("tests", []):
            failures.append("import performance receipt missing focused test command")

    print(json.dumps({"ok": not failures, "failures": failures}, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
