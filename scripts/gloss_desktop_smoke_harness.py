#!/usr/bin/env python3
"""Desktop smoke contract harness for current Gloss runtime truth.

This script is deliberately conservative: the default mode proves only the
scripted source/runtime contracts that can be checked in CI/headless shells.
Release-grade desktop coverage still requires a live GUI receipt or
``--require-live`` in an environment that can exercise the Tauri app.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

# A live receipt proves only the observations actually captured. These cases
# are the minimum desktop acceptance inventory, not an exhaustive product audit.
LIVE_SCHEMA = "GlossLiveDesktopSmokeReceiptV2"
REQUIRED_LIVE_CASES = (
    "startup_idle",
    "notebook_crud_restart",
    "folder_import_scope",
    "chat_no_retrieval",
    "chat_persistence_restart",
    "chat_cancel_and_retry",
    "notebook_switch_isolation",
    "retrieval_backend_and_degradation",
    "citation_evidence",
    "model_dropdown_and_prompt",
    "notes_persistence",
    "source_delete_restart",
)
SAFETY_FLAGS = ("source_scope_widened", "hidden_fallback", "raw_uuid_flood")


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _source_identity(repo: Path) -> dict:
    # Shared with the canonical release verifier. Do not invent a second
    # fingerprint convention for desktop evidence.
    script_dir = str(Path(__file__).resolve().parent)
    if script_dir not in sys.path:
        sys.path.insert(0, script_dir)
    from source_snapshot import capture_source_identity

    return capture_source_identity(repo)


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


def _validate_evidence(entry: object, root: Path, label: str) -> list[str]:
    if not isinstance(entry, dict):
        return [f"{label}: evidence descriptor must be an object"]
    relative = entry.get("path")
    if not isinstance(relative, str) or not relative.strip():
        return [f"{label}: evidence path missing"]
    candidate = root / relative
    if Path(relative).is_absolute() or not candidate.resolve().is_relative_to(root.resolve()):
        return [f"{label}: evidence path escapes receipt directory"]
    if candidate.is_symlink() or not candidate.is_file():
        return [f"{label}: evidence must be a regular file"]
    try:
        if candidate.stat().st_size == 0:
            return [f"{label}: evidence is empty"]
        if entry.get("sha256") != file_sha256(candidate):
            return [f"{label}: evidence digest mismatch"]
        if entry.get("kind") == "screenshot" and candidate.read_bytes()[:8] != b"\x89PNG\r\n\x1a\n":
            return [f"{label}: screenshot is not PNG"]
    except OSError as exc:
        return [f"{label}: evidence unreadable: {exc}"]
    return []


def _validate_live_receipt(path: Path | None, repo: Path | None = None) -> tuple[bool, list[str], dict | None]:
    if path is None:
        return False, ["no live desktop receipt supplied"], None
    if not path.exists():
        return False, [f"live desktop receipt missing: {path}"], None
    data = _read_json(path)
    failures: list[str] = []
    if not isinstance(data, dict):
        return False, ["live desktop receipt must be an object"], None
    if data.get("schema") != LIVE_SCHEMA:
        failures.append(f"live desktop receipt schema must be {LIVE_SCHEMA}")
    if data.get("status") != "pass":
        failures.append("live desktop receipt status is not pass")
    if data.get("live_desktop_exercised") is not True:
        failures.append("live desktop receipt does not assert live_desktop_exercised=true")
    for flag in SAFETY_FLAGS:
        if data.get(flag) is not False:
            failures.append(f"live desktop receipt must explicitly report {flag}=false")
    if data.get("runtime") != "native_tauri":
        failures.append("live desktop receipt must exercise native_tauri runtime")
    if not isinstance(data.get("isolated_data_root"), str) or not Path(data["isolated_data_root"]).is_absolute():
        failures.append("live desktop receipt must identify an absolute isolated_data_root")
    if not isinstance(data.get("run_id"), str) or not data["run_id"].strip():
        failures.append("live desktop receipt run_id missing")
    try:
        started = datetime.fromisoformat(data["started_at"])
        finished = datetime.fromisoformat(data["finished_at"])
        if started.tzinfo is None or finished.tzinfo is None or finished <= started:
            raise ValueError("invalid interval")
        if finished > datetime.now(timezone.utc):
            raise ValueError("future observation")
    except (KeyError, TypeError, ValueError):
        failures.append("live desktop receipt must contain a completed timezone-aware run interval")
    try:
        current_source = _source_identity(repo or Path(__file__).resolve().parents[1])
        if not current_source.get("worktree_clean"):
            failures.append("live desktop release replay requires a clean source snapshot")
        if data.get("source") != current_source:
            failures.append("live desktop receipt source does not match current source snapshot")
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as exc:
        failures.append(f"cannot establish current source snapshot: {exc}")

    build = data.get("build")
    if not isinstance(build, dict):
        failures.append("live desktop receipt build evidence missing")
    else:
        if build.get("source") != data.get("source") or build.get("exit_code") != 0 or type(build.get("exit_code")) is not int:
            failures.append("live desktop build must have succeeded on the same source snapshot")
        if not isinstance(build.get("command"), list) or not build["command"] or not all(isinstance(x, str) for x in build["command"]):
            failures.append("live desktop build command missing")
        failures.extend(_validate_evidence(build.get("log"), path.parent, "build log"))
        failures.extend(_validate_evidence(build.get("binary"), path.parent, "built executable"))

    cases = data.get("cases")
    if not isinstance(cases, list):
        failures.append("live desktop receipt required cases missing")
        cases = []
    seen: set[str] = set()
    for case in cases:
        if not isinstance(case, dict) or case.get("id") not in REQUIRED_LIVE_CASES:
            failures.append("live desktop receipt contains an invalid case")
            continue
        case_id = case["id"]
        if case_id in seen:
            failures.append(f"duplicate live desktop case: {case_id}")
        seen.add(case_id)
        if case.get("status") != "pass":
            failures.append(f"live desktop case {case_id} did not pass")
        if not isinstance(case.get("observation"), str) or not case["observation"].strip():
            failures.append(f"live desktop case {case_id} observation missing")
        evidence = case.get("evidence")
        if not isinstance(evidence, list) or not evidence:
            failures.append(f"live desktop case {case_id} evidence missing")
            continue
        kinds = {entry.get("kind") for entry in evidence
                 if isinstance(entry, dict) and isinstance(entry.get("kind"), str)}
        if not {"runtime_log", "screenshot"}.issubset(kinds):
            failures.append(f"live desktop case {case_id} needs runtime_log and screenshot evidence")
        for entry in evidence:
            failures.extend(_validate_evidence(entry, path.parent, case_id))
    for missing in sorted(set(REQUIRED_LIVE_CASES) - seen):
        failures.append(f"required live desktop case missing: {missing}")
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
        ["tauri-driver", "--version"],
    ]
    probes = [_run_probe(repo, command) for command in probe_commands]
    display_available = bool(display["DISPLAY"] or display["WAYLAND_DISPLAY"])
    has_cargo = any(p["command"].startswith("cargo ") and p["available"] for p in probes)
    has_npm = any(p["command"].startswith("npm ") and p["available"] for p in probes)
    has_tauri = any(p["command"].startswith("npx tauri ") and p["available"] for p in probes)
    has_webkit = any("webkit2gtk" in p["command"] and p["available"] for p in probes)
    has_driver = any(p["command"].startswith("tauri-driver ") and p["available"] for p in probes)
    has_native_driver = shutil.which("WebKitWebDriver") is not None
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
        "tauri_driver_available": has_driver,
        "native_webdriver_available": has_native_driver,
        "live_receipt_supplied": live_receipt is not None,
        "active_live_gui_driver_present": bool(existing_drivers),
        "live_gui_driver_candidates_checked": driver_candidates,
        "live_gui_drivers_found": existing_drivers,
        "can_attempt_live_gui_smoke": all(
            [display_available, has_cargo, has_npm, has_tauri, has_webkit, has_driver, has_native_driver, bool(existing_drivers)]
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
    live_ok, live_failures, live_data = _validate_live_receipt(live_receipt, repo)
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
        "release_grade": live_ok and not args.skip_scripted and not scripted_failures,
        "release_blocker": not live_ok or args.skip_scripted or bool(scripted_failures),
        "release_decision": "blocked_live_desktop_smoke_missing"
        if not live_ok
        else ("blocked_scripted_checks_skipped" if args.skip_scripted
              else "blocked_scripted_checks_failed" if scripted_failures
              else "live_desktop_smoke_receipt_validated"),
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
        "required_live_case_ids": list(REQUIRED_LIVE_CASES),
        "evidence_limit": "Digests establish artifact integrity and source binding, not runner authenticity or exhaustive production readiness.",
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
