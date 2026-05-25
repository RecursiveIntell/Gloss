#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default=".")
    args = parser.parse_args()
    root = Path(args.repo).resolve()
    canonical = root / "codex-control-pack"
    legacy = root / "codex_control_pack"
    failures: list[str] = []
    warnings: list[str] = []
    if not canonical.exists():
        failures.append("missing canonical codex-control-pack")
    if legacy.exists():
        failures.append("legacy codex_control_pack is active at repo root")
    install_scripts = [
        path.relative_to(root).as_posix()
        for path in root.glob("install_into*")
        if path.is_file()
    ]
    if install_scripts:
        failures.append(f"ambiguous root installer scripts: {install_scripts}")
    quarantined = root / "docs/noncanonical-source-archive/20260524-p34-quarantine/stale-control-surfaces/codex_control_pack"
    if not quarantined.exists():
        warnings.append("no quarantined legacy codex_control_pack receipt path found")
    print(json.dumps({"ok": not failures, "failures": failures, "warnings": warnings}, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
