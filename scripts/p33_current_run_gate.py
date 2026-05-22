#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

DEFAULT_RUN_ID = "GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519"


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace") if path.exists() else ""


def main() -> int:
    parser = argparse.ArgumentParser(description="Validate active P33 run truth.")
    parser.add_argument("--repo", default=".")
    parser.add_argument("--run-id", default=DEFAULT_RUN_ID)
    args = parser.parse_args()
    repo = Path(args.repo).resolve()
    findings: list[dict[str, str]] = []

    current_run = read(repo / "docs/codex-runs/CURRENT_RUN.md")
    if args.run_id not in current_run:
        findings.append(
            {
                "severity": "error",
                "code": "current-run-mismatch",
                "path": "docs/codex-runs/CURRENT_RUN.md",
                "detail": f"expected active run {args.run_id}",
            }
        )

    run_dir = repo / "docs/codex-runs" / args.run_id
    if not run_dir.exists():
        findings.append(
            {
                "severity": "error",
                "code": "missing-run-directory",
                "path": str(run_dir.relative_to(repo)),
            }
        )

    active_files = [
        repo / "AGENTS.md",
        repo / "README.md",
        repo / "package.json",
        repo / "docs/codex-runs/CURRENT_RUN.md",
        repo / "scripts/check_release_eligibility_current.py",
        repo / "scripts/gloss_button_up_gate.py",
    ]
    stale_patterns = [r"\bP30\b", r"GLOSS_BUTTON_UP_20260519"]
    for path in active_files:
        text = read(path)
        rel = str(path.relative_to(repo))
        for pattern in stale_patterns:
            if re.search(pattern, text) and args.run_id not in text:
                findings.append(
                    {
                        "severity": "error",
                        "code": "stale-active-run-reference",
                        "path": rel,
                        "detail": f"matched {pattern}",
                    }
                )

    required = [
        "FINAL_RECEIPT.json",
        "FINAL_AUDITOR_HANDOFF.md",
        "COMMANDS_RUN.md",
        "CHANGED_FILES.txt",
        "VALIDATION_RESULTS.md",
    ]
    for name in required:
        if not (run_dir / name).exists():
            findings.append(
                {
                    "severity": "error",
                    "code": "missing-run-artifact",
                    "path": str((run_dir / name).relative_to(repo)),
                }
            )

    errors = [finding for finding in findings if finding.get("severity") == "error"]
    result = {
        "ok": not errors,
        "run_id": args.run_id,
        "error_count": len(errors),
        "finding_count": len(findings),
        "findings": findings,
    }
    print(json.dumps(result, indent=2))
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
