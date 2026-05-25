#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RUN_ID = "GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519"

REQUIRED_ROOT_FILES = [
    "AGENTS.md",
    "CODEX_MASTER_PROMPT.md",
    "PHASE_PROMPTS",
    f"codex/schemas/{RUN_ID}/final_receipt.schema.json",
    f"docs/codex-runs/{RUN_ID}/PHASE_ORDER.md",
    f"docs/codex-runs/{RUN_ID}/ACCEPTANCE_GATES.md",
]

EXPECTED_PHASE_IDS = [
    "PHASE_00",
    "PHASE_01",
    "PHASE_02",
    "PHASE_03",
    "PHASE_04",
    "PHASE_05",
    "PHASE_06",
    "PHASE_07",
    "PHASE_08",
    "PHASE_09",
    "PHASE_10",
    "PHASE_11",
    "PHASE_12",
]

PHASE_PROMPT_RE = re.compile(r"^PHASE_(\d{2})(?:_|$).*\.md$")


def validate() -> list[str]:
    errors: list[str] = []

    missing = [path for path in REQUIRED_ROOT_FILES if not (ROOT / path).exists()]
    if missing:
        errors.extend(f"missing required file: {path}" for path in missing)

    prompt_dir = ROOT / "PHASE_PROMPTS"
    phase_prompt_ids = sorted(
        {f"PHASE_{match.group(1)}"
        for path in prompt_dir.glob("PHASE_*.md")
        if (match := PHASE_PROMPT_RE.match(path.name))
        }
    )
    if phase_prompt_ids != EXPECTED_PHASE_IDS:
        errors.append(
            f"unexpected P33 phase prompt ordering or membership: {phase_prompt_ids}"
        )

    phase_order_path = ROOT / "docs" / "codex-runs" / RUN_ID / "PHASE_ORDER.md"
    if phase_order_path.exists():
        phase_order = phase_order_path.read_text(encoding="utf-8", errors="replace")
        for phase_id in EXPECTED_PHASE_IDS:
            if phase_id not in phase_order:
                errors.append(f"PHASE_ORDER.md missing {phase_id}")

    for path in [
        "scripts/validate_codex_pack.py",
        "scripts/assert_codex_active_pack.py",
        "scripts/run_completion_checks.sh",
    ]:
        if not (ROOT / path).exists():
            errors.append(f"missing required repository command/script: {path}")

    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--quiet", action="store_true")
    args = parser.parse_args()

    errors = validate()
    if not errors and not args.quiet:
        print("OK: codex pack validation passed")
        return 0

    if errors:
        if not args.quiet:
            print("Codex pack validation failed:")
            for error in errors:
                print(f"- {error}")
        return 1

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
