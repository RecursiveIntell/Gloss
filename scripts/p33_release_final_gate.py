#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

RUN_ID = "GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519"


def read_json(path: Path) -> tuple[dict, str | None]:
    try:
        return json.loads(path.read_text(encoding="utf-8")), None
    except Exception as exc:
        return {}, str(exc)


def run_gate(cmd: list[str], repo: Path) -> tuple[int, str]:
    proc = subprocess.run(cmd, cwd=repo, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    return proc.returncode, proc.stdout


def main() -> int:
    parser = argparse.ArgumentParser(description="P33 final release gate.")
    parser.add_argument("--repo", default=".")
    parser.add_argument("--run-id", default=RUN_ID)
    args = parser.parse_args()
    repo = Path(args.repo).resolve()
    run_dir = repo / "docs/codex-runs" / args.run_id
    findings: list[dict[str, object]] = []

    receipt_path = run_dir / "FINAL_RECEIPT.json"
    receipt, err = read_json(receipt_path)
    if err:
        findings.append({"severity": "error", "code": "invalid-final-receipt", "path": str(receipt_path.relative_to(repo)), "detail": err})

    if receipt.get("schema") != "GlossP33FinalReceiptV1":
        findings.append({"severity": "error", "code": "final-receipt-schema-mismatch", "value": receipt.get("schema")})
    if receipt.get("run_id") != args.run_id:
        findings.append({"severity": "error", "code": "final-receipt-run-mismatch", "value": receipt.get("run_id")})

    required_artifacts = [
        "COMMANDS_RUN.md",
        "CHANGED_FILES.txt",
        "VALIDATION_RESULTS.md",
        "DESKTOP_SMOKE_RESULTS.md",
        "FINAL_AUDITOR_HANDOFF.md",
        "FINAL_RECEIPT.json",
    ]
    for name in required_artifacts:
        if not (run_dir / name).exists():
            findings.append({"severity": "error", "code": "missing-final-artifact", "path": str((run_dir / name).relative_to(repo))})

    desktop_receipt = run_dir / "desktop_smoke/final_desktop_smoke.json"
    desktop_rc, desktop_out = run_gate(
        [
            sys.executable,
            "scripts/p33_desktop_smoke_gate.py",
            "--repo",
            str(repo),
            "--receipt",
            str(desktop_receipt.relative_to(repo)),
        ],
        repo,
    )
    if desktop_rc != 0:
        findings.append({"severity": "error", "code": "desktop-smoke-gate-failed", "detail": desktop_out[-3000:]})

    validation = receipt.get("validation") or {}
    release_ready = bool(receipt.get("release_ready"))
    if release_ready:
        required_passes = [
            "p33_release_preflight",
            "p33_current_run_gate",
            "p33_sm_tq_settings_gate",
            "p33_gui_asset_gate",
            "npm_run_build",
            "cargo_fmt_check",
            "cargo_test_default",
            "cargo_test_semantic_memory_backend",
            "cargo_test_semantic_memory_turbo_quant",
            "desktop_smoke_gate",
            "fresh_unzip_replay",
        ]
        for key in required_passes:
            if validation.get(key) not in {"passed", "pass", True, "passed_with_reference_warnings"}:
                findings.append({"severity": "error", "code": "release-ready-without-required-pass", "detail": f"{key}={validation.get(key)!r}"})
        for rel in [
            "package_replay/fresh_unzip_replay.json",
            "package_replay/archive_manifest.json",
        ]:
            if not (run_dir / rel).exists():
                findings.append({"severity": "error", "code": "missing-package-replay-proof", "path": str((run_dir / rel).relative_to(repo))})
    else:
        blockers = receipt.get("blockers") or []
        if not blockers:
            findings.append({"severity": "error", "code": "release-not-ready-without-blockers"})
        findings.append({"severity": "error", "code": "release-not-ready", "detail": receipt.get("release_decision")})

    errors = [finding for finding in findings if finding.get("severity") == "error"]
    result = {
        "ok": not errors,
        "run_id": args.run_id,
        "release_ready": release_ready and not errors,
        "error_count": len(errors),
        "finding_count": len(findings),
        "findings": findings,
    }
    print(json.dumps(result, indent=2))
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
