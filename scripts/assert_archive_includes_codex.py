#!/usr/bin/env python3
from __future__ import annotations

import argparse
import zipfile
from pathlib import Path

REQUIRED = [
    "AGENTS.md",
    "codex/prompts/GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519/MASTER_PROMPT.md",
    "codex/prompts/GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519/PHASE_12_FINAL_AUDIT_RELEASE_DECISION.md",
    "codex/schemas/GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519/final_receipt.schema.json",
    "docs/codex-runs/CURRENT_RUN.md",
    "docs/codex-runs/GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519/FINAL_RECEIPT.json",
]


def archive_names(path: Path) -> set[str]:
    with zipfile.ZipFile(path) as zf:
        names = {name.lstrip("/") for name in zf.namelist()}
    # Support archives with a single root directory prefix.
    expanded = set(names)
    for name in names:
        parts = name.split("/", 1)
        if len(parts) == 2:
            expanded.add(parts[1])
    return expanded


def main() -> int:
    parser = argparse.ArgumentParser(description="Assert a SCR archive includes active Codex control files")
    parser.add_argument("archive", nargs="?")
    args = parser.parse_args()
    if not args.archive:
        parser.print_help()
        return 0
    path = Path(args.archive)
    names = archive_names(path)
    missing = [p for p in REQUIRED if p not in names]
    if missing:
        print("Archive is missing active Codex files:")
        for item in missing:
            print(f"- {item}")
        return 1
    print("OK: archive includes active Codex control pack")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
