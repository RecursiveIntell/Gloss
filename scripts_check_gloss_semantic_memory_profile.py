#!/usr/bin/env python3
"""Static gate sketch for Gloss semantic-memory/TurboQuant reactivation.

Copy to scripts/check_gloss_semantic_memory_profile.py during implementation.
This intentionally checks for expected surfaces, not runtime correctness.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve()
errors: list[str] = []
warnings: list[str] = []

def read(path: str) -> str:
    p = ROOT / path
    if not p.exists():
        errors.append(f"missing {path}")
        return ""
    return p.read_text(errors="replace")

package = read("package.json")
try:
    scripts = json.loads(package).get("scripts", {})
except Exception as exc:
    errors.append(f"package.json parse failed: {exc}")
    scripts = {}

for name in [
    "tauri:dev:sm",
    "tauri:dev:sm-tq",
    "tauri:build:sm",
    "tauri:build:sm-tq",
    "test:tauri:sm",
    "test:tauri:sm-tq",
]:
    if name not in scripts:
        errors.append(f"missing package script {name}")

features = read("src-tauri/src/features.rs")
for token in [
    "enable_semantic_memory_profile",
    "use_gloss_local_profile",
    "get_semantic_memory_profile_status",
    "semantic_memory_auto_project",
    "semantic_memory_strict_testing",
]:
    if token not in features and token not in read("src-tauri/src/commands/sources.rs"):
        errors.append(f"missing expected profile/status token: {token}")

chat = read("src-tauri/src/commands/chat.rs")
for token in [
    "backend_requested",
    "backend_used",
    "fallback_reason",
    "semantic_memory_candidate_backend",
    "exact_rerank",
]:
    if token not in chat:
        warnings.append(f"chat evidence may not expose {token}")

settings = read("src/components/settings/SettingsDialog.tsx")
for token in [
    "Enable semantic-memory",
    "TurboQuant",
    "Run retrieval probe",
    "Strict",
]:
    if token not in settings:
        errors.append(f"settings UI missing expected text/action: {token}")

sources = read("src-tauri/src/commands/sources.rs")
for token in [
    "semantic_memory_reindex_source",
    "semantic_memory_reindex_notebook",
    "semantic_memory_rebuild_vector_artifacts",
    "diagnose_retrieval_query",
]:
    if token not in sources and token not in chat and token not in read("src-tauri/src/memory/semantic_memory_adapter.rs"):
        warnings.append(f"missing or not yet implemented: {token}")

print(json.dumps({"errors": errors, "warnings": warnings, "status": "fail" if errors else "pass"}, indent=2))
sys.exit(1 if errors else 0)
