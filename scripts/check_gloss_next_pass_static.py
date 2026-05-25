#!/usr/bin/env python3
"""Static release-gate checks for Gloss P31.
Run from repo root: python3 scripts/check_gloss_next_pass_static.py .
"""
from __future__ import annotations
import json, re, sys
from pathlib import Path
root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path('.')
errors: list[str] = []
warnings: list[str] = []
def read(rel: str) -> str:
    p = root / rel
    if not p.exists():
        errors.append(f"missing required file: {rel}")
        return ""
    return p.read_text(encoding='utf-8', errors='replace')
conf_text = read('src-tauri/tauri.conf.json')
if conf_text:
    try:
        conf = json.loads(conf_text)
        csp = conf.get('app', {}).get('security', {}).get('csp', None)
        if csp is None or csp == "": errors.append('tauri.conf.json app.security.csp is null/empty')
    except Exception as e: errors.append(f'tauri.conf.json invalid json: {e}')
cap_text = read('src-tauri/capabilities/default.json')
if cap_text:
    try:
        cap = json.loads(cap_text); perms = set(cap.get('permissions', []))
        if 'shell:default' in perms: errors.append('default capability still grants shell:default')
        if 'fs:default' in perms: warnings.append('default capability grants fs:default; verify scopes are minimal')
        if 'clipboard-manager:allow-read-text' in perms: warnings.append('clipboard read is enabled; justify in SECURITY_AND_PRIVACY')
    except Exception as e: errors.append(f'capabilities/default.json invalid json: {e}')
gloss_local = read('src-tauri/src/memory/gloss_local.rs')
if gloss_local:
    if 'score: 0.0' in gloss_local and 'fts' not in gloss_local.lower() and 'bm25' not in gloss_local.lower(): errors.append('gloss_local.rs still appears source-order-only with score 0.0 and no FTS/BM25 path')
    if re.search(r'for\s+source_id\s+in\s+resolved_scope\.source_ids\(\).*?get_chunks_for_source', gloss_local, re.S): warnings.append('gloss_local.rs still iterates source IDs and chunks directly; verify this is only degraded fallback')
backend = read('src-tauri/src/memory/backend.rs')
if backend:
    if 'semantic_memory_result = None' in backend: errors.append('backend comparison still hardcodes semantic_memory_result = None')
    if re.search(r'candidate\s*\.\s*sm_chunk_id.*?\.or_else\s*\(', backend, re.S): errors.append('semantic candidate mapping still falls back from sm_chunk_id to arbitrary doc chunk')
adapter = read('src-tauri/src/memory/semantic_memory_adapter.rs')
if adapter:
    if 'VALUES (?1, ?2, ?3, ?4, NULL, NULL' in adapter: errors.append('semantic_memory_adapter still inserts NULL sm_chunk_id/sm_episode_id for links')
    if 'content_digest' not in adapter: errors.append('semantic_memory_adapter lacks content digest mapping support')
chat = read('src-tauri/src/commands/chat/mod.rs')
if chat:
    if 'local-retrieval-fallback' in chat and 'fts' not in chat.lower() and 'bm25' not in chat.lower(): warnings.append('chat fallback markers exist but no clear FTS/BM25 evidence path in chat.rs')
    for field in ['backend_requested','backend_used','fallback_used','fallback_reason','degradation_markers','receipt_id']:
        if field not in chat: errors.append(f'chat evidence missing field marker: {field}')
pkg = read('package.json')
if pkg:
    try: json.loads(pkg)
    except Exception as e: errors.append(f'package.json invalid json: {e}')
result = {'errors': errors, 'warnings': warnings, 'status': 'fail' if errors else 'pass'}
print(json.dumps(result, indent=2))
if errors: sys.exit(1)
