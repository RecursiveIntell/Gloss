#!/usr/bin/env python3
"""Assert required Gloss handoff paths in an archive match active workflow claims."""
import sys
import zipfile
from pathlib import Path

BASE_REQUIRED = {
    "AGENTS.md",
    "README.md",
    "package.json",
    "src/App.tsx",
    "src/lib/tauri.ts",
    "src/components/settings/SettingsDialog.tsx",
    "src-tauri/Cargo.toml",
    "src-tauri/src/lib.rs",
    "src-tauri/src/features.rs",
    "scripts/check_feature_flags_static.py",
    "scripts/check_gloss_active_validation_scope.py",
    "scripts/check_release_eligibility_current.py",
    "scripts/gloss_button_up_gate.py",
    "scripts/verify_archive_manifest_parity.py",
    "docs/codex-runs/CURRENT_RUN.md",
    "docs/codex-runs/GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519/FINAL_RECEIPT.json",
}

FORBIDDEN_PREFIXES = (
    "testtmp/",
    "target/",
    "target_files/",
    "manual_injections/",
    "docs/codex-runs/archive/",
    ".codex_run_evidence/",
)
FORBIDDEN_PATHS = {
    "scr-runtime-generic-rust-next-codex-context-",
}
FORBIDDEN_SUFFIXES = (
    ".codex-archive.json",
)


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: assert_required_archive_paths.py <archive.zip>", file=sys.stderr)
        return 2
    with zipfile.ZipFile(Path(sys.argv[1])) as zf:
        names = {i.filename for i in zf.infolist() if not i.is_dir()}
    missing = sorted(BASE_REQUIRED - names)
    forbidden = sorted(
        p
        for p in names
        if p in FORBIDDEN_PATHS
        or p.startswith(FORBIDDEN_PREFIXES)
        or p.startswith("docs/root-markdown-archive/")
        or p.startswith("generic-rust-next-codex-context-")
        or p.endswith(FORBIDDEN_SUFFIXES)
    )
    # If .codex is present, require the active automation basics. If absent, do not fail here.
    has_codex = any(p.startswith(".codex/") for p in names)
    if has_codex:
        for p in [".codex/prompt_manifest.json"]:
            if p not in names:
                missing.append(p)
    if missing or forbidden:
        if missing:
            print("missing required archive paths:", file=sys.stderr)
            for p in missing:
                print(f"  {p}", file=sys.stderr)
        if forbidden:
            print("forbidden archive paths:", file=sys.stderr)
            for p in forbidden:
                print(f"  {p}", file=sys.stderr)
        return 1
    print(f"ok required_archive_paths files={len(names)} codex_present={has_codex}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
