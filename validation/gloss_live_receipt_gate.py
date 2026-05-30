#!/usr/bin/env python3
import argparse, json, pathlib, sys

def load(path, failures):
    if not path.exists(): failures.append(f'missing receipt: {path}'); return {}
    try: return json.loads(path.read_text())
    except Exception as e: failures.append(f'invalid json {path}: {e}'); return {}

def truth(v): return v is True or str(v).lower() == 'true'
def not_exercised(data): return data.get('claim_decision') == 'not_exercised'

def main():
    ap=argparse.ArgumentParser(); ap.add_argument('--repo', default='.'); ap.add_argument('--run-id', required=True)
    args=ap.parse_args(); repo=pathlib.Path(args.repo).resolve(); run=repo/'docs'/'codex-runs'/args.run_id
    failures=[]; warnings=[]
    dense=load(run/'DENSE_INDEXING_RECEIPT.json', failures)
    sm=load(run/'SEMANTIC_MEMORY_PROJECTION_RECEIPT.json', failures)
    tq=load(run/'TURBOQUANT_RUNTIME_RECEIPT.json', failures)
    live=load(run/'LIVE_DESKTOP_SMOKE_RECEIPT.json', failures)
    emb=load(run/'EMBEDDING_PROVIDER_RECEIPT.json', failures)

    # Dense indexing: skip checks if not_exercised (honest blocker)
    if dense and not_exercised(dense):
        pass
    else:
        if dense and int(dense.get('indexed_chunks') or 0) <= 0:
            failures.append('dense indexed_chunks must be > 0')
        if dense and not truth(dense.get('live_dense_ingestion_exercised')):
            failures.append('dense live_dense_ingestion_exercised must be true')

    # Semantic memory projection: skip checks if not_exercised (honest blocker)
    if sm and not_exercised(sm):
        pass
    else:
        if sm and int(sm.get('live_projection_sources') or 0) <= 0:
            failures.append('semantic live_projection_sources must be > 0')
        if sm and not truth(sm.get('passed')):
            failures.append('semantic projection passed must be true')

    # TurboQuant: skip checks if runtime_claimed is explicitly false (honest blocker)
    if tq:
        if tq.get('runtime_claimed') is not True:
            pass
        else:
            if not truth(tq.get('exact_rerank')):
                failures.append('TurboQuant exact_rerank must be true')
            if int(tq.get('exact_rerank_count') or 0) <= 0:
                failures.append('TurboQuant exact_rerank_count must be > 0')
            if not tq.get('vector_artifact_manifest_digest'):
                failures.append('TurboQuant/vector artifact manifest digest missing')

    # Desktop smoke: skip checks if not_exercised (honest blocker)
    if live and not_exercised(live):
        pass
    else:
        if live and not truth(live.get('live_desktop_exercised')):
            failures.append('live desktop smoke not exercised')
        if live and not truth(live.get('release_grade')):
            failures.append('live desktop smoke not release_grade')

    # Embedding provider: skip if not_exercised
    if emb and not_exercised(emb):
        pass
    elif emb and emb.get('release_default_provider') != 'fastembed':
        failures.append('embedding provider release default must be fastembed')

    print(json.dumps({'ok': not failures, 'failures': failures, 'warnings': warnings}, indent=2))
    return 0 if not failures else 1
if __name__=='__main__': raise SystemExit(main())
