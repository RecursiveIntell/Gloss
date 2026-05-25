#!/usr/bin/env python3
"""Static verification gate for Gloss semantic-memory + TurboQuant full fix.

This does not replace Rust/TS tests. It checks that the expected implementation surfaces exist.
"""
from __future__ import annotations
import json, sys
from pathlib import Path

ROOT = Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve()
errors: list[str] = []
warnings: list[str] = []

def read(path: str) -> str:
    p = ROOT / path
    if not p.exists():
        errors.append(f"missing {path}")
        return ""
    if p.is_dir():
        return "\n".join(
            child.read_text(errors="replace")
            for child in sorted(p.rglob("*.rs"))
            if child.is_file()
        )
    return p.read_text(errors="replace")

def require(path: str, token: str, label: str | None = None):
    text = read(path)
    if token not in text:
        errors.append(f"{path} missing {label or token}")

def warn_require(path: str, token: str, label: str | None = None):
    text = read(path)
    if token not in text:
        warnings.append(f"{path} missing {label or token}")

# package scripts
package_text = read("package.json")
try:
    scripts = json.loads(package_text).get("scripts", {})
except Exception as exc:
    errors.append(f"package.json parse failed: {exc}")
    scripts = {}
for key in ["tauri:dev:sm", "tauri:dev:sm-tq", "tauri:build:sm", "tauri:build:sm-tq", "test:tauri:sm", "test:tauri:sm-tq"]:
    if key not in scripts:
        errors.append(f"missing package script {key}")

# backend surfaces
require("src-tauri/src/db/migrations.rs", "semantic_memory_projection_status", "source-level projection status table")
require("src-tauri/src/db/notebook_db/mod.rs", "semantic_memory_projection", "projection status DB methods")
require("src-tauri/src/memory/semantic_memory_adapter.rs", "skipped_no_chunks", "zero-chunk skip classification")
require("src-tauri/src/memory/semantic_memory_adapter.rs", "rebuild_vector_artifacts", "vector artifact rebuild surface")
require("src-tauri/src/commands/sources/mod.rs", "SemanticMemoryBackfillReceipt", "batch backfill receipt")
require("src-tauri/src/commands/sources/mod.rs", "semantic_memory_backfill_notebook", "backfill command")
require("src-tauri/src/commands/sources/mod.rs", "semantic_memory_rebuild_vector_artifacts", "TQ artifact rebuild command")
require("src-tauri/src/commands/settings.rs", "get_semantic_memory_profile_status", "profile status command")
require("src-tauri/src/commands/settings.rs", "semantic-memory-strict", "strict SM profile")
require("src-tauri/src/commands/settings.rs", "semantic-memory-turbo-quant-strict", "strict TQ profile")
require("src-tauri/src/commands/chat/mod.rs", "ProjectionReadiness", "chat projection readiness gate")
require("src-tauri/src/commands", "diagnose_retrieval_query", "retrieval diagnostic command")

# frontend surfaces
require("src/stores/sourceStore.ts", "sourceListStatus", "source list status")
require("src/stores/sourceStore.ts", "sourceListError", "source list error")
require("src/stores/sourceStore.ts", "kind: 'none'", "safe none scope")
require("src/components/settings/SettingsDialog/index.tsx", "Run projection backfill", "projection backfill UI")
require("src/components/settings/SettingsDialog/index.tsx", "Run retrieval probe", "retrieval probe UI")
require("src/components/settings/SettingsDialog/index.tsx", "Rebuild TurboQuant artifacts", "TQ artifact rebuild UI")
require("src/lib/tauri.ts", "getSemanticMemoryProfileStatus", "profile status API")
require("src/lib/tauri.ts", "diagnoseRetrievalQuery", "retrieval diagnostics API")
require("src/lib/types.ts", "SemanticMemoryProfileStatus", "profile status TS type")
require("src/lib/types.ts", "RetrievalDiagnostics", "retrieval diagnostics TS type")

# evidence surface warnings
warn_require("src/lib/types.ts", "candidate_backend", "candidate backend evidence")
warn_require("src/lib/types.ts", "vector_artifact_manifest_digest", "TQ artifact digest evidence")
warn_require("src/lib/types.ts", "exact_rerank", "exact rerank evidence")

result = {"status": "fail" if errors else "pass", "errors": errors, "warnings": warnings}
print(json.dumps(result, indent=2))
if errors:
    sys.exit(1)
