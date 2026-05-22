#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from pathlib import Path

DEFAULT_RUN_ID = "GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519"


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace") if path.exists() else ""


def contains_any(path: Path, needles: list[str]) -> bool:
    text = read(path)
    return all(needle in text for needle in needles)


def main() -> int:
    parser = argparse.ArgumentParser(description="P33 release candidate startup preflight.")
    parser.add_argument("--repo", default=".")
    parser.add_argument("--run-id", default=DEFAULT_RUN_ID)
    args = parser.parse_args()
    repo = Path(args.repo).resolve()
    findings: list[dict[str, str]] = []

    required_paths = [
        "AGENTS.md",
        "README.md",
        "package.json",
        "src-tauri/Cargo.toml",
        "src-tauri/tauri.conf.json",
        "src-tauri/src/features.rs",
        "src-tauri/src/commands/settings.rs",
        "src-tauri/src/memory/semantic_memory_adapter.rs",
        "src-tauri/src/retrieval/hybrid_search.rs",
        "src/components/settings/SettingsDialog.tsx",
        "docs/codex-runs/CURRENT_RUN.md",
        f"docs/codex-runs/{args.run_id}/FINAL_RECEIPT.json",
    ]
    for rel in required_paths:
        if not (repo / rel).exists():
            findings.append({"severity": "error", "code": "missing-required-path", "path": rel})

    if args.run_id not in read(repo / "docs/codex-runs/CURRENT_RUN.md"):
        findings.append(
            {
                "severity": "error",
                "code": "current-run-mismatch",
                "path": "docs/codex-runs/CURRENT_RUN.md",
            }
        )

    feature_file = repo / "src-tauri/src/features.rs"
    required_feature_tokens = [
        "EXPERIMENTAL_FEATURES_ENABLED",
        "FEATURE_SEMANTIC_MEMORY_PREVIEW_ENABLED",
        "FEATURE_SEMANTIC_MEMORY_TURBO_QUANT_ENABLED",
        "default_enabled: false",
        "semantic_memory_preview_active",
        "turbo_quant_active",
        'cfg!(feature = "semantic-memory-backend")',
        'cfg!(feature = "semantic-memory-turbo-quant")',
    ]
    if not contains_any(feature_file, required_feature_tokens):
        findings.append(
            {
                "severity": "error",
                "code": "feature-governance-incomplete",
                "path": "src-tauri/src/features.rs",
            }
        )

    adapter_file = repo / "src-tauri/src/memory/semantic_memory_adapter.rs"
    required_adapter_tokens = [
        "SemanticMemoryRuntimeConfig",
        "turbo_quant_enabled: false",
        "runtime_config_from_settings",
        "turbo_quant_require_exact_rerank",
    ]
    if not contains_any(adapter_file, required_adapter_tokens):
        findings.append(
            {
                "severity": "error",
                "code": "semantic-memory-runtime-config-incomplete",
                "path": "src-tauri/src/memory/semantic_memory_adapter.rs",
            }
        )

    errors = [finding for finding in findings if finding.get("severity") == "error"]
    result = {
        "ok": not errors,
        "run_id": args.run_id,
        "error_count": len(errors),
        "finding_count": len(findings),
        "findings": findings,
    }
    print(json.dumps(result, indent=2))
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
