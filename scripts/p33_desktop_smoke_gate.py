#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from pathlib import Path

RUN_ID = "GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519"


def main() -> int:
    parser = argparse.ArgumentParser(description="Validate P33 desktop RAG smoke receipt.")
    parser.add_argument("--repo", default=".")
    parser.add_argument(
        "--receipt",
        default=f"docs/codex-runs/{RUN_ID}/desktop_smoke/final_desktop_smoke.json",
    )
    args = parser.parse_args()
    repo = Path(args.repo).resolve()
    receipt_path = repo / args.receipt
    findings: list[dict[str, object]] = []

    if not receipt_path.exists():
        findings.append({"severity": "error", "code": "missing-desktop-smoke-receipt", "path": args.receipt})
        receipt = {}
    else:
        try:
            receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as exc:
            receipt = {}
            findings.append({"severity": "error", "code": "invalid-desktop-smoke-json", "detail": str(exc)})

    required_truth = {
        "completed": True,
        "app_launched": True,
        "source_ingested": True,
        "prompt_submitted": True,
        "response_non_empty": True,
        "chat_attempt_trace_captured": True,
    }
    for key, expected in required_truth.items():
        if receipt.get(key) is not expected:
            findings.append({"severity": "error", "code": "desktop-smoke-proof-missing", "field": key, "value": receipt.get(key)})

    if receipt.get("run_id") != RUN_ID:
        findings.append({"severity": "error", "code": "desktop-smoke-run-mismatch", "value": receipt.get("run_id")})
    if receipt.get("blocked"):
        findings.append({"severity": "error", "code": "desktop-smoke-blocked", "blockers": receipt.get("blockers", [])})
    if not receipt.get("citations"):
        findings.append({"severity": "error", "code": "desktop-smoke-citations-missing"})
    if not receipt.get("retrieval_backend_used") or not receipt.get("retrieval_mode"):
        findings.append({"severity": "error", "code": "desktop-smoke-retrieval-proof-missing"})

    errors = [finding for finding in findings if finding.get("severity") == "error"]
    result = {
        "ok": not errors,
        "run_id": RUN_ID,
        "error_count": len(errors),
        "finding_count": len(findings),
        "findings": findings,
    }
    print(json.dumps(result, indent=2))
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
