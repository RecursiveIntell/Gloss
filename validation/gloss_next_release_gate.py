#!/usr/bin/env python3
"""Aggregate release gate for GLOSS_RELEASE_PROOF_EMBEDDING_UNIFICATION_CLEANUP_20260525.

This script is copied into Gloss/validation/ by the next Codex pass.
It intentionally delegates to existing repo gates plus the new pass gates.
It writes docs/codex-runs/<run-id>/NEXT_RELEASE_GATE_RESULTS.json.
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import List, Dict, Any


def current_run(repo: Path) -> str | None:
    try:
        text = (repo / "docs/codex-runs/CURRENT_RUN.md").read_text(errors="ignore")
        match = re.search(r"Current run:\s*`?([^`\n]+)`?", text)
        return match.group(1).strip() if match else None
    except Exception:
        return None


def run_cmd(repo: Path, args: List[str]) -> Dict[str, Any]:
    proc = subprocess.run(
        args,
        cwd=repo,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    return {
        "cmd": args,
        "returncode": proc.returncode,
        "stdout_tail": proc.stdout[-4000:],
        "stderr_tail": proc.stderr[-4000:],
        "passed": proc.returncode == 0,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default=".")
    parser.add_argument("--run-id", default="CURRENT", help="Run ID or 'CURRENT' to use CURRENT_RUN.md")
    parser.add_argument("--skip-npm", action="store_true", help="Skip node frontend contract tests only with explicit reason in final receipt.")
    args = parser.parse_args()

    repo = Path(args.repo).resolve()
    run_id = current_run(repo) if args.run_id == "CURRENT" else args.run_id
    run_dir = repo / "docs" / "codex-runs" / (run_id or "__missing__")
    run_dir.mkdir(parents=True, exist_ok=True)

    commands: List[List[str]] = [
        [sys.executable, "validation/gloss_stale_pass_surface_gate.py", "--repo", "."],
        [sys.executable, "validation/gloss_embedding_provider_gate.py", "--repo", "."],
        [sys.executable, "validation/gloss_live_receipt_gate.py", "--repo", ".", "--run-id", (run_id or "__missing__")],
        [sys.executable, "scripts/gloss_current_run_truth_gate.py", "--repo", "."],
        [sys.executable, "scripts/gloss_receipt_integrity_gate.py", "--repo", "."],
        [sys.executable, "scripts/gloss_feature_matrix_gate.py", "--repo", "."],
        [sys.executable, "scripts/gloss_static_runtime_truth_gate.py", "--repo", "."],
        [sys.executable, "scripts/audit_tauri_security.py", "."],
        [sys.executable, "scripts/audit_ui_disclosure_a11y.py", "."],
        [sys.executable, "scripts/gloss_evidence_ui_gate.py", "--repo", "."],
        [sys.executable, "scripts/gloss_retrieval_gate.py", "--repo", "."],
        [sys.executable, "scripts/gloss_validator_path_gate.py", "--repo", "."],
    ]
    if not args.skip_npm:
        commands.insert(3, ["node", "scripts/run_frontend_contract_tests.mjs"])

    results = [run_cmd(repo, cmd) for cmd in commands]
    payload = {
        "schema": "GlossNextReleaseGateResultsV1",
        "run_id": run_id,
        "recorded_utc": datetime.now(timezone.utc).isoformat(),
        "repo": str(repo),
        "passed": all(r["passed"] for r in results),
        "commands": results,
    }
    out = run_dir / "NEXT_RELEASE_GATE_RESULTS.json"
    out.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    if payload["passed"]:
        print(f"PASS: wrote {out}")
        return 0
    print(f"FAIL: wrote {out}")
    for result in results:
        if not result["passed"]:
            print("FAILED:", " ".join(result["cmd"]))
            if result["stdout_tail"]:
                print(result["stdout_tail"])
            if result["stderr_tail"]:
                print(result["stderr_tail"], file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
