#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REQUIRED_RUN_ID = "GLOSS_TOTAL_COMPLETION_AND_HARDENING_SUPERPASS_20260526"


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace") if path.exists() else ""


def current_run() -> str | None:
    match = re.search(
        r"Current run:\s*`?([^`\n]+)`?",
        read(ROOT / "docs/codex-runs/CURRENT_RUN.md"),
    )
    return match.group(1).strip() if match else None


def validate() -> list[str]:
    errors: list[str] = []
    run_id = current_run()
    if run_id != REQUIRED_RUN_ID:
        errors.append(f"current run is {run_id!r}, expected {REQUIRED_RUN_ID}")

    pack_manifest = ROOT / "PACK_MANIFEST.json"
    try:
        manifest = json.loads(read(pack_manifest) or "{}")
    except Exception as exc:
        errors.append(f"PACK_MANIFEST.json invalid JSON: {exc}")
        manifest = {}
    if manifest.get("run_id") != REQUIRED_RUN_ID:
        errors.append("PACK_MANIFEST.json run_id does not match active run")

    for path in [
        "AGENTS.md",
        "README.md",
        "docs/codex-runs/CURRENT_RUN.md",
        f"docs/codex-runs/{REQUIRED_RUN_ID}/startup_preflight.md",
        f"docs/codex-runs/{REQUIRED_RUN_ID}/subagent_findings.md",
        f"docs/codex-runs/{REQUIRED_RUN_ID}/STALE_PASS_CLEANUP_MANIFEST.json",
        f"docs/codex-runs/{REQUIRED_RUN_ID}/LIVE_SEMANTIC_MEMORY_SMOKE_RECEIPT.json",
        f"docs/codex-runs/{REQUIRED_RUN_ID}/TURBOQUANT_RUNTIME_RECEIPT.json",
        f"docs/codex-runs/{REQUIRED_RUN_ID}/TIMEOUT_CHANGE_RECEIPT.json",
        f"docs/codex-runs/{REQUIRED_RUN_ID}/RELEASE_CANDIDATE_GATE_RESULTS.json",
    ]:
        if not (ROOT / path).exists():
            errors.append(f"missing required active pack file: {path}")

    for path in ["AGENTS.md", "README.md"]:
        if REQUIRED_RUN_ID not in read(ROOT / path):
            errors.append(f"{path} does not reference {REQUIRED_RUN_ID}")

    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--quiet", action="store_true")
    args = parser.parse_args()
    errors = validate()
    if errors:
        if not args.quiet:
            print("Codex pack validation failed:")
            for error in errors:
                print(f"- {error}")
        return 1
    if not args.quiet:
        print("OK: active Codex pack validation passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
