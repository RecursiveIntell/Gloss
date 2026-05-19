#!/usr/bin/env python3
from __future__ import annotations
import argparse, json, re, sys
from pathlib import Path

def text(path: Path) -> str:
    try: return path.read_text(encoding='utf-8')
    except Exception: return ''

def check(condition: bool, code: str, detail: str, findings: list[dict]):
    if not condition:
        findings.append({'code': code, 'detail': detail})

def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument('--repo', default='.')
    ap.add_argument('--allow-current-preview', action='store_true', help='allow known pre-pass degraded adapter markers')
    args = ap.parse_args()
    repo = Path(args.repo).resolve()
    findings = []
    adapter_p = repo/'src-tauri/src/memory/semantic_memory_adapter.rs'
    backend_p = repo/'src-tauri/src/memory/backend.rs'
    sm_lib = repo/'src-tauri/vendor/semantic-memory/src/lib.rs'
    sm_docs = repo/'src-tauri/vendor/semantic-memory/src/documents.rs'
    sm_config = repo/'src-tauri/vendor/semantic-memory/src/config.rs'
    sm_search = repo/'src-tauri/vendor/semantic-memory/src/search.rs'
    adapter = text(adapter_p)
    backend = text(backend_p)
    lib = text(sm_lib)
    docs = text(sm_docs)
    config = text(sm_config)
    search = text(sm_search)

    check(adapter_p.exists(), 'missing-adapter', str(adapter_p), findings)
    check(backend_p.exists(), 'missing-backend', str(backend_p), findings)
    check(sm_config.exists(), 'missing-semantic-memory-config', str(sm_config), findings)
    check('DerivedVectorBackendPolicy' in config and 'TurboQuantCandidateOnly' in config, 'missing-turboquant-config', 'semantic-memory config lacks TurboQuantCandidateOnly policy', findings)
    check('exact_rerank' in search or 'reranked_from_f32' in search, 'missing-exact-rerank-signal', 'semantic-memory search should expose exact rerank signal', findings)

    has_manifest_api = ('ExternalChunkManifest' in lib+docs+adapter) or ('ChunkManifest' in lib+docs+adapter and 'sm_chunk_id' in lib+docs+adapter)
    if not args.allow_current_preview:
        check(has_manifest_api, 'missing-chunk-manifest-api', 'No public/exposed chunk-manifest ingest API/mapping detected', findings)
        check('ingest_document_chunk_manifest' in adapter or 'ExternalChunkManifest' in adapter, 'adapter-not-using-chunk-manifest', 'Gloss adapter does not appear to use chunk-manifest ingest', findings)
        check('sm_chunk_id: Option<&str> = None' not in adapter, 'adapter-forces-missing-sm-chunk', 'Adapter still forces sm_chunk_id to None', findings)
        check('degraded-missing-exact-backpointer' not in adapter or 'assert' in adapter.lower() or 'test' in adapter.lower(), 'adapter-degraded-backpointer-marker', 'Adapter still contains degraded-missing-exact-backpointer production marker', findings)
        check('ExactnessProfile::PreferExact' not in adapter or 'TurboQuant' in adapter or 'AllowApproximate' in adapter, 'turbo-blocked-by-prefer-exact', 'Adapter may force PreferExact and block TurboQuant candidate backend', findings)
    check("sync_status != \"synced\"" in backend or "sync_status == \"synced\"" in backend, 'missing-sync-status-filter', 'Backend should filter semantic links by sync_status', findings)
    check('sm_chunk_id' in backend and 'content_digest' in backend, 'missing-backpointer-filter', 'Backend should require sm_chunk_id or content_digest mapping', findings)

    out = {'ok': not findings, 'findings': findings}
    print(json.dumps(out, indent=2, sort_keys=True))
    return 0 if not findings else 1

if __name__ == '__main__':
    raise SystemExit(main())
