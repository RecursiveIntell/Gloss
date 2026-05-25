#!/usr/bin/env python3
"""Replay release-truth gates from the current tree or a freshly unzipped archive."""
from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path


REQUIRED_PATHS = [
    "Cargo.toml",
    "Cargo.lock",
    "package.json",
    "docs/codex-runs/CURRENT_RUN.md",
    "docs/CURRENT_FEATURE_MATRIX.md",
    "src-tauri/Cargo.toml",
    "src-tauri/tauri.conf.json",
    "scripts/gloss_current_run_truth_gate.py",
    "scripts/gloss_validator_path_gate.py",
    "scripts/gloss_runtime_log_gate.py",
    "scripts/gloss_receipt_integrity_gate.py",
    "scripts/gloss_feature_matrix_gate.py",
    "scripts/gloss_desktop_smoke_harness.py",
]


def _extract_archive(archive: Path, destination: Path) -> Path:
    if destination.exists():
        shutil.rmtree(destination)
    destination.mkdir(parents=True)
    with zipfile.ZipFile(archive) as zf:
        zf.extractall(destination)
    candidates = [p for p in destination.iterdir() if p.is_dir()]
    if len(candidates) == 1 and (candidates[0] / "package.json").exists():
        return candidates[0]
    return destination


def _run(target: Path, command: list[str]) -> dict:
    completed = subprocess.run(
        command,
        cwd=target,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    return {
        "command": " ".join(command),
        "exit_code": completed.returncode,
        "stdout_tail": completed.stdout[-4000:],
        "stderr_tail": completed.stderr[-4000:],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--archive")
    parser.add_argument("--repo", default=".")
    parser.add_argument("--fresh-unzip")
    parser.add_argument(
        "--presence-only",
        action="store_true",
        help="Check required paths without replaying executable gates.",
    )
    args = parser.parse_args()

    repo = Path(args.repo).resolve()
    if args.archive:
        archive = Path(args.archive).resolve()
        target = _extract_archive(
            archive,
            Path(args.fresh_unzip or tempfile.mkdtemp(prefix="gloss-replay-")).resolve(),
        )
        mode = "fresh_unzip_archive"
    else:
        target = repo
        mode = "repo"

    failures: list[str] = []
    for required in REQUIRED_PATHS:
        if not (target / required).exists():
            failures.append(f"missing {required} in replay root {target}")

    commands: list[dict] = []
    skips: list[str] = []
    if not failures and not args.presence_only:
        replay_commands = [
            ["python3", "scripts/gloss_current_run_truth_gate.py", "--repo", str(target)],
            ["python3", "scripts/gloss_validator_path_gate.py", "--repo", str(target)],
            ["python3", "scripts/gloss_receipt_integrity_gate.py", "--repo", str(target)],
            ["python3", "scripts/gloss_feature_matrix_gate.py", "--repo", str(target)],
        ]
        runtime_bad_log = target / "fixtures/runtime_bad_missing_notebook_context_length.log"
        if runtime_bad_log.exists():
            replay_commands.append([
                "python3",
                "scripts/gloss_runtime_log_gate.py",
                "--log",
                str(runtime_bad_log),
                "--expect",
                "fail",
            ])
        else:
            skips.append(f"runtime bad log fixture absent from replay archive: {runtime_bad_log}")
        replay_commands.append(["cargo", "metadata", "--format-version", "1", "--no-deps"])
        commands = [_run(target, command) for command in replay_commands]
        failures.extend(
            f"{result['command']} exited {result['exit_code']}"
            for result in commands
            if result["exit_code"] != 0
        )

    out = {
        "schema": "GlossP35ReleaseReplayGateV1",
        "status": "fail" if failures else "pass",
        "mode": mode,
        "root": str(target),
        "failures": failures,
        "skips": skips,
        "commands": commands,
    }
    print(json.dumps(out, indent=2))
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
