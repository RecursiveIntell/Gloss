# Retrieval Runtime Readiness

Status: `p32-retrieval-runtime-shadow-only`

P32 keeps raw SQLite f32 embeddings authoritative. TurboQuant artifacts are derived acceleration artifacts only, feature-gated behind `turbo-quant-codec`, disabled by default, and exact-reranked before results are returned.

## Runtime Shape

- Exact profile `PreferExact` bypasses derived candidates.
- TurboQuant candidate generation validates one active artifact generation before scoring.
- Approximate scoring reads encoded artifacts and keeps a bounded candidate heap.
- Raw f32 rows are loaded only for selected approximate candidates and exact rerank.
- Receipts disclose generation ID, manifest digest, approximate scanned/returned counts, exact rerank count, raw row load count, filter strategy, fallback, and degradation.

## Degradation Rules

- Missing, invalidated, incomplete, or corrupt generations fall back to authoritative raw f32 and record fallback.
- Filter-aware under-return records degradation instead of silently claiming completeness.
- No material search/build operation should be considered done without receipt metadata or proof debt.

## Remaining Proof Debt

- Full internal and release-candidate benchmark classes are defined but not executed in this pass.
- Workspace-wide gates are quarantined because the parent workspace was dirty before P32 started.
