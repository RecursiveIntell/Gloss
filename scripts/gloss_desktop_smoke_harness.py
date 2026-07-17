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
import os
import shutil
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


def _run_probe(repo: Path, command: list[str]) -> dict:
    if shutil.which(command[0]) is None:
        return {
            "command": " ".join(command),
            "available": False,
            "exit_code": None,
            "stdout_tail": "",
            "stderr_tail": f"{command[0]} not found on PATH",
        }
    result = _run(repo, command)
    result["available"] = result["exit_code"] == 0
    return result


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


def _current_run(repo: Path) -> str | None:
    path = repo / "docs/codex-runs/CURRENT_RUN.md"
    if not path.exists():
        return None
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        if line.startswith("Current run:"):
            return line.split(":", 1)[1].strip().strip("`")
    return None


def _default_receipt(repo: Path) -> Path | None:
    run_id = _current_run(repo)
    if not run_id:
        return None
    return repo / "docs" / "codex-runs" / run_id / "LIVE_DESKTOP_SMOKE_RECEIPT.json"


def _capability_detection(repo: Path, live_receipt: Path | None) -> dict:
    display = {
        "DISPLAY": os.environ.get("DISPLAY") or "",
        "WAYLAND_DISPLAY": os.environ.get("WAYLAND_DISPLAY") or "",
        "XDG_SESSION_TYPE": os.environ.get("XDG_SESSION_TYPE") or "",
    }
    probe_commands = [
        ["cargo", "--version"],
        ["npm", "--version"],
        ["npx", "tauri", "--version"],
        ["pkg-config", "--modversion", "webkit2gtk-4.1"],
        ["pkg-config", "--modversion", "webkit2gtk-4.0"],
        ["pkg-config", "--modversion", "javascriptcoregtk-4.1"],
    ]
    probes = [_run_probe(repo, command) for command in probe_commands]
    display_available = bool(display["DISPLAY"] or display["WAYLAND_DISPLAY"])
    has_cargo = any(p["command"].startswith("cargo ") and p["available"] for p in probes)
    has_npm = any(p["command"].startswith("npm ") and p["available"] for p in probes)
    has_tauri = any(p["command"].startswith("npx tauri ") and p["available"] for p in probes)
    has_webkit = any("webkit2gtk" in p["command"] and p["available"] for p in probes)
    driver_candidates = [
        "tests/e2e/desktop-smoke.spec.ts",
        "tests/desktop_smoke.rs",
        "scripts/live_desktop_smoke.py",
        "scripts/live_desktop_smoke_harness.py",
    ]
    existing_drivers = [candidate for candidate in driver_candidates if (repo / candidate).exists()]
    return {
        "schema": "GlossDesktopSmokeCapabilityDetectionV1",
        "display": display,
        "display_available": display_available,
        "cargo_available": has_cargo,
        "npm_available": has_npm,
        "tauri_cli_available": has_tauri,
        "webkitgtk_available": has_webkit,
        "live_receipt_supplied": live_receipt is not None,
        "active_live_gui_driver_present": bool(existing_drivers),
        "live_gui_driver_candidates_checked": driver_candidates,
        "live_gui_drivers_found": existing_drivers,
        "can_attempt_live_gui_smoke": all(
            [display_available, has_cargo, has_npm, has_tauri, has_webkit, bool(existing_drivers)]
        ),
        "probes": probes,
    }


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
    if receipt is None:
        receipt = _default_receipt(repo)

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
    capability_detection = _capability_detection(repo, live_receipt)
    blocked_reasons: list[str] = []
    if not live_ok:
        blocked_reasons.extend(live_failures)
        if not capability_detection["active_live_gui_driver_present"]:
            blocked_reasons.append("no active automated live GUI smoke driver found in current repo")
        if not capability_detection["display_available"]:
            blocked_reasons.append("no DISPLAY or WAYLAND_DISPLAY available for GUI launch")
        if not capability_detection["webkitgtk_available"]:
            blocked_reasons.append("webkitgtk pkg-config dependency not detected")
        if not capability_detection["tauri_cli_available"]:
            blocked_reasons.append("Tauri CLI not detected through npx tauri --version")

    result = {
        "schema": "GlossP35DesktopSmokeHarnessV1",
        "status": "fail" if failures else "pass",
        "repo": str(repo),
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "mode": "scripted_runtime_contract" if not args.require_live else "release_live_required",
        "scripted_contract_exercised": not args.skip_scripted,
        "live_desktop_exercised": live_ok,
        "release_grade": live_ok and not scripted_failures,
        "release_blocker": not live_ok or bool(scripted_failures),
        "release_decision": "blocked_live_desktop_smoke_missing"
        if not live_ok
        else "live_desktop_smoke_receipt_present",
        "exit_code_policy": "scripted failures fail; live failures fail only with --require-live",
        "failures": failures,
        "blocked_reasons": blocked_reasons,
        "capability_detection": capability_detection,
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
