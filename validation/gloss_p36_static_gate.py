#!/usr/bin/env python3
"""Static release gate for Gloss P36 dense indexing + TurboQuant pass.

This script is intentionally conservative. It catches the exact classes of
failures visible in the current package before release claims are made.
"""
from __future__ import annotations
import argparse
import json
import re
import sys
from pathlib import Path

def current_run(repo: Path) -> str | None:
    text = (repo / "docs/codex-runs/CURRENT_RUN.md").read_text(errors="ignore")
    match = re.search(r"Current run:\s*`?([^`\n]+)`?", text)
    return match.group(1).strip() if match else None


# fallback — overridden inside main() once --repo is parsed
_RUN_ID = "GLOSS_P36_RELEASE_COMPLETION_DENSE_TQ_RELEASE_20260525"


def resolve_run_id(repo: Path) -> str:
    return current_run(repo) or _RUN_ID


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace")


def fail(msg: str, failures: list[str]) -> None:
    failures.append(msg)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default=".")
    args = parser.parse_args()
    repo = Path(args.repo).resolve()
    run_id = resolve_run_id(repo)
    failures: list[str] = []
    warnings: list[str] = []

    cargo = read(repo / "src-tauri" / "Cargo.toml")
    package = read(repo / "package.json")

    if "semantic-memory-turbo-quant" not in cargo:
        fail("src-tauri/Cargo.toml does not define semantic-memory-turbo-quant", failures)
    if re.search(r'default\s*=\s*\[[^\]]*"semantic-memory-turbo-quant"', cargo, re.S) is None:
        if "tauri:build:release" not in package and "tauri:build:sm-tq" not in package:
            fail("Release/default build does not clearly include semantic-memory-turbo-quant", failures)
        else:
            warnings.append("Cargo default does not include TQ; relying on explicit release script")
    if "semantic-memory/turbo-quant-codec" not in cargo:
        fail("semantic-memory-turbo-quant feature does not include semantic-memory/turbo-quant-codec", failures)

    state = read(repo / "src-tauri" / "src" / "state.rs")
    if "NATIVE_SEMANTIC_INDEXING_ENABLED: bool = false" in state:
        fail("Native dense indexing is still hard-disabled in state.rs", failures)
    # Robust check: find the actual constant assignment, not just "false" in nearby text
    idx_match = re.search(r'NATIVE_SEMANTIC_INDEXING_ENABLED\s*:\s*bool\s*=\s*(true|false)', state)
    if idx_match and idx_match.group(1) == "false":
        fail("Native dense indexing constant still appears false/disabled", failures)

    sources_mod = read(repo / "src-tauri" / "src" / "commands" / "sources" / "mod.rs")
    if re.search(r'IngestionOpts\s*\{[^}]*embed_chunks:\s*false[^}]*queue_summary:\s*false', sources_mod, re.S):
        fail("Folder import still hardcodes embed_chunks=false and queue_summary=false", failures)
    if '"semantic_memory_synced"' in sources_mod and "source_processing_state" not in sources_mod:
        fail("semantic_memory_synced is still emitted without rich source state separation", failures)

    types = read(repo / "src" / "lib" / "types.ts")
    if "SourceProcessingState" not in types:
        fail("Frontend SourceProcessingState type is missing", failures)

    chat = read(repo / "src" / "components" / "chat" / "ChatPanel.tsx")
    if 'source.status !== "ready"' in chat or "source.status !== 'ready'" in chat:
        fail("ChatPanel still computes readiness from raw source.status", failures)
    if 'source.status === "pending"' in chat or "source.status === 'pending'" in chat:
        fail("ChatPanel still computes unindexed count from raw source.status", failures)

    for tsx in (repo / "src").rglob("*.tsx"):
        txt = read(tsx)
        if "\\u00" in txt:
            fail(f"Visible unicode escape remains in TSX: {tsx.relative_to(repo)}", failures)

    adapter = read(repo / "src-tauri" / "src" / "memory" / "semantic_memory_adapter.rs")
    if "ProjectionSubchunk" not in adapter and "subchunk" not in adapter.lower():
        fail("semantic_memory_adapter.rs lacks deterministic projection subchunking", failures)
    if "SEMANTIC_MEMORY_PROJECTION_MAX_CHUNKS_PER_BATCH: usize = 12" in adapter:
        fail("Projection batch cap is still 12 chunks; lower release-safe cap required", failures)
    if "24_000" in adapter or "6_000" in adapter:
        fail("Projection char/token budget still uses oversized current defaults", failures)

    migrations = read(repo / "src-tauri" / "src" / "db" / "migrations.rs")
    if "source_processing_state" not in migrations:
        fail("DB migration for source_processing_state missing", failures)

    if run_id not in "\n".join([read(p) for p in [repo / "AGENTS.md", repo / "README.md"] if p.exists()]):
        warnings.append("Run ID not visible in AGENTS.md/README.md; acceptable only if docs/codex-runs/CURRENT_RUN.md owns it")

    result = {"gate": "gloss_p36_static_gate", "failures": failures, "warnings": warnings}
    print(json.dumps(result, indent=2))
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
