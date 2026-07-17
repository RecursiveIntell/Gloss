# Semantic-Memory Completion Spec

## Required fixes

1. Only emit `semantic_memory_feature_disabled` when the build/runtime feature gate is actually closed.
2. Add exact reason codes for projection, links, search, timeout, mapping, digest mismatch, scope filtering, and TurboQuant artifact freshness.
3. Implement `SemanticMemoryRuntimeTruthV1` backend command and per-answer snapshot/digest.
4. Expand and activate `RetrievalCapabilityDecisionV1`.
5. Attach runtime truth and retrieval decision to every answer evidence payload.
6. Add strict no-fallback semantic-memory diagnostic mode.
7. Add isolated semantic-memory diagnostic: FastEmbed init, open store, ingest one fixture, search it, map candidate.
8. Add notebook projection backfill/reconcile with counts and receipts.
9. Add existing notebook health reconciliation.
10. Add UI source health state from backend truth.

## Required reason codes

```text
semantic_memory_build_feature_missing
semantic_memory_experimental_master_disabled
semantic_memory_preview_flag_disabled
semantic_memory_projection_required
semantic_memory_projection_failed
semantic_memory_links_missing
semantic_memory_links_degraded
semantic_memory_search_timeout
semantic_memory_search_error
semantic_memory_no_candidates
semantic_memory_no_mapped_candidates
semantic_memory_candidate_digest_mismatch
semantic_memory_scope_filtered
semantic_memory_turbo_quant_artifact_stale
semantic_memory_used
```

## Strict live fixture

A fresh fixture must:

1. create clean notebook;
2. import known text;
3. chunk it;
4. embed with FastEmbed;
5. project to semantic-memory;
6. run strict semantic-memory query with fallback disabled;
7. require `backend_used=semantic-memory-preview`;
8. require `fallback_used=false`;
9. require mapped source/chunk ids;
10. emit semantic-memory runtime truth and retrieval decision receipts.
