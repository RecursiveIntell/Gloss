#!/usr/bin/env python3
from __future__ import annotations
import argparse, json, re, subprocess, time
from pathlib import Path

def read(root, rel):
    p=root/rel
    return p.read_text(errors='ignore') if p.exists() else ''

def has(root, rel): return (root/rel).exists()

def main():
    ap=argparse.ArgumentParser(); ap.add_argument('--run-id', default='P32R3'); args=ap.parse_args()
    root=Path.cwd(); failures=[]; warnings=[]
    files={rel: read(root,rel) for rel in [
      'src-tauri/src/commands/chat/mod.rs','src-tauri/src/commands/sources/mod.rs','src-tauri/src/commands/settings.rs',
      'src-tauri/src/providers/mod.rs','src-tauri/src/providers/ollama.rs','src-tauri/src/jobs/mod.rs','src-tauri/src/lib.rs',
      'src-tauri/src/db/notebook_db/mod.rs','src/lib/types.ts','src/lib/events.ts','src/lib/tauri.ts',
      'src/stores/sourceStore.ts','src/components/chat/ChatPanel.tsx','src/components/sources/SourcesPanel.tsx','src/components/layout/StatusBar.tsx'
    ]}
    def require(cond, msg):
        if not cond: failures.append(msg)
    def warn(cond, msg):
        if not cond: warnings.append(msg)

    chat=files['src-tauri/src/commands/chat/mod.rs']; types=files['src/lib/types.ts']; events=files['src/lib/events.ts']
    require('chat:status' in chat or 'ChatStatus' in chat, 'chat.rs must emit typed chat status or equivalent')
    require('ChatStatusPayload' in types, 'types.ts must define ChatStatusPayload')
    require('onChatStatus' in events, 'events.ts must expose onChatStatus')
    require('first_token' in chat.lower() or 'FIRST_TOKEN' in chat, 'chat.rs must include first-token timeout/watchdog')
    require('idle' in chat.lower() and 'timeout' in chat.lower(), 'chat.rs must include stream idle timeout/watchdog')
    require('num_ctx: Some(16_384)' not in chat and 'num_ctx: Some(16384)' not in chat, 'hardcoded universal num_ctx 16384 must be removed')
    require('truncated' in chat.lower() or 'incomplete' in chat.lower(), 'chat.rs must mark truncated/incomplete stream state')

    providers=files['src-tauri/src/providers/mod.rs']; settings=files['src-tauri/src/commands/settings.rs']
    require('unwrap_or_else(|| self.providers.get(&ProviderType::Ollama))' not in providers, 'providers/mod.rs must not default unknown model to Ollama')
    require('provider_id' in settings and re.search(r'if\s+let\s+Some\s*\(|match\s+provider_id', settings), 'refresh_models must branch on provider_id')
    warn('stale' in settings.lower() or 'available' in settings.lower(), 'model stale/unavailable state not obvious in settings.rs')

    source_store=files['src/stores/sourceStore.ts']; tauri=files['src/lib/tauri.ts']; sources=files['src-tauri/src/commands/sources/mod.rs']
    require('addSourceFiles' in source_store or 'add_source_files' in sources or 'addSourceFiles' in tauri, 'batch addSourceFiles/add_source_files missing')
    require('deleteSources' in source_store or 'delete_sources' in sources or 'deleteSources' in tauri, 'bulk deleteSources/delete_sources missing')
    require('debounce' in source_store.lower() or 'setTimeout' in source_store or 'persistTimer' in source_store, 'selection persistence debounce missing')
    require('sort_by' in sources or '.sort()' in sources or 'sort_unstable' in sources, 'folder traversal deterministic sort missing')
    require('folder_scan' in sources or 'scan_empty' in sources or 'scan_truncated' in sources, 'folder scan events missing')

    db=files['src-tauri/src/db/notebook_db/mod.rs']
    require('list_source_headers_needing_summary' in db or ('list_sources_needing_summary' in db and 'content_text' not in re.search(r'list_sources_needing_summary[\s\S]{0,1200}', db).group(0)), 'summary candidate discovery still appears to load content_text')
    warn('transaction' in db.lower() or '.transaction' in db, 'NotebookDb transaction usage not obvious')

    jobs=files['src-tauri/src/jobs/mod.rs']; lib=files['src-tauri/src/lib.rs']; statusbar=files['src/components/layout/StatusBar.tsx']
    warn('gate' in lib.lower() and ('owner' in lib.lower() or 'GateOwner' in lib), 'gate owner tracking not obvious in lib.rs')
    require('selected model' in statusbar.lower() or 'model missing' in statusbar.lower() or 'modelMissing' in statusbar, 'StatusBar must disclose selected model availability, not only provider health')
    require('embed_described_source' not in sources or 'finalize_described_source' in sources, 'fake embed_described_source naming should be removed/aliased to finalization')

    final=root/'docs'/'codex-runs'/args.run_id/'FINAL_RECEIPT.json'
    warn(final.exists(), 'FINAL_RECEIPT.json missing; okay before final phase only')
    receipts=root/'docs'/'codex-runs'/args.run_id/'receipts'
    warn(receipts.exists(), 'phase receipts dir missing')

    rec={'run_id':args.run_id,'ts':time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),'failures':failures,'warnings':warnings}
    out=root/'docs'/'codex-runs'/args.run_id/'reports'/'STATIC_VALIDATOR_REPORT.json'; out.parent.mkdir(parents=True, exist_ok=True); out.write_text(json.dumps(rec, indent=2, sort_keys=True), encoding='utf-8')
    print(json.dumps(rec, indent=2))
    return 1 if failures else 0
if __name__ == '__main__': raise SystemExit(main())
