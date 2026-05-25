# Source-of-truth map

| Concept | Canonical owner | Must not be owned by | Notes |
|---|---|---|---|
| Source lifecycle readiness | `source_processing_state.lifecycle_status` / backend state API | semantic-memory projection status, frontend-only calculations | `ready` means extract/chunk/index base lifecycle complete. |
| Summary state | summary queue + `source_processing_state.summary_status` | `Source.summary` alone, generic `source.status` | Must distinguish missing/pending/processing/failed/stale/ready. |
| FTS/BM25 index state | SQLite chunks + FTS integrity query + `fts_index_status` | semantic-memory links | Release retrieval can use FTS/BM25 as proven local default. |
| Native dense index state | `chunks.embedding_id`, HNSW index receipt, `dense_index_status` | `source.status`, semantic-memory projection | Dense indexing must be enabled and proven for release. |
| semantic-memory projection | `semantic_memory_projection_status` and `semantic_memory_links` | source lifecycle | Projection can be synced/partial/degraded/failed independently. |
| TurboQuant proof | vector artifact receipts + retrieval probe receipts | README/config labels | Must prove compiled + runtime active + exact rerank. |
| Public release claims | feature matrix + final receipt | README optimism, screenshots | Claim only what gates prove. |
| Package/replay truth | package sidecars + release-evidence manifest | stale Codex run folders | Fresh-unzip replay must be self-contained. |
