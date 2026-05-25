#!/usr/bin/env python3
"""Desktop smoke contract harness for current Gloss runtime truth.

This script is deliberately conservative: the default mode proves only the
scripted source/runtime contracts that can be checked in CI/headless shells.
Release-grade desktop coverage still requires a live GUI receipt or
``--require-live`` in an environment that can exercise the Tauri app.
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path


def _run(repo: Path, command: list[str]) -> dict:
    completed = subprocess.run(
        command,
        cwd=repo,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    return {
        "command": " ".join(command),
        "exit_code": completed.returncode,
        "stdout_tail": completed.stdout[-4000:],
        "stderr_tail": completed.stderr[-4000:],
    }


def _read_json(path: Path) -> dict:
    try:
        return json.loads(path.read_text())
    except Exception as exc:  # noqa: BLE001 - receipt diagnostics must preserve parse cause.
        return {"status": "invalid", "error": str(exc)}


def _validate_live_receipt(path: Path | None) -> tuple[bool, list[str], dict | None]:
    if path is None:
        return False, ["no live desktop receipt supplied"], None
    if not path.exists():
        return False, [f"live desktop receipt missing: {path}"], None
    data = _read_json(path)
    failures: list[str] = []
    if data.get("status") != "pass":
        failures.append("live desktop receipt status is not pass")
    if data.get("live_desktop_exercised") is not True:
        failures.append("live desktop receipt does not assert live_desktop_exercised=true")
    if data.get("source_scope_widened") is True:
        failures.append("live desktop receipt reports source_scope_widened=true")
    if data.get("hidden_fallback") is True:
        failures.append("live desktop receipt reports hidden_fallback=true")
    if data.get("raw_uuid_flood") is True:
        failures.append("live desktop receipt reports raw_uuid_flood=true")
    return not failures, failures, data


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default=".")
    parser.add_argument("--receipt")
    parser.add_argument("--live-receipt")
    parser.add_argument("--require-live", action="store_true")
    parser.add_argument(
        "--skip-scripted",
        action="store_true",
        help="Only validate a supplied live receipt. Intended for manual release replay.",
    )
    args = parser.parse_args()

    repo = Path(args.repo).resolve()
    receipt = Path(args.receipt).resolve() if args.receipt else None
    live_receipt = Path(args.live_receipt).resolve() if args.live_receipt else None

    scripted_commands = [
        ["python3", "scripts/gloss_current_run_truth_gate.py", "--repo", str(repo)],
        ["python3", "scripts/gloss_static_runtime_truth_gate.py", "--repo", str(repo)],
        ["python3", "scripts/gloss_evidence_ui_gate.py", "--repo", str(repo)],
        ["python3", "scripts/gloss_validator_path_gate.py", "--repo", str(repo)],
        ["python3", "scripts/gloss_receipt_integrity_gate.py", "--repo", str(repo)],
        ["python3", "scripts/gloss_feature_matrix_gate.py", "--repo", str(repo)],
        [
            "python3",
            "scripts/gloss_runtime_log_gate.py",
            "--log",
            "fixtures/runtime_bad_missing_notebook_context_length.log",
            "--expect",
            "fail",
        ],
    ]

    command_results: list[dict] = []
    if not args.skip_scripted:
        command_results = [_run(repo, command) for command in scripted_commands]

    scripted_failures = [
        f"{result['command']} exited {result['exit_code']}"
        for result in command_results
        if result["exit_code"] != 0
    ]
    live_ok, live_failures, live_data = _validate_live_receipt(live_receipt)
    live_required_failures = live_failures if args.require_live else []
    failures = scripted_failures + live_required_failures

    result = {
        "schema": "GlossP35DesktopSmokeHarnessV1",
        "status": "fail" if failures else "pass",
        "repo": str(repo),
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "mode": "scripted_runtime_contract" if not args.require_live else "release_live_required",
        "scripted_contract_exercised": not args.skip_scripted,
        "live_desktop_exercised": live_ok,
        "release_grade": live_ok and not scripted_failures,
        "release_decision": "blocked_live_desktop_smoke_missing"
        if not live_ok
        else "live_desktop_smoke_receipt_present",
        "exit_code_policy": "scripted failures fail; live failures fail only with --require-live",
        "failures": failures,
        "live_receipt_failures": live_failures,
        "required_assertions": [
            "folder import rejects missing or superseded notebooks",
            "retrieval reports requested and effective backend truthfully",
            "fallback/degradation is visible per answer",
            "citations are valid or reason-coded when filtered",
            "normal evidence UI does not flood raw UUIDs",
            "live GUI import/query/delete/restart flow is release-proven only by live receipt",
        ],
        "commands": command_results,
        "live_receipt": live_data,
    }

    if receipt:
        receipt.parent.mkdir(parents=True, exist_ok=True)
        receipt.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
