# TurboQuant Readiness

TurboQuant support in semantic-memory is feature-gated candidate generation only. It is disabled by default and must exact-rerank candidates against authoritative SQLite f32 embeddings.

## Current P32 State

- Feature flag: `turbo-quant-codec`.
- Backend policy: `SearchConfig::derived_vector_backend = turbo_quant_candidate_only`.
- Raw f32 embeddings remain authoritative.
- Derived artifacts are stored as acceleration artifacts and are invalidated on authoritative embedding writes, re-embeds, and deletes.
- Rebuilds create `derived_vector_artifact_generations` rows with source snapshot and artifact manifest digests.
- Search receipts disclose fallback/degradation and scanned vs reranked counts.

## Claim Discipline

Do not claim default readiness, production readiness, or lossless retrieval quality. P32 only supports a shadow/runtime-readiness lane until benchmark gates and workspace gates are green.

## Default Eligibility

Default eligibility requires the default gate in `docs/RETRIEVAL_BENCHMARK_GATES.md`, no unexplained fallback, no stale artifacts, and accepted p50/p95/p99 latency against a previous accepted run. P32 defines this gate but does not require it to pass.
