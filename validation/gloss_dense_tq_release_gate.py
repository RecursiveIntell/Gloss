#!/usr/bin/env python3
"""Dense indexing + TurboQuant release evidence gate.

Checks static build evidence and, optionally, release-run receipts produced by
Codex or the app. This is a guardrail script; it is not a substitute for cargo,
Tauri, or live GUI smoke commands.
"""
from __future__ import annotations
import argparse
import json
import re
import sys
from pathlib import Path

RUN_ID = "GLOSS_P36_RELEASE_COMPLETION_DENSE_TQ_RELEASE_20260525"


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace")


def load_json(path: Path):
    return json.loads(read(path))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", default=".")
    ap.add_argument("--require-live-evidence", action="store_true")
    args = ap.parse_args()
    repo = Path(args.repo).resolve()
    run_dir = repo / "docs" / "codex-runs" / RUN_ID
    failures: list[str] = []
    warnings: list[str] = []

    cargo = read(repo / "src-tauri" / "Cargo.toml")
    package = read(repo / "package.json")

    if "semantic-memory-turbo-quant" not in cargo:
        failures.append("TurboQuant feature absent from Cargo.toml")
    if "semantic-memory/turbo-quant-codec" not in cargo:
        failures.append("TurboQuant codec feature not wired")
    if "tauri:build:sm-tq" not in package and "tauri:build:release" not in package:
        failures.append("No npm release build script with semantic-memory-turbo-quant")

    state = read(repo / "src-tauri" / "src" / "state.rs")
    if "NATIVE_SEMANTIC_INDEXING_ENABLED: bool = false" in state:
        failures.append("Native dense indexing hard-disabled")

    if args.require_live_evidence:
        required = [
            "DENSE_INDEXING_RECEIPT.json",
            "TURBOQUANT_BUILD_RECEIPT.json",
            "SEMANTIC_MEMORY_PROJECTION_RECEIPT.json",
            "LIVE_DESKTOP_SMOKE_RECEIPT.json",
            "FINAL_RECEIPT.json",
        ]
        for name in required:
            if not (run_dir / name).exists():
                failures.append(f"Missing required release evidence: {run_dir / name}")

        if (run_dir / "DENSE_INDEXING_RECEIPT.json").exists():
            dense = load_json(run_dir / "DENSE_INDEXING_RECEIPT.json")
            if not dense.get("enabled", False):
                failures.append("Dense indexing receipt does not say enabled=true")
            if int(dense.get("indexed_chunks", 0)) <= 0:
                failures.append("Dense indexing receipt has no indexed chunks")

        if (run_dir / "TURBOQUANT_BUILD_RECEIPT.json").exists():
            tq = load_json(run_dir / "TURBOQUANT_BUILD_RECEIPT.json")
            if not tq.get("compiled", False):
                failures.append("TurboQuant receipt does not say compiled=true")
            if not tq.get("runtime_enabled", False):
                failures.append("TurboQuant receipt does not say runtime_enabled=true")
            if tq.get("exact_rerank") is not True:
                failures.append("TurboQuant receipt does not prove exact_rerank=true")

        if (run_dir / "SEMANTIC_MEMORY_PROJECTION_RECEIPT.json").exists():
            sm = load_json(run_dir / "SEMANTIC_MEMORY_PROJECTION_RECEIPT.json")
            if int(sm.get("context_length_failures", 0)) != 0:
                failures.append("semantic-memory projection has context_length_failures > 0")
            if sm.get("silent_truncation") is True:
                failures.append("semantic-memory projection used silent truncation")

        if (run_dir / "FINAL_RECEIPT.json").exists():
            final = load_json(run_dir / "FINAL_RECEIPT.json")
            if final.get("release_ready") is True:
                for key in ["dense_indexing_passed", "turboquant_passed", "live_smoke_passed", "fresh_replay_passed"]:
                    if final.get(key) is not True:
                        failures.append(f"FINAL_RECEIPT release_ready=true but {key} is not true")

    print(json.dumps({"gate": "gloss_dense_tq_release_gate", "failures": failures, "warnings": warnings}, indent=2))
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
