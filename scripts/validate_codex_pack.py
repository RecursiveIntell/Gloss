#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

REQUIRED_ROOT_FILES = [
    ".codex/config.toml",
    ".codex/hooks.json",
    ".codex/prompt_manifest.json",
    ".codex/prompts/MASTER_AUTOMATED_COMPLETION.md",
    ".codex/prompts/phase_00_current_state_and_failure_proof.md",
    ".codex/prompts/phase_01_restore_active_codex_pack.md",
    ".codex/prompts/phase_02_auto_phase_runner.md",
    ".codex/prompts/phase_03_packaging_policy.md",
    ".codex/prompts/phase_04_tests_and_release_gates.md",
    ".codex/prompts/phase_05_fresh_unzip_certification.md",
    ".codex/prompts/phase_06_hostile_audit_handoff.md",
    ".codex/auto_gates/phase_00_gate.md",
    ".codex/auto_gates/phase_01_gate.md",
    ".codex/auto_gates/phase_02_gate.md",
    ".codex/auto_gates/phase_03_gate.md",
    ".codex/auto_gates/phase_04_gate.md",
    ".codex/auto_gates/phase_05_gate.md",
    ".codex/auto_gates/phase_06_gate.md",
    ".codex/rules/safety.rules",
    ".codex/tools/auto_phase_runner.py",
    ".codex/tools/phase_prompt_builder.py",
    ".codex/tools/inspect_codex_setup.py",
    ".codex/skills/codex-control-pack/SKILL.md",
    ".codex/skills/phase-gate/SKILL.md",
    ".codex/skills/hostile-audit/SKILL.md",
    ".codex/skills/run-certifier/SKILL.md",
    ".codex/skills/source-of-truth-map/SKILL.md",
    ".agents/skills/codex-control-pack/SKILL.md",
    ".agents/skills/phase-gate/SKILL.md",
    ".agents/skills/hostile-audit/SKILL.md",
    ".agents/skills/run-certifier/SKILL.md",
    ".agents/skills/source-of-truth-map/SKILL.md",
]

EXPECTED_PHASE_IDS = [
    "phase_00",
    "phase_01",
    "phase_02",
    "phase_03",
    "phase_04",
    "phase_05",
    "phase_06",
]

PHASE_ID_RE = re.compile(r"^phase_\d+$")


def validate() -> list[str]:
    errors: list[str] = []

    missing = [path for path in REQUIRED_ROOT_FILES if not (ROOT / path).exists()]
    if missing:
        errors.extend(f"missing required file: {path}" for path in missing)

    manifest_path = ROOT / ".codex/prompt_manifest.json"
    if not manifest_path.exists():
        errors.append("missing required file: .codex/prompt_manifest.json")
    else:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        if manifest.get("manual_injections_required") is not False:
            errors.append("manual_injections_required must be false")
        if manifest.get("auto_injections_required") is not True:
            errors.append("auto_injections_required must be true")
        if manifest.get("master_prompt") != ".codex/prompts/MASTER_AUTOMATED_COMPLETION.md":
            errors.append("master prompt path must be .codex/prompts/MASTER_AUTOMATED_COMPLETION.md")

        phases = manifest.get("phases")
        if not isinstance(phases, list) or not phases:
            errors.append("manifest phases missing or invalid")
        else:
            phase_ids = [phase.get("id") for phase in phases]
            if any(not isinstance(pid, str) or not PHASE_ID_RE.match(pid) for pid in phase_ids):
                errors.append("manifest contains invalid phase id(s)")
            elif phase_ids != EXPECTED_PHASE_IDS:
                errors.append("unexpected phase id ordering or membership")

            for idx, phase in enumerate(phases):
                for field in ("id", "name", "prompt", "auto_injection"):
                    if field not in phase:
                        errors.append(f"phase {idx} missing {field}")
                for field in ("prompt", "auto_injection"):
                    rel = phase.get(field)
                    if rel and not (ROOT / rel).exists():
                        errors.append(f"phase {phase.get('id')} missing {field}: {rel}")

    for path in [
        "scripts/validate_codex_pack.py",
        "scripts/assert_codex_active_pack.py",
        "scripts/run_completion_checks.sh",
    ]:
        if not (ROOT / path).exists():
            errors.append(f"missing required repository command/script: {path}")

    agents_skill_dirs = sorted(d.name for d in (ROOT / ".agents/skills").iterdir() if d.is_dir())
    codex_skill_dirs = sorted(d.name for d in (ROOT / ".codex/skills").iterdir() if d.is_dir())
    if agents_skill_dirs != codex_skill_dirs:
        errors.append(
            "agent and codex skill directories out of sync: "
            f"agents={agents_skill_dirs}, codex={codex_skill_dirs}"
        )

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
