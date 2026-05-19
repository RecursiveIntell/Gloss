#!/usr/bin/env python3
from pathlib import Path
import sys

root = Path(__file__).resolve().parents[1]
required = [
    "README.md",
    "AGENTS.md",
    "prompts/00_MAIN_CODEX_PROMPT.md",
    "prompts/01_PHASE_0_SOURCE_BASIS.md",
    "crates/scr-kernel/src/lib.rs",
    "crates/scr-reference/src/lib.rs",
    "crates/scr-reference/src/policy.rs",
    "crates/scr-audit-adapter/src/lib.rs",
    "scripts/run_p31_completion_checks.sh",
    "specs/SCR_P0A_SPEC.md",
    "specs/ACCEPTANCE_GATES.md",
    "policies/audit_policy_v1.toml",
    "scripts/run_all_checks.sh",
    "scripts/run_completion_checks.sh",
]
missing = [p for p in required if not (root / p).exists()]
if missing:
    print("Missing:")
    for m in missing:
        print(f"  - {m}")
    sys.exit(1)
print("Bundle structure check passed.")
