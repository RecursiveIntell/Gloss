#!/usr/bin/env python3
"""Static assertions for Gloss active/selectable retrieval stack.

This is intentionally conservative. It catches known false-completion patterns from the
semantic-memory/TurboQuant activation passes.
"""
from __future__ import annotations
import sys
from pathlib import Path

ROOT = Path(sys.argv[1]) if len(sys.argv) > 1 else Path('.')
errors: list[str] = []
warnings: list[str] = []

def read(rel: str) -> str:
    p = ROOT / rel
    if not p.exists():
        errors.append(f"missing {rel}")
        return ""
    return p.read_text(encoding='utf-8', errors='replace')

app = read('src/App.tsx')
source_store = read('src/stores/sourceStore.ts')
sources_panel = read('src/components/sources/SourcesPanel.tsx')
chat_panel = read('src/components/chat/ChatPanel.tsx')
source_scope = read('src-tauri/src/retrieval/source_scope.rs')
notebook_db = read('src-tauri/src/db/notebook_db/mod.rs')
sources_cmd = read('src-tauri/src/commands/sources/mod.rs')
settings_cmd = read('src-tauri/src/commands/settings.rs')
semantic_adapter = read('src-tauri/src/memory/semantic_memory_adapter.rs')
package_json = read('package.json')

# 1. Known large batch false-ready pattern.
if 'EAGER_BATCH_SOURCE_LOAD_LIMIT' in app and 'payload.count <= EAGER_BATCH_SOURCE_LOAD_LIMIT' in app:
    errors.append('large batch source list reload is still gated by EAGER_BATCH_SOURCE_LOAD_LIMIT')

# 2. Sources panel must render sourceListStatus before empty-state.
if 'sourceListStatus' not in sources_panel or 'sourceListError' not in sources_panel:
    errors.append('SourcesPanel does not consume sourceListStatus/sourceListError')
if 'sources.length === 0 && (' in sources_panel and 'No sources yet' in sources_panel:
    warnings.append('SourcesPanel still has a raw sources.length empty-state; ensure it is guarded by stats/sourceListStatus')

# 3. Chat panel should not locally reinvent scope mode.
if 'const scopeMode =' in chat_panel and 'sources.length === 0' in chat_panel:
    errors.append('ChatPanel still derives scopeMode locally from sources.length')

# 4. Source store needs explicit scope mode, not only selectedSourceIds size.
if 'SourceScopeMode' not in source_store:
    errors.append('sourceStore lacks explicit SourceScopeMode')
if 'selectedSourceIds.size === 0' in source_store and "kind: 'none'" in source_store:
    warnings.append('sourceStore may still infer none from empty selectedSourceIds')

# 5. Backend DB APIs should not rely on empty [] meaning all/none.
if 'pub fn retrieval_coverage(' in notebook_db and 'source_ids: &[String]' in notebook_db:
    errors.append('retrieval_coverage still accepts raw source_ids slice instead of explicit scope')
if 'pub fn semantic_memory_projection_summary(' in notebook_db and 'source_ids: &[String]' in notebook_db:
    errors.append('semantic_memory_projection_summary still accepts raw source_ids slice instead of explicit scope')
if 'source_ids.is_empty()' in notebook_db and ('SELECT COUNT(*) FROM chunks{chunk_clause}' in notebook_db or 'semantic_memory_links' in notebook_db):
    warnings.append('notebook_db still branches on source_ids.is_empty(); audit for none/all widening')

# 6. Diagnostics should run shared probe path, not bespoke raw coverage/query.
if 'diagnose_retrieval_query' in sources_cmd and 'fts_search_chunks_in_sources(&query' in sources_cmd:
    errors.append('diagnose_retrieval_query still runs bespoke FTS path instead of shared retrieval probe')
if 'run_retrieval_probe' not in sources_cmd:
    errors.append('missing run_retrieval_probe command')

# 7. TurboQuant must be compile-feature gated.
if 'semantic_memory_rebuild_vector_artifacts' in sources_cmd and 'not(feature = "semantic-memory-turbo-quant")' not in sources_cmd:
    errors.append('semantic_memory_rebuild_vector_artifacts lacks hard non-TQ compile gate')
if 'exact_rerank: false' in sources_cmd and 'exact_rerank_count: 0' in sources_cmd:
    errors.append('TurboQuant vector artifact status still hardcodes exact_rerank=false/count=0')

# 8. Receipt tables should exist.
if 'semantic_memory_vector_artifact_receipts' not in read('src-tauri/src/db/migrations.rs'):
    errors.append('missing semantic_memory_vector_artifact_receipts migration')
if 'semantic_memory_retrieval_probe_receipts' not in read('src-tauri/src/db/migrations.rs'):
    errors.append('missing semantic_memory_retrieval_probe_receipts migration')

# 9. Package scripts.
for script in ['tauri:dev:sm', 'tauri:dev:sm-tq', 'tauri:build:sm', 'tauri:build:sm-tq', 'test:tauri:sm', 'test:tauri:sm-tq']:
    if script not in package_json:
        errors.append(f'missing package script {script}')

# 10. Semantic adapter must request and preserve receipts.
if 'ReceiptMode::ReturnReceipt' not in semantic_adapter:
    errors.append('semantic-memory adapter does not request returned receipts')
if 'candidate_backend' not in semantic_adapter or 'exact_rerank_count' not in semantic_adapter:
    warnings.append('semantic adapter may not expose candidate_backend/exact_rerank_count from receipts')

print({
    'status': 'fail' if errors else 'pass',
    'errors': errors,
    'warnings': warnings,
})
if errors:
    sys.exit(1)
