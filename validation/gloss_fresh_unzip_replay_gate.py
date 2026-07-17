#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile
from datetime import datetime, timezone
from pathlib import Path


RUN_ID_RE = re.compile(r"Current run:\s*`?([^`\n]+)`?")
EXCLUDED_DIRS = {
    ".git",
    ".pytest_cache",
    "__pycache__",
    "dist",
    "node_modules",
    "target",
}
EXCLUDED_GLOSS_DIRS = {
    "src-tauri/vendor",
}
EXCLUDED_SUFFIXES = {
    ".7z",
    ".zip",
}
REQUIRED_PATHS = [
    "Cargo.toml",
    "Cargo.lock",
    "package.json",
    "package-lock.json",
    "docs/codex-runs/CURRENT_RUN.md",
    "docs/CURRENT_FEATURE_MATRIX.md",
    "src-tauri/Cargo.toml",
    "src-tauri/tauri.conf.json",
    "validation/gloss_release_candidate_gate.py",
    "validation/gloss_notebook_portability_gate.py",
    "validation/gloss_fresh_unzip_replay_gate.py",
    "scripts/run_all_checks.sh",
]
REPLAY_COMMANDS = [
    ["python3", "validation/gloss_current_run_truth_gate.py", "--repo", "."],
    ["python3", "validation/gloss_stale_pass_surface_gate.py", "--repo", "."],
    ["python3", "validation/gloss_package_scope_gate.py", "--repo", "."],
    ["python3", "validation/gloss_notebook_portability_gate.py", "--repo", "."],
    ["python3", "validation/gloss_release_candidate_gate.py", "--repo", ".", "--run-id", "CURRENT"],
    ["cargo", "metadata", "--manifest-path", "src-tauri/Cargo.toml", "--format-version", "1", "--no-deps"],
]


def read_text(path: Path) -> str:
    return path.read_text(errors="ignore") if path.exists() else ""


def current_run(repo: Path) -> str:
    match = RUN_ID_RE.search(read_text(repo / "docs/codex-runs/CURRENT_RUN.md"))
    return match.group(1).strip() if match else "UNKNOWN_RUN"


def sha256_file(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def should_exclude(root: Path, path: Path, output_zip: Path) -> bool:
    if path == output_zip:
        return True
    rel = path.relative_to(root).as_posix()
    if any(rel == excluded or rel.startswith(f"{excluded}/") for excluded in EXCLUDED_GLOSS_DIRS):
        return True
    if path.name.startswith("FRESH_UNZIP_REPLAY_SOURCE") or path.name.startswith("FRESH_UNZIP_REPLAY_RECEIPT"):
        return True
    if path.suffix in EXCLUDED_SUFFIXES:
        return True
    return False


def add_tree(
    zip_out: zipfile.ZipFile,
    source_root: Path,
    archive_prefix: str,
    output_zip: Path,
) -> tuple[int, int]:
    count = 0
    total_bytes = 0
    for current, dirnames, filenames in os.walk(source_root):
        current_path = Path(current)
        dirnames[:] = [
            dirname
            for dirname in dirnames
            if dirname not in EXCLUDED_DIRS and not (current_path / dirname).is_symlink()
        ]
        for filename in sorted(filenames):
            path = current_path / filename
            if path.is_symlink() or not path.is_file() or should_exclude(source_root, path, output_zip):
                continue
            rel = path.relative_to(source_root).as_posix()
            zip_out.write(path, f"{archive_prefix}/{rel}")
            count += 1
            total_bytes += path.stat().st_size
    return count, total_bytes


def build_archive(repo: Path, output_zip: Path) -> dict:
    libraries_root = repo.parent / "Libraries"
    if not libraries_root.is_dir():
        raise FileNotFoundError(f"required sibling Libraries directory missing: {libraries_root}")
    output_zip.parent.mkdir(parents=True, exist_ok=True)
    if output_zip.exists():
        output_zip.unlink()
    with zipfile.ZipFile(output_zip, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6) as zip_out:
        gloss_count, gloss_bytes = add_tree(zip_out, repo, "Gloss", output_zip)
        libraries_count, libraries_bytes = add_tree(zip_out, libraries_root, "Libraries", output_zip)
    return {
        "archive_path": str(output_zip),
        "archive_sha256": sha256_file(output_zip),
        "archive_bytes": output_zip.stat().st_size,
        "gloss_files": gloss_count,
        "gloss_bytes": gloss_bytes,
        "libraries_files": libraries_count,
        "libraries_bytes": libraries_bytes,
    }


def extract_archive(archive: Path, destination: Path) -> Path:
    if destination.exists():
        shutil.rmtree(destination)
    destination.mkdir(parents=True)
    with zipfile.ZipFile(archive) as zip_in:
        zip_in.extractall(destination)
    root = destination / "Gloss"
    if not (root / "package.json").exists():
        raise FileNotFoundError(f"fresh unzip did not contain Gloss/package.json under {destination}")
    return root


def run_command(root: Path, command: list[str]) -> dict:
    completed = subprocess.run(
        command,
        cwd=root,
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
    parser.add_argument("--repo", default=".")
    parser.add_argument("--archive")
    parser.add_argument("--fresh-unzip")
    parser.add_argument("--receipt")
    args = parser.parse_args()

    repo = Path(args.repo).resolve()
    run_id = current_run(repo)
    run_dir = repo / "docs/codex-runs" / run_id
    archive = Path(args.archive).resolve() if args.archive else run_dir / "FRESH_UNZIP_REPLAY_SOURCE.zip"
    receipt_path = Path(args.receipt).resolve() if args.receipt else run_dir / "FRESH_UNZIP_REPLAY_RECEIPT.json"
    unzip_dir = Path(args.fresh_unzip).resolve() if args.fresh_unzip else Path(tempfile.mkdtemp(prefix="gloss-fresh-unzip-"))

    failures: list[str] = []
    archive_info: dict = {}
    commands: list[dict] = []
    extracted_root = None
    try:
        archive_info = build_archive(repo, archive)
        extracted_root = extract_archive(archive, unzip_dir)
        for required in REQUIRED_PATHS:
            if not (extracted_root / required).exists():
                failures.append(f"missing required replay path: {required}")
        if not (unzip_dir / "Libraries" / "semantic-memory" / "Cargo.toml").exists():
            failures.append("missing sibling Libraries/semantic-memory path dependency in fresh unzip")
        if not failures:
            commands = [run_command(extracted_root, command) for command in REPLAY_COMMANDS]
            failures.extend(
                f"{result['command']} exited {result['exit_code']}"
                for result in commands
                if result["exit_code"] != 0
            )
    except Exception as exc:
        failures.append(str(exc))

    receipt = {
        "schema": "GlossFreshUnzipReplayReceiptV1",
        "run_id": run_id,
        "recorded_utc": datetime.now(timezone.utc).isoformat(),
        "fresh_unzip_replay_passed": not failures,
        "archive": archive_info,
        "fresh_unzip_dir": str(unzip_dir),
        "extracted_root": str(extracted_root) if extracted_root else None,
        "required_paths": REQUIRED_PATHS,
        "commands": commands,
        "failures": failures,
        "excluded_policy": {
            "dirs": sorted(EXCLUDED_DIRS),
            "gloss_dirs": sorted(EXCLUDED_GLOSS_DIRS),
            "suffixes": sorted(EXCLUDED_SUFFIXES),
        },
    }
    receipt_path.parent.mkdir(parents=True, exist_ok=True)
    receipt_path.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"ok": not failures, "receipt": str(receipt_path), "archive": str(archive), "failures": failures}, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
