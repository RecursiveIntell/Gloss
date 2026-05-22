#!/usr/bin/env python3
from pathlib import Path
import sys

root = Path(__file__).resolve().parents[1]
required = [
    "README.md",
    "AGENTS.md",
    "package.json",
    "src/App.tsx",
    "src-tauri/Cargo.toml",
    "src-tauri/src/lib.rs",
    "src-tauri/src/features.rs",
    "scripts/run_all_checks.sh",
    "scripts/check_gloss_active_validation_scope.py",
    "scripts/check_feature_flags_static.py",
    "scripts/check_release_eligibility_current.py",
    "scripts/gloss_button_up_gate.py",
    "docs/codex-runs/CURRENT_RUN.md",
    "docs/codex-runs/GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519/PHASE_REPORTS.md",
    "scripts/run_all_checks.sh",
    "scripts/run_completion_checks.sh",
]
missing = [p for p in required if not (root / p).exists()]
if missing:
    print("Missing:")
    for m in missing:
        print(f"  - {m}")
    sys.exit(1)
print("Gloss bundle structure check passed.")
