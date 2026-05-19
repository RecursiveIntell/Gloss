#!/usr/bin/env python3
"""Assert required SCR handoff paths in an archive match active workflow claims."""
import sys
import zipfile
from pathlib import Path

BASE_REQUIRED = {
    "AGENTS.md",
    "Cargo.toml",
    "README.md",
    "crates/scr-kernel/src/lib.rs",
    "crates/scr-reference/src/lib.rs",
    "crates/scr-reference/src/policy.rs",
    "crates/scr-cli/src/main.rs",
    "schemas/generated/control-evaluation-input-v1.schema.json",
    "schemas/generated/control-decision-receipt-v1.schema.json",
    "policies/audit_policy_v1.toml",
    "scripts/run_p31_completion_checks.sh",
    "scripts/verify_archive_manifest_parity.py",
    "docs/SOURCE_BASIS.md",
    "docs/EXTERNAL_CRATE_BOUNDARY_MAP.md",
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
        or p.startswith("scr-runtime-generic-rust-next-codex-context-")
        or p.endswith(FORBIDDEN_SUFFIXES)
    )
    # If .codex is present, require the active automation basics. If absent, do not fail here.
    has_codex = any(p.startswith(".codex/") for p in names)
    if has_codex:
        for p in [".codex/tools/auto_phase_runner.py", ".codex/prompt_manifest.json"]:
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
