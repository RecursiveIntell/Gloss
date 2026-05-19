#!/usr/bin/env python3
"""Reject stale/non-authoritative surfaces in active SCR repo."""
from pathlib import Path
import sys

ROOT = Path.cwd()
FORBIDDEN_ACTIVE_PATHS = [
    ROOT / "testtmp",
    ROOT / "target_files",
    ROOT / "manual_injections",
]
FORBIDDEN_TERMS = [
    "ClaimLedger",
    "claim_ledger",
]
ALLOWED_HISTORY_PARTS = [
    "docs/codex-runs/archive/",
    "docs/root-markdown-archive/",
    "docs/codex-runs/",
    "docs/codex-runs",
]
ALLOWED_EXACT_PATHS = {
    ROOT / "scr-runtime-generic-rust-next-codex-context-20260513.manifest.json",
    ROOT / "scr-runtime-generic-rust-next-codex-context-20260513.report.md",
    ROOT / "scr-runtime-generic-rust-next-codex-context-20260513.excluded.json",
    ROOT / "scr-runtime-generic-rust-next-codex-context-20260513.findings.json",
    ROOT / "scr-runtime-generic-rust-next-codex-context-20260513.codex-archive.json",
    ROOT / "scr-runtime-generic-rust-next-codex-context-20260513.zip",
}
TEXT_EXTS = {".md", ".rs", ".toml", ".json", ".py", ".sh", ".txt", ".rules"}


def is_allowed_history(path: Path) -> bool:
    rel = path.relative_to(ROOT).as_posix()
    return (
        any(part in rel for part in ALLOWED_HISTORY_PARTS)
        or rel.startswith("prompts/")
        or rel.startswith(".codex/")
        or rel.startswith(".agents/")
        or rel in (p.relative_to(ROOT).as_posix() for p in ALLOWED_EXACT_PATHS)
    )


def main() -> int:
    errors = []
    for path in FORBIDDEN_ACTIVE_PATHS:
        if path.exists():
            errors.append(f"forbidden active path exists: {path.relative_to(ROOT)}")
    for path in ROOT.rglob("*"):
        if path == ROOT / "scripts/assert_no_stale_surfaces.py":
            continue
        if (
            not path.is_file()
            or path.suffix not in TEXT_EXTS
            or is_allowed_history(path)
            or str(path).startswith(str(ROOT / ".git"))
        ):
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except Exception as exc:
            errors.append(f"cannot read {path}: {exc}")
            continue
        for term in FORBIDDEN_TERMS:
            if term in text:
                errors.append(f"forbidden/stale term {term!r} in {path.relative_to(ROOT)}")
    if errors:
        print("stale surface violations:", file=sys.stderr)
        for err in errors[:300]:
            print(f"  {err}", file=sys.stderr)
        return 1
    print("ok no stale active surfaces")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
