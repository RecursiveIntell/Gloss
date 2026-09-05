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
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable, NamedTuple, Sequence

from source_snapshot import capture_source_identity


MAX_GATE_RECORDS = 24
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
            "python_script_contracts",
            ("python3", "-m", "unittest", "discover", "-s", "scripts/tests", "-v"),
        ),
        Gate(
            "python_validation_contracts",
            ("python3", "-m", "unittest", "discover", "-s", "validation/tests", "-v"),
        ),
        Gate(
            "native_owner_contracts",
            ("cargo", "test", "--locked", "--manifest-path", "validation/native_harness/Cargo.toml"),
        ),
        Gate(
            "turbo_quant_runtime_contracts",
            ("cargo", "test", "--locked", "--manifest-path", "validation/turbo_quant_harness/Cargo.toml"),
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
        Gate(
            "cargo_clippy",
            ("cargo", "clippy", "--locked", "--manifest-path", "src-tauri/Cargo.toml",
             "--features", "semantic-memory-turbo-quant", "--", "-D", "warnings"),
        ),
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
    if len(gates) > MAX_GATE_RECORDS:
        raise ValueError("Gate inventory exceeds the receipt bound; refusing truncated proof")
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


def verify_snapshot(
    root: Path, gates: Sequence[Gate], *, runner: Runner = subprocess_runner,
) -> tuple[dict[str, object], int]:
    """Bind execution to one unchanged source snapshot and retain missing gates."""
    started_at = datetime.now(timezone.utc).isoformat()
    try:
        source_before = capture_source_identity(root)
    except (OSError, ValueError, subprocess.SubprocessError) as exc:
        return {
            "schema": "GlossBuildVerificationV2", "status": "failed",
            "scope": "build_and_contracts_only", "source_error": str(exc),
            "gates": [], "unrun_gates": [gate.name for gate in gates],
        }, 1
    receipt, exit_code = run_gates(
        gates, root=root, runner=runner,
    )
    try:
        source_after = capture_source_identity(root)
    except (OSError, ValueError, subprocess.SubprocessError) as exc:
        source_after = {"error": str(exc)}
    source_unchanged = source_before == source_after
    if not source_unchanged:
        receipt["status"] = "failed"
        receipt["source_error"] = "Checkout changed during verification; rerun on the final source"
        exit_code = exit_code or 1
    receipt.update({
        "schema": "GlossBuildVerificationV2",
        "scope": "build_and_contracts_only",
        "started_at": started_at,
        "finished_at": datetime.now(timezone.utc).isoformat(),
        "source_before": source_before,
        "source_after": source_after,
        "source_unchanged": source_unchanged,
        "unrun_gates": [gate.name for gate in gates[len(receipt["gates"]):]],
        "remaining_runtime_gates": ["live_desktop", "live_provider", "package_replay", "native_leak_check"],
    })
    return receipt, exit_code


def main() -> int:
    args = parse_args()
    root = args.root.resolve()
    receipt, exit_code = verify_snapshot(
        root, build_gates(skip_desktop_compile=args.skip_desktop_compile),
    )
    rendered = json.dumps(receipt, separators=(",", ":"), sort_keys=True)
    if args.receipt:
        args.receipt.parent.mkdir(parents=True, exist_ok=True)
        args.receipt.write_text(rendered + "\n", encoding="utf-8")
    print(rendered)
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
