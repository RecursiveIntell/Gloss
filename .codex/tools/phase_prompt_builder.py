#!/usr/bin/env python3
"""Build deterministic Codex phase prompts from the manifest."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
MANIFEST = ROOT / ".codex" / "prompt_manifest.json"


def read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def load_manifest(path: Path = MANIFEST) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def build_prompt(phase_id: str, include_master: bool = True) -> str:
    manifest = load_manifest()
    phases = {phase["id"]: phase for phase in manifest["phases"]}
    if phase_id not in phases:
        raise SystemExit(f"unknown phase id: {phase_id}")
    phase = phases[phase_id]
    chunks: list[str] = []
    if include_master:
        chunks.append("# Loaded Master Prompt\n\n" + read_text(ROOT / manifest["master_prompt"]))
    chunks.append("# Loaded Phase Prompt\n\n" + read_text(ROOT / phase["prompt"]))
    chunks.append("# Loaded Automatic Phase Gate\n\n" + read_text(ROOT / phase["auto_injection"]))
    chunks.append("# Declared Required Commands\n\n```json\n" + json.dumps(phase.get("required_commands", []), indent=2) + "\n```")
    return "\n\n---\n\n".join(chunks).strip() + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("phase_id")
    parser.add_argument("--no-master", action="store_true")
    args = parser.parse_args()
    print(build_prompt(args.phase_id, include_master=not args.no_master))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
