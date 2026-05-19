#!/usr/bin/env python3
"""Minimal dry-run phase runner used by completion checks.

This pass only needs the runner to emit deterministic dry-run receipts for the
existing shell checks. It does not execute phase prompts or mutate code.
"""
from __future__ import annotations

import argparse
import json
from datetime import datetime, timezone
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--print-prompts", action="store_true")
    parser.add_argument("--phase")
    parser.add_argument("--receipt", required=True)
    args = parser.parse_args()

    phase = args.phase or "all"
    receipt = {
        "schema": "auto_phase_runner_receipt_v1",
        "phase": phase,
        "dry_run": bool(args.dry_run),
        "print_prompts": bool(args.print_prompts),
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "status": "ok",
    }

    receipt_path = Path(args.receipt)
    receipt_path.parent.mkdir(parents=True, exist_ok=True)
    receipt_path.write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")

    if args.print_prompts:
        print(f"dry-run phase: {phase}")
    else:
        print(json.dumps(receipt, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
