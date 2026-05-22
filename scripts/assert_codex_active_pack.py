#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RUN_ID = "GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519"
REQUIRED = [
    "AGENTS.md",
    f"codex/prompts/{RUN_ID}/MASTER_PROMPT.md",
    f"codex/prompts/{RUN_ID}/PHASE_12_FINAL_AUDIT_RELEASE_DECISION.md",
    f"codex/schemas/{RUN_ID}/final_receipt.schema.json",
    f"docs/codex-runs/{RUN_ID}/PHASE_ORDER.md",
    f"docs/codex-runs/{RUN_ID}/ACCEPTANCE_GATES.md",
]
EXPECTED_PHASE_IDS = [f"PHASE_{idx:02d}" for idx in range(13)]
PHASE_PROMPT_RE = re.compile(r"^PHASE_(\d{2})_.*\.md$")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--quiet", action="store_true")
    args = parser.parse_args()
    missing = [p for p in REQUIRED if not (ROOT / p).exists()]
    errors: list[str] = []
    errors.extend(f"missing required file: {p}" for p in missing)

    prompt_dir = ROOT / "codex" / "prompts" / RUN_ID
    phase_prompt_ids = sorted(
        f"PHASE_{match.group(1)}"
        for path in prompt_dir.glob("PHASE_*.md")
        if (match := PHASE_PROMPT_RE.match(path.name))
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

    if errors:
        if not args.quiet:
            print("Codex active pack validation failed:")
            for error in errors:
                print(f"- {error}")
        return 1
    if not args.quiet:
        print("OK: active P33 Codex pack present and phase prompts are valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
