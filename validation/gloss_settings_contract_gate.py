#!/usr/bin/env python3
"""Static gate for settings/diagnostics contract alignment."""
from __future__ import annotations
import re
import sys
from pathlib import Path
ROOT = Path(sys.argv[1]) if len(sys.argv) > 1 else Path.cwd()
FAIL=[]

def read(rel):
    p=ROOT/rel
    if not p.exists():
        FAIL.append(f"missing required file: {rel}"); return ""
    return p.read_text(errors="ignore")

settings=read("src-tauri/src/commands/settings.rs")
types=read("src/lib/types.ts")
tauri=read("src/lib/tauri.ts")
ui=read("src/components/settings/SettingsDialog/index.tsx")

required_terms=["semantic_memory", "provider", "model", "dimensions", "projection_summary"]
for term in required_terms:
    if term not in settings:
        FAIL.append(f"settings diagnostics/profile status missing term: {term}")

if "run_embedding_diagnostics" in settings and "ensure_embedder(None)" in settings and "semantic_memory" in settings:
    FAIL.append("run_embedding_diagnostics still appears to initialize native embedder as semantic-memory diagnostic")

if "nomic-embed-text" in settings and "semantic_memory" in settings and "get_setting" not in settings[settings.find("nomic-embed-text")-500:settings.find("nomic-embed-text")+500]:
    FAIL.append("settings diagnostics may still hardcode nomic-embed-text")

for field in ["provider_healthy", "model_found", "model_available"]:
    if field not in settings or field not in types:
        FAIL.append(f"ProviderModelTestResult field drift or missing field: {field}")

if ("model_list_count" in types or "model_list_count" in tauri) and "model_list_count" not in settings:
    FAIL.append("frontend expects model_list_count but backend result does not expose it")

if "selected scope has no chunk-bearing sources" in settings and "next_actions" not in settings:
    FAIL.append("chunk-bearing source error must include actionable next_actions")

if "semantic_memory_auto_project: true" in settings:
    FAIL.append("blocked profile receipt still hardcodes semantic_memory_auto_project true")

if "Run embedding diagnostics" not in ui:
    FAIL.append("settings UI lost embedding diagnostics action")

if FAIL:
    print("FAILURES:")
    for f in FAIL:
        print("  -", f)
    sys.exit(1)
print("gloss_settings_contract_gate: PASS")
