#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REQUIRED = [
    ".codex/config.toml",
    ".codex/hooks.json",
    ".codex/prompt_manifest.json",
    ".codex/prompts/MASTER_AUTOMATED_COMPLETION.md",
    ".codex/tools/auto_phase_runner.py",
    ".codex/tools/phase_prompt_builder.py",
    ".codex/rules/safety.rules",
    ".agents/skills/phase-gate/SKILL.md",
    ".agents/skills/run-certifier/SKILL.md",
    ".agents/skills/hostile-audit/SKILL.md",
    ".agents/skills/source-of-truth-map/SKILL.md",
    ".agents/skills/codex-control-pack/SKILL.md",
    ".codex/skills/codex-control-pack/SKILL.md",
    ".codex/skills/phase-gate/SKILL.md",
    ".codex/skills/hostile-audit/SKILL.md",
    ".codex/skills/run-certifier/SKILL.md",
    ".codex/skills/source-of-truth-map/SKILL.md",
]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--quiet", action="store_true")
    args = parser.parse_args()
    missing = [p for p in REQUIRED if not (ROOT / p).exists()]
    errors: list[str] = []
    errors.extend(f"missing required file: {p}" for p in missing)

    manifest_path = ROOT / ".codex/prompt_manifest.json"
    if manifest_path.exists():
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        if manifest.get("manual_injections_required") is not False:
            errors.append("manual_injections_required must be false")
        if manifest.get("auto_injections_required") is not True:
            errors.append("auto_injections_required must be true")
        for phase in manifest.get("phases", []):
            for key in ("prompt", "auto_injection"):
                rel = phase.get(key)
                if not rel or not (ROOT / rel).exists():
                    errors.append(f"phase {phase.get('id')} missing {key}: {rel}")
    else:
        errors.append("cannot inspect manifest because it is missing")

    if errors:
        if not args.quiet:
            print("Codex active pack validation failed:")
            for error in errors:
                print(f"- {error}")
        return 1
    if not args.quiet:
        print("OK: active Codex pack present and automated phase manifest is valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
