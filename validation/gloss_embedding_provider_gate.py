#!/usr/bin/env python3
import argparse, json, pathlib, re

def current_run(repo: pathlib.Path) -> str | None:
    try:
        text = (repo / "docs/codex-runs/CURRENT_RUN.md").read_text(errors="ignore")
        match = re.search(r"Current run:\s*`?([^`\n]+)`?", text)
        return match.group(1).strip() if match else None
    except Exception:
        return None

RUN_ID = current_run(pathlib.Path(".").resolve()) or "GLOSS_RELEASE_PROOF_EMBEDDING_UNIFICATION_CLEANUP_20260525"

def text(p):
    try: return p.read_text(errors='ignore')
    except Exception: return ''

def main():
    ap=argparse.ArgumentParser(); ap.add_argument('--repo', default='.')
    repo=pathlib.Path(ap.parse_args().repo).resolve()
    run_id = current_run(repo) or RUN_ID
    failures=[]; warnings=[]
    sm_adapter=text(repo/'src-tauri/src/memory/semantic_memory_adapter.rs')
    embed_rs=text(repo/'src-tauri/src/ingestion/embed.rs')
    settings=text(repo/'src-tauri/src/db/migrations.rs')+text(repo/'src-tauri/src/state.rs')+text(repo/'src/components/settings/SettingsDialog/index.tsx')
    all_rs='\n'.join(text(p) for p in (repo/'src-tauri/src').rglob('*.rs'))
    if 'open_with_embedder' not in sm_adapter:
        failures.append('semantic_memory_adapter.rs does not use MemoryStore::open_with_embedder for release-default provider injection')
    if not re.search(r'FastEmbed.*Embedder|Embedder.*FastEmbed|SemanticMemoryFastEmbed|FastEmbedSemantic', all_rs, re.I):
        failures.append('no explicit FastEmbed semantic-memory Embedder adapter found')
    if not re.search(r'EmbeddingProvider(Kind|Profile|Config|Receipt)|embedding_provider', all_rs, re.I):
        failures.append('no canonical embedding provider config/receipt boundary found')
    if "semantic_memory_embedding_provider" not in settings and "embedding_provider" not in settings:
        failures.append('settings/migration do not expose semantic-memory embedding provider selection')
    if re.search(r'MemoryStore::open\s*\(', sm_adapter) and 'open_with_embedder' not in sm_adapter:
        failures.append('semantic adapter still appears hardwired to MemoryStore::open/Ollama')
    if 'Run embedding diagnostics' not in text(repo/'src/components/settings/SettingsDialog/index.tsx') and 'embedding_diagnostics' not in all_rs:
        failures.append('embedding diagnostics UI/command not found')
    receipt=repo/'docs'/'codex-runs'/run_id/'EMBEDDING_PROVIDER_RECEIPT.json'
    if receipt.exists():
        try:
            data=json.loads(receipt.read_text())
            if data.get('release_default_provider') != 'fastembed': failures.append('receipt release_default_provider is not fastembed')
            if data.get('dimension') not in (768, '768'): failures.append('receipt dimension is not 768')
        except Exception as e: failures.append(f'invalid EMBEDDING_PROVIDER_RECEIPT.json: {e}')
    else:
        warnings.append('EMBEDDING_PROVIDER_RECEIPT.json not present yet')
    print(json.dumps({'ok': not failures, 'failures': failures, 'warnings': warnings}, indent=2))
    return 0 if not failures else 1
if __name__ == '__main__': raise SystemExit(main())
