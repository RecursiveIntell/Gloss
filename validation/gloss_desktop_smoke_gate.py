#!/usr/bin/env python3
"""Validate the current-run desktop smoke receipt without overclaiming release proof."""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path


def _current_run(repo: Path) -> str | None:
    path = repo / "docs/codex-runs/CURRENT_RUN.md"
    text = path.read_text(encoding="utf-8", errors="replace") if path.exists() else ""
    match = re.search(r"Current run:\s*`?([^`\n]+)`?", text)
    return match.group(1).strip() if match else None


def _load(path: Path, failures: list[str]) -> dict:
    if not path.exists():
        failures.append(f"missing desktop smoke receipt: {path}")
        return {}
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except Exception as exc:  # noqa: BLE001 - gate must preserve parse diagnostics.
        failures.append(f"invalid desktop smoke receipt JSON: {exc}")
        return {}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default=".")
    parser.add_argument("--receipt")
    args = parser.parse_args()

    repo = Path(args.repo).resolve()
    failures: list[str] = []
    warnings: list[str] = []
    run_id = _current_run(repo)
    if not run_id:
        failures.append("cannot determine current run id")
        receipt_path = None
    else:
        receipt_path = (
            Path(args.receipt).resolve()
            if args.receipt
            else repo / "docs" / "codex-runs" / run_id / "LIVE_DESKTOP_SMOKE_RECEIPT.json"
        )

    receipt = _load(receipt_path, failures) if receipt_path else {}
    if receipt:
        if receipt.get("schema") != "GlossP35DesktopSmokeHarnessV1":
            failures.append(f"unexpected desktop smoke receipt schema: {receipt.get('schema')!r}")
        if receipt.get("status") != "pass":
            failures.append("desktop smoke scripted contract did not pass")
        if receipt.get("scripted_contract_exercised") is not True:
            failures.append("scripted desktop contract was not exercised")
        if receipt.get("live_desktop_exercised") is True and receipt.get("release_grade") is not True:
            failures.append("live desktop receipt cannot be live=true while release_grade is not true")
        if receipt.get("release_grade") is True and receipt.get("live_desktop_exercised") is not True:
            failures.append("release_grade desktop smoke requires live_desktop_exercised=true")
        if receipt.get("release_grade") is True and receipt.get("failures"):
            failures.append("release_grade desktop smoke cannot contain failures")

        capability = receipt.get("capability_detection")
        if not isinstance(capability, dict):
            failures.append("desktop smoke receipt missing capability_detection")
        else:
            if capability.get("schema") != "GlossDesktopSmokeCapabilityDetectionV1":
                failures.append("desktop smoke capability_detection schema mismatch")
            for key in [
                "display_available",
                "cargo_available",
                "npm_available",
                "tauri_cli_available",
                "webkitgtk_available",
                "active_live_gui_driver_present",
                "can_attempt_live_gui_smoke",
            ]:
                if key not in capability:
                    failures.append(f"desktop smoke capability_detection missing {key}")
            if not isinstance(capability.get("probes"), list) or not capability.get("probes"):
                failures.append("desktop smoke capability probes missing")

        if receipt.get("live_desktop_exercised") is not True:
            if receipt.get("release_decision") != "blocked_live_desktop_smoke_missing":
                failures.append("non-live desktop receipt must carry blocked_live_desktop_smoke_missing")
            if receipt.get("release_blocker") is not True:
                failures.append("non-live desktop receipt must mark release_blocker=true")
            blocked_reasons = receipt.get("blocked_reasons")
            if not isinstance(blocked_reasons, list) or not blocked_reasons:
                failures.append("non-live desktop receipt must include blocked_reasons")
            warnings.append("desktop GUI smoke is still not release-grade; receipt is an honest blocker")

        command_failures = [
            command
            for command in receipt.get("commands", [])
            if isinstance(command, dict) and command.get("exit_code") != 0
        ]
        if command_failures:
            failures.append(f"{len(command_failures)} scripted desktop smoke command(s) failed")

    out = {
        "ok": not failures,
        "run_id": run_id,
        "receipt": str(receipt_path) if receipt_path else None,
        "release_grade": bool(receipt.get("release_grade")) if receipt else False,
        "live_desktop_exercised": bool(receipt.get("live_desktop_exercised")) if receipt else False,
        "failures": failures,
        "warnings": warnings,
    }
    print(json.dumps(out, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
