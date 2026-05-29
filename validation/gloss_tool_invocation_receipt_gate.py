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
    helper = text(repo / "src-tauri/src/tool_invocation.rs")
    jobs = text(repo / "src-tauri/src/jobs/mod.rs")
    settings = text(repo / "src-tauri/src/commands/settings.rs")
    types = text(repo / "src/lib/types.ts")
    receipt = (
        repo
        / "docs/codex-runs"
        / (current_run(repo) or "__missing__")
        / "TOOL_INVOCATION_RECEIPT.json"
    )

    checks = {
        "ToolInvocationReceiptV1 helper": "ToolInvocationReceiptV1" in helper,
        "redacted args": "args_redacted" in helper,
        "stderr digest": "stderr_sha256" in helper and "Sha256" in helper,
        "stderr preview redaction test": "stderr_preview_redacts_paths_and_secrets" in helper,
        "jobs use receipt helper": "run_tool_output_receipt" in jobs and "tool_invocation_receipts" in jobs,
        "settings use receipt helper": "ExternalToolAvailabilityReceipt" in settings and "run_tool_status_receipt" in settings,
        "frontend type": "ExternalToolAvailabilityReceipt" in types,
    }
    for name, ok in checks.items():
        if not ok:
            failures.append(f"missing {name}")

    direct_sites = []
    for path in [repo / "src-tauri/src/jobs/mod.rs", repo / "src-tauri/src/commands/settings.rs"]:
        body = text(path)
        if "tokio::process::Command::new" in body or "std::process::Command::new" in body:
            direct_sites.append(str(path.relative_to(repo)))
    if direct_sites:
        failures.append(f"external tool direct Command::new remains outside helper: {direct_sites}")

    if not receipt.exists():
        failures.append(f"missing tool invocation receipt: {receipt.relative_to(repo)}")
    else:
        try:
            data = json.loads(receipt.read_text())
        except Exception as exc:
            failures.append(f"invalid tool invocation receipt JSON: {exc}")
            data = {}
        if data.get("schema") != "ToolInvocationReceiptGateReceiptV1":
            failures.append("tool invocation receipt schema mismatch")
        if not data.get("tool_invocation_receipts_active"):
            failures.append("tool invocation gate receipt does not mark receipts active")

    print(json.dumps({"ok": not failures, "failures": failures}, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
