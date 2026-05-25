#!/usr/bin/env python3
import argparse, json, re
from pathlib import Path

def main():
 ap=argparse.ArgumentParser(); ap.add_argument('--repo', default='.') ; args=ap.parse_args(); root=Path(args.repo)
 failures=[]; warnings=[]
 sources=(root/'src-tauri/src/commands/sources/mod.rs').read_text(errors='ignore')
 chat=(root/'src-tauri/src/commands/chat/mod.rs').read_text(errors='ignore')
 adapter=(root/'src-tauri/src/memory/semantic_memory_adapter.rs').read_text(errors='ignore')
 if 'app_db.get_notebook(&notebook_id)' not in sources.split('pub async fn add_source_folder')[1].split('tauri::async_runtime::spawn')[0]:
  failures.append('add_source_folder does not validate notebook exists before spawning background import')
 if 'import_batch_id' not in sources:
  failures.append('no import_batch_id in sources import pipeline')
 if 'cancelled_superseded' not in sources:
  failures.append('no cancelled_superseded terminal state for stale notebook/source jobs')
 if 'input length exceeds the context length' not in adapter and 'max_manifest' not in adapter and 'batch' not in adapter.lower():
  failures.append('semantic-memory adapter has no explicit context-length/batch retry handling')
 if 'CitationAnchor' not in chat + adapter + (root/'src-tauri/src/retrieval/citations.rs').read_text(errors='ignore'):
  failures.append('no CitationAnchorV1/CitationAnchor type in retrieval/citation path')
 if 'CitationFilterReason' not in chat + (root/'src-tauri/src/retrieval/citations.rs').read_text(errors='ignore'):
  warnings.append('no CitationFilterReason type; filtered citation reasons likely aggregate-only')
 if 'source_scope_preserved: true' in adapter or 'source_scope_preserved: true' in (root/'src-tauri/src/memory/gloss_local.rs').read_text(errors='ignore'):
  failures.append('source_scope_preserved true still hard-coded in memory backend/adapter')
 print(json.dumps({'ok':not failures,'failures':failures,'warnings':warnings}, indent=2))
 return 0 if not failures else 1
if __name__=='__main__': raise SystemExit(main())
