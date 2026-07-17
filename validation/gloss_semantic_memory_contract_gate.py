#!/usr/bin/env python3
"""Static gate for semantic-memory chunk/projection correctness."""
from __future__ import annotations
import re
import sys
from pathlib import Path

ROOT = Path(sys.argv[1]) if len(sys.argv) > 1 else Path.cwd()
FAIL: list[str] = []

def read(rel: str) -> str:
    p = ROOT / rel
    if not p.exists():
        FAIL.append(f"missing required file: {rel}")
        return ""
    return p.read_text(errors="ignore")

adapter = read("src-tauri/src/memory/semantic_memory_adapter.rs")
doctor = read("src-tauri/src/db/doctor.rs")
migrations = read("src-tauri/src/db/migrations.rs")
notebook_db = read("src-tauri/src/db/notebook_db/mod.rs")
settings = read("src-tauri/src/commands/settings.rs")

if re.search(r"embedding_provider:\s*DEFAULT_EMBEDDING_PROVIDER,[\s\S]{0,200}embedding_model:\s*FASTEMBED_MODEL_NAME", adapter):
    FAIL.append("SemanticMemoryRuntimeConfig::default pairs default Ollama provider with FastEmbed model")

if re.search(r"config\.embedding\.dimensions\s*=\s*if[\s\S]{0,220}else\s*\{\s*defaults\.dimensions\s*\}", adapter):
    FAIL.append("semantic-memory Ollama dimensions still fall back to EmbeddingConfig::default().dimensions")

if "probe_ollama" not in adapter.lower() and "embedding_dimension" not in adapter.lower():
    FAIL.append("semantic-memory adapter must probe or validate actual embedding dimensions")

if "projection_unit" not in migrations + adapter + notebook_db:
    FAIL.append("semantic-memory schema must model projection_unit identity for subchunks")

if "gloss_chunk_id" not in migrations + adapter + notebook_db:
    FAIL.append("semantic-memory links must carry parent gloss_chunk_id")

if re.search(r"LEFT JOIN chunks c ON c\.id = l\.chunk_id", doctor) and "projection_unit" not in doctor:
    FAIL.append("DB doctor still treats semantic_memory_links.chunk_id as real chunk id; subchunks will be deleted")

if "projection_unit_count" not in notebook_db and "healthy_parent" not in notebook_db:
    FAIL.append("projection summary must distinguish parent chunks from projection units/subchunks")

if re.search(r"contains\(\s*\"embed\"\s*\)|contains\(\s*\"nomic\"\s*\)|contains\(\s*\"bge\"\s*\)", adapter):
    FAIL.append("semantic-memory embedding model validation still uses name substring heuristics")

if "768" in settings and "semantic_memory" in settings and "dimensions" in settings:
    FAIL.append("settings diagnostics appear to hardcode semantic-memory dimensions")

if FAIL:
    print("FAILURES:")
    for f in FAIL:
        print(f"  - {f}")
    sys.exit(1)
print("gloss_semantic_memory_contract_gate: PASS")
