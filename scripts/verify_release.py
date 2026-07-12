#!/usr/bin/env python3
"""Canonical, fail-fast release verification for Gloss.

Commands write their normal output to stderr. The final receipt is emitted as a
single bounded JSON document on stdout and can also be written with --receipt.
"""

from __future__ import annotations

import argparse
import json
import shlex
import subprocess
import sys
import time
from pathlib import Path
from typing import Callable, NamedTuple, Sequence


MAX_GATE_RECORDS = 16
MAX_COMMAND_CHARS = 512


class Gate(NamedTuple):
    name: str
    command: tuple[str, ...] | None


Runner = Callable[[tuple[str, ...], Path], int]


def build_gates(*, skip_desktop_compile: bool) -> list[Gate]:
    """Return the complete, ordered release gate inventory."""
    desktop_command: tuple[str, ...] | None
    if skip_desktop_compile:
        desktop_command = None
    else:
        desktop_command = (
            "npm",
            "exec",
            "--",
            "tauri",
            "build",
            "--debug",
            "--no-bundle",
            "--features",
            "semantic-memory-turbo-quant",
        )

    return [
        Gate("cargo_fmt", ("cargo", "fmt", "--all", "--", "--check")),
        Gate(
            "tauri_contract",
            ("python3", "validation/verify_tauri_contract.py", "."),
        ),
        Gate(
            "rust_static_repair_gates",
            ("bash", "validation/run_all_gloss_repair_gates.sh", "."),
        ),
        Gate(
            "cargo_check_default",
            ("cargo", "check", "--locked", "--manifest-path", "src-tauri/Cargo.toml"),
        ),
        Gate(
            "cargo_check_semantic_memory_backend",
            (
                "cargo",
                "check",
                "--locked",
                "--manifest-path",
                "src-tauri/Cargo.toml",
                "--no-default-features",
                "--features",
                "semantic-memory-backend",
            ),
        ),
        Gate(
            "cargo_check_semantic_memory_turbo_quant",
            (
                "cargo",
                "check",
                "--locked",
                "--manifest-path",
                "src-tauri/Cargo.toml",
                "--no-default-features",
                "--features",
                "semantic-memory-turbo-quant",
            ),
        ),
        Gate(
            "cargo_test_semantic_memory_turbo_quant",
            (
                "cargo",
                "test",
                "--locked",
                "--manifest-path",
                "src-tauri/Cargo.toml",
                "--no-default-features",
                "--features",
                "semantic-memory-turbo-quant",
            ),
        ),
        Gate("npm_unit_tests", ("npm", "run", "test:unit")),
        Gate("npm_static_contract_tests", ("npm", "run", "test:contracts")),
        Gate("npm_build", ("npm", "run", "build")),
        Gate("cargo_deny", ("cargo", "deny", "check", "advisories", "licenses", "sources")),
        Gate(
            "npm_production_audit",
            ("npm", "audit", "--omit=dev", "--audit-level=high"),
        ),
        Gate("desktop_compile", desktop_command),
    ]


def command_text(command: Sequence[str]) -> str:
    return shlex.join(command)[:MAX_COMMAND_CHARS]


def subprocess_runner(command: tuple[str, ...], root: Path) -> int:
    return subprocess.run(
        command,
        cwd=root,
        check=False,
        stdout=sys.stderr,
        stderr=sys.stderr,
    ).returncode


def run_gates(
    gates: Sequence[Gate],
    *,
    root: Path,
    runner: Runner = subprocess_runner,
) -> tuple[dict[str, object], int]:
    """Run ordered gates once, returning after the first failed command."""
    records: list[dict[str, object]] = []
    skipped = False

    for gate in gates:
        if gate.command is None:
            skipped = True
            records.append(
                {
                    "name": gate.name,
                    "command": None,
                    "exit_code": None,
                    "elapsed_seconds": 0.0,
                    "status": "skipped",
                }
            )
            continue

        started = time.monotonic()
        try:
            exit_code = runner(gate.command, root)
        except FileNotFoundError:
            exit_code = 127
        elapsed = round(time.monotonic() - started, 3)
        status = "passed" if exit_code == 0 else "failed"
        records.append(
            {
                "name": gate.name,
                "command": command_text(gate.command),
                "exit_code": exit_code,
                "elapsed_seconds": elapsed,
                "status": status,
            }
        )
        if exit_code != 0:
            return {"status": "failed", "gates": records[:MAX_GATE_RECORDS]}, exit_code

    overall_status = "passed_with_skips" if skipped else "passed"
    return {"status": overall_status, "gates": records[:MAX_GATE_RECORDS]}, 0


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path.cwd(), help="repository root")
    parser.add_argument(
        "--skip-desktop-compile",
        action="store_true",
        help="skip the desktop compile gate for local fast verification; CI must not use this",
    )
    parser.add_argument("--receipt", type=Path, help="optional path for the JSON receipt")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    root = args.root.resolve()
    receipt, exit_code = run_gates(
        build_gates(skip_desktop_compile=args.skip_desktop_compile),
        root=root,
    )
    rendered = json.dumps(receipt, separators=(",", ":"), sort_keys=True)
    if args.receipt:
        args.receipt.parent.mkdir(parents=True, exist_ok=True)
        args.receipt.write_text(rendered + "\n", encoding="utf-8")
    print(rendered)
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
