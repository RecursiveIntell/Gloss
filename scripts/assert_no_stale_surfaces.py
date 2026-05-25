#!/usr/bin/env python3
"""Reject stale/non-authoritative surfaces in active Gloss repo."""
import argparse
from pathlib import Path
import sys

DEFAULT_ROOT = Path.cwd()
FORBIDDEN_TERMS = [
    "ClaimLedger",
    "claim_ledger",
]
ALLOWED_HISTORY_PARTS = [
    "docs/codex-runs/archive/",
    "docs/root-markdown-archive/",
    "docs/codex-runs/",
    "docs/codex-runs",
    "docs/noncanonical-source-archive/",
    "src-tauri/vendor/",
]
TEXT_EXTS = {".md", ".rs", ".toml", ".json", ".py", ".sh", ".txt", ".rules"}


def allowed_exact_paths(root: Path) -> set[Path]:
    return {
        root / "scr-runtime-generic-rust-next-codex-context-20260513.manifest.json",
        root / "scr-runtime-generic-rust-next-codex-context-20260513.report.md",
        root / "scr-runtime-generic-rust-next-codex-context-20260513.excluded.json",
        root / "scr-runtime-generic-rust-next-codex-context-20260513.findings.json",
        root / "scr-runtime-generic-rust-next-codex-context-20260513.codex-archive.json",
        root / "scr-runtime-generic-rust-next-codex-context-20260513.zip",
    }


def forbidden_active_paths(root: Path) -> list[Path]:
    return [
        root / "testtmp",
        root / "target_files",
        root / "manual_injections",
        root / "codex_control_pack",
    ]


def is_allowed_history(root: Path, path: Path) -> bool:
    rel = path.relative_to(root).as_posix()
    return (
        any(part in rel for part in ALLOWED_HISTORY_PARTS)
        or rel.startswith("prompts/")
        or rel.startswith(".codex/")
        or rel.startswith(".agents/")
        or rel in (p.relative_to(root).as_posix() for p in allowed_exact_paths(root))
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default=str(DEFAULT_ROOT))
    args = parser.parse_args()
    root = Path(args.repo).resolve()
    errors = []
    for path in forbidden_active_paths(root):
        if path.exists():
            errors.append(f"forbidden active path exists: {path.relative_to(root)}")
    for path in root.rglob("*"):
        if path == root / "scripts/assert_no_stale_surfaces.py":
            continue
        if (
            not path.is_file()
            or path.suffix not in TEXT_EXTS
            or is_allowed_history(root, path)
            or str(path).startswith(str(root / ".git"))
        ):
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except Exception as exc:
            errors.append(f"cannot read {path}: {exc}")
            continue
        for term in FORBIDDEN_TERMS:
            if term in text:
                errors.append(f"forbidden/stale term {term!r} in {path.relative_to(root)}")
    if errors:
        print("stale surface violations:", file=sys.stderr)
        for err in errors[:300]:
            print(f"  {err}", file=sys.stderr)
        return 1
    print("ok no stale active surfaces")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
