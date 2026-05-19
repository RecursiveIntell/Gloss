# semantic-memory Fix Pass Ledger - 2026-05-11
## Audit Sources
- `04_DEEP_AUDIT_REPORT.md`
- `02_FINDINGS_INDEX.csv`
- `03_FINDINGS_INDEX.json`
## Baseline
- Starting commit: `8bf62c552d7201457e78d242439e09594284bdbe`
- Rust: `rustc 1.93.0 (254b59607 2026-01-19)`
- Cargo: `cargo 1.93.0 (083ac5135 2025-12-15)`
- Note: parent workspace was dirty before this pass; existing changes were preserved.
## Commands Run
- `cargo check -p semantic-memory --all-features`: pass
- `cargo test -p semantic-memory --all-features`: pass
- `cargo fmt --all --check`: pass after formatting
- `cargo clippy -p semantic-memory --all-features -- -D warnings`: pass
- `cargo doc -p semantic-memory --all-features --no-deps`: pass with one pre-existing rustdoc warning
- `cargo test -p semantic-memory --no-default-features --features brute-force`: pass
- `cargo test -p semantic-memory --no-default-features --features hnsw`: pass
- `python3 z.py --root . --profile semantic-memory --mode next-codex-context --strict --check-cargo-path-deps`: pass
- `clean extraction cargo check --workspace --all-features under /home/sikmindz/sm-hermetic`: pass
- `clean extraction cargo check under /tmp/sm-hermetic`: blocked: /tmp tmpfs filled with os error 28
- `TMPDIR=/home/sikmindz/tmp CARGO_TARGET_DIR=/home/sikmindz/sm-hermetic-target bash 01_ACCEPTANCE_GATES.sh /home/sikmindz/sm-hermetic`: pass
## P0/P1 Closure Summary
- P0: 20/27 fixed, 7 explicitly deferred.
- P1: 37/83 fixed, 46 explicitly deferred.

### Fixed P0/P1 Findings
- `SM-AUD-0001` Archive is not hermetic despite passing certifier: Generated semantic-memory archive profile injects root workspace Cargo.toml; clean extraction cargo check passed under /home/sikmindz/sm-hermetic.
- `SM-AUD-0002` No packaged root workspace manifest for included local crates: Generated archive root Cargo.toml includes semantic-memory, stack-ids, semantic-memory-forge, forge-memory-bridge.
- `SM-AUD-0003` Multiple Cargo.lock files create ambiguous dependency source of truth: semantic-memory z.py profile prunes member Cargo.lock files and includes one root Cargo.lock.
- `SM-AUD-0004` Document ingest silently truncates chunks on embedder batch-count mismatch: MemoryStore::embed_batch_internal validates returned batch count before document ingest writes; vector_invariants tests cover fewer/more vectors.
- `SM-AUD-0005` Fact re-embedding silently truncates on batch-count mismatch: MemoryStore::embed_batch_internal validates returned batch count before fact reembed updates.
- `SM-AUD-0006` Chunk re-embedding silently truncates on batch-count mismatch: MemoryStore::embed_batch_internal validates returned batch count before chunk reembed updates.
- `SM-AUD-0007` Message re-embedding silently truncates on batch-count mismatch: MemoryStore::embed_batch_internal validates returned batch count before message reembed updates.
- `SM-AUD-0008` Episode re-embedding silently truncates on batch-count mismatch: MemoryStore::embed_batch_internal validates returned batch count before episode reembed updates.
- `SM-AUD-0009` Public embedding validation is dimension-only: db::validate_embedding checks dimension and finite values; facade write paths call it; vector_invariants tests cover NaN/Inf.
- `SM-AUD-0010` delete_document does not explicitly clean episode derived state: documents::delete_document_with_chunks deletes episode FTS, rowid map, episode_causes, episode rows, and queues HNSW deletes before document delete.
- `SM-AUD-0011` delete_document can leave stale HNSW episode keys: documents::delete_document_with_chunks queues pending HNSW deletes for every episode attached to deleted document.
- `SM-AUD-0012` Vector scan uses bytemuck::try_cast_slice on SQLite Vec<u8>: search vector scan now decodes SQLite blobs with db::decode_f32_le instead of bytemuck casts.
- `SM-AUD-0013` HNSW sidecar loader allocates raw byte_len from file without cap: hnsw sidecar loader validates byte_len against configured dimensions before allocation and uses checked usize conversion.
- `SM-AUD-0015` HNSW save is not atomic: HnswIndex::save writes temp sidecars, fsyncs files, and renames into place.
- `SM-AUD-0018` HNSW sidecar save clones Arc while graph can still mutate: flush/sync paths hold the HNSW write guard while saving/flushing keymap instead of saving an unlocked clone.
- `SM-AUD-0022` cosine_similarity truncates mismatched vectors in release builds: search::cosine_similarity returns Result and checks length in all builds; search_tests cover mismatch.
- `SM-AUD-0023` cosine similarity accepts non-finite stored/query vectors: central validation rejects non-finite vectors before storage; cosine also rejects non-finite query/stored vectors.
- `SM-AUD-0025` conversation HNSW search runs blocking CPU work on async thread: conversation HNSW search is routed through tokio::task::spawn_blocking with bounded candidates.
- `SM-AUD-0026` delete_fact does not clean episode_causes references: delete_fact_with_fts removes episode_causes and derivation_edges referencing the fact.
- `SM-AUD-0027` update_fact does not update dependent episode/projection search text: update_fact_with_fts invalidates derivation_edges touching the fact so dependent projection surfaces must be recomputed.
- `SM-AUD-0028` delete_namespace returns only fact count despite deleting many entity types: delete_namespace now returns NamespaceDeleteReport with facts/documents/chunks/messages/sessions/episodes/projection_rows/hnsw_ops.
- `SM-AUD-0030` SQL errors while counting embeddings are swallowed as zero: HNSW open embedding-count query now propagates SQL errors instead of unwrap_or(0).
- `SM-AUD-0031` Orphan-count SQL errors are swallowed as zero: HNSW orphan-count query now propagates SQL errors instead of unwrap_or(0).
- `SM-AUD-0032` Missing hnsw_keymap table silently leaves loaded graph without keys: load_keymap returns HnswError when hnsw_keymap table is missing while sidecar exists.
- `SM-AUD-0033` Malformed next_id metadata falls back silently: Malformed hnsw_metadata next_id now returns HnswError instead of silently falling back.
- `SM-AUD-0037` u64 node id is cast to usize without range check: HNSW sidecar node_id uses usize::try_from and max_elements validation.
- `SM-AUD-0039` HNSW rebuild silently skips invalid fact embeddings: HNSW rebuild validates fact embedding blobs and returns an error if any invalid rows were skipped.
- `SM-AUD-0040` HNSW rebuild silently skips invalid chunk embeddings: HNSW rebuild validates chunk embedding blobs and returns an error if any invalid rows were skipped.
- `SM-AUD-0041` HNSW rebuild silently skips invalid message embeddings: HNSW rebuild validates message embedding blobs and returns an error if any invalid rows were skipped.
- `SM-AUD-0042` HNSW rebuild silently skips invalid episode embeddings: HNSW rebuild validates episode embedding blobs and returns an error if any invalid rows were skipped.
- `SM-AUD-0046` Invalid timestamp becomes maximally fresh: Invalid timestamps now produce no recency contribution instead of age zero.
- `SM-AUD-0048` candidate_pool_size.max(k * 3) can overflow: Main search candidate calculation uses saturating_mul and max candidate caps.
- `SM-AUD-0049` Second candidate_pool_size.max(k * 3) overflow surface: Vector-only search candidate calculation uses saturating_mul and max candidate caps.
- `SM-AUD-0050` Third candidate_pool_size.max(k * 3) overflow surface: Explained search candidate calculation uses saturating_mul and max candidate caps.
- `SM-AUD-0051` conversation candidate_pool_size.max(k * 3) overflow surface: Conversation search candidate calculation uses saturating_mul and max candidate caps.
- `SM-AUD-0052` Unknown token counts are treated as zero in budget selection: Unknown token counts in budget selection are conservatively estimated from content length.
- `SM-AUD-0053` Token budget addition can overflow u32: Token budget accumulation uses saturating_add.
- `SM-AUD-0054` session_token_count casts negative SQL sum to u64: session_token_count rejects negative aggregates as corrupt data before u64 conversion.
- `SM-AUD-0057` Episode search limit is unbounded: episode search limit is clamped and bound as a SQL parameter.
- `SM-AUD-0059` Episode parse errors report document_id instead of episode_id: episode parsing diagnostics use episode_id rather than document_id.
- `SM-AUD-0060` INSERT OR IGNORE collapses duplicate cause IDs silently: episode cause sync rejects duplicate cause_ids and uses INSERT instead of INSERT OR IGNORE.
- `SM-AUD-0363` Storage byte cast is alignment/endian fragile: db::bytes_to_embedding now uses explicit little-endian decode for persisted blobs.
- `SM-AUD-0377` zip iteration can silently truncate mismatched collections: document chunk/id insertion already checks lengths before zip; retained and covered by existing tests.
- `SM-AUD-0384` zip iteration can silently truncate mismatched collections: document ingest batch zip is guarded by embed_batch_internal count validation.
- `SM-AUD-0466` zip iteration can silently truncate mismatched collections: public batch embedding path validates counts before zips can truncate.
- `SM-AUD-0470` zip iteration can silently truncate mismatched collections: fact reembed zip guarded by embed_batch_internal count validation.
- `SM-AUD-0473` zip iteration can silently truncate mismatched collections: chunk reembed zip guarded by embed_batch_internal count validation.
- `SM-AUD-0475` zip iteration can silently truncate mismatched collections: message reembed zip guarded by embed_batch_internal count validation.
- `SM-AUD-0477` zip iteration can silently truncate mismatched collections: episode reembed zip guarded by embed_batch_internal count validation.
- `SM-AUD-0566` debug_assert is not a release invariant: cosine length invariant is a runtime Result error, not debug_assert.
- `SM-AUD-0567` zip iteration can silently truncate mismatched collections: cosine zip is preceded by runtime length validation.
- `SM-AUD-0573` Storage byte cast is alignment/endian fragile: search vector scan uses explicit little-endian decode for persisted blobs.
- `SM-AUD-0595` zip iteration can silently truncate mismatched collections: Reviewed as test-only zip use; production truncation paths are guarded. Existing quantization/db tests compare lengths or use equal constructed vectors.
- `SM-AUD-0598` zip iteration can silently truncate mismatched collections: Reviewed as test-only zip use; production truncation paths are guarded. Existing quantization/db tests compare lengths or use equal constructed vectors.
- `SM-AUD-0599` zip iteration can silently truncate mismatched collections: Reviewed as test-only zip use; production truncation paths are guarded. Existing quantization/db tests compare lengths or use equal constructed vectors.
- `SM-AUD-0602` zip iteration can silently truncate mismatched collections: Reviewed as test-only zip use; production truncation paths are guarded. Existing quantization/db tests compare lengths or use equal constructed vectors.
- `SM-AUD-0603` zip iteration can silently truncate mismatched collections: Reviewed as test-only zip use; production truncation paths are guarded. Existing quantization/db tests compare lengths or use equal constructed vectors.

### Deferred P0/P1 Findings
- `SM-AUD-0014` HNSW data format stores dimensions using usize: requires follow-up; no false closure claimed.
- `SM-AUD-0016` Pending HNSW mutations are applied before sidecar save succeeds: requires follow-up; no false closure claimed.
- `SM-AUD-0017` Pending upsert calls insert instead of update: requires follow-up; no false closure claimed.
- `SM-AUD-0019` Graph sidecar validation only checks non-empty file: requires follow-up; no false closure claimed.
- `SM-AUD-0020` HNSW integrity is count-based, not key-level: requires follow-up; no false closure claimed.
- `SM-AUD-0021` FTS integrity count checks use dynamic table names and count parity only: requires follow-up; no false closure claimed.
- `SM-AUD-0024` HNSW filtered search can return empty results without brute-force fallback: requires follow-up; no false closure claimed.
- `SM-AUD-0029` Open-time HNSW rebuild/degrade policy is implicit: requires follow-up; no false closure claimed.
- `SM-AUD-0034` HNSW len can report nonzero even when keymap is empty: requires follow-up; no false closure claimed.
- `SM-AUD-0035` Tombstone overfetch is too naive: requires follow-up; no false closure claimed.
- `SM-AUD-0036` deleted_ratio may divide using graph count that includes unreachable/unmapped points: requires follow-up; no false closure claimed.
- `SM-AUD-0038` insert ignores return/status from hnsw_rs graph.insert: requires follow-up; no false closure claimed.
- `SM-AUD-0043` clear_pending_index_ops is all-or-nothing per processed key list after sidecar save: requires follow-up; no false closure claimed.
- `SM-AUD-0044` q8 optionality conflicts with integrity expectations: requires follow-up; no false closure claimed.
- `SM-AUD-0045` q8 baseline lacks explicit versioned storage envelope: requires follow-up; no false closure claimed.
- `SM-AUD-0047` recency scoring uses wall-clock inside ranking: requires follow-up; no false closure claimed.
- `SM-AUD-0055` Session/channel identifiers are not consistently length/whitespace validated: requires follow-up; no false closure claimed.
- `SM-AUD-0056` Document title/source/metadata size validation is weaker than content validation: requires follow-up; no false closure claimed.
- `SM-AUD-0058` search_episodes drops episode_id and returns document_id: requires follow-up; no false closure claimed.
- `SM-AUD-0061` update_episode_outcome cannot clear experiment_id: requires follow-up; no false closure claimed.
- `SM-AUD-0062` HNSW key parsing split_once(:) is fragile for IDs containing colon: requires follow-up; no false closure claimed.
- `SM-AUD-0063` Message dedup key uses session_id:message_id delimiter: requires follow-up; no false closure claimed.
- `SM-AUD-0064` Correctness depends on PRAGMA foreign_keys being enabled for every pooled connection: requires follow-up; no false closure claimed.
- `SM-AUD-0065` PRAGMA max_page_count computed with dynamic formatting: requires follow-up; no false closure claimed.
- `SM-AUD-0066` Dynamic table_info table name formatting relies on internal callers only: requires follow-up; no false closure claimed.
- `SM-AUD-0067` Dynamic ALTER TABLE formatting relies on internal table/column whitelists: requires follow-up; no false closure claimed.
- `SM-AUD-0068` Dynamic SELECT COUNT table name relies on internal map table list: requires follow-up; no false closure claimed.
- `SM-AUD-0069` SQLite WAL/checkpoint/backpressure policy not visible in archive-level docs: requires follow-up; no false closure claimed.
- `SM-AUD-0070` Connection pool shutdown/poison behavior needs stress coverage: requires follow-up; no false closure claimed.
- `SM-AUD-0071` Projection storage integrity is likely separate from memory integrity: requires follow-up; no false closure claimed.
- `SM-AUD-0072` Projection query uses unwrap_or_default for missing claim/source IDs: requires follow-up; no false closure claimed.
- `SM-AUD-0073` Bridge transform uses unwrap_or_default, potentially hiding malformed optional payloads: requires follow-up; no false closure claimed.
- `SM-AUD-0074` JSON import begins with from_str(...).ok(): requires follow-up; no false closure claimed.
- `SM-AUD-0075` Legacy compatibility serializes with unwrap_or_default: requires follow-up; no false closure claimed.
- `SM-AUD-0076` HTTP embedder response body uses unwrap_or_default on error: requires follow-up; no false closure claimed.
- `SM-AUD-0077` External embedder failure modes need retry/backoff/rate-limit policy: requires follow-up; no false closure claimed.
- `SM-AUD-0312` Dynamic SQL construction should be whitelisted: requires follow-up; no false closure claimed.
- `SM-AUD-0348` INSERT OR IGNORE can hide duplicate/constraint bugs: requires follow-up; no false closure claimed.
- `SM-AUD-0349` INSERT OR IGNORE can hide duplicate/constraint bugs: requires follow-up; no false closure claimed.
- `SM-AUD-0352` INSERT OR IGNORE can hide duplicate/constraint bugs: requires follow-up; no false closure claimed.
- `SM-AUD-0353` Dynamic SQL construction should be whitelisted: requires follow-up; no false closure claimed.
- `SM-AUD-0354` Dynamic SQL construction should be whitelisted: requires follow-up; no false closure claimed.
- `SM-AUD-0358` Dynamic SQL construction should be whitelisted: requires follow-up; no false closure claimed.
- `SM-AUD-0359` Dynamic SQL construction should be whitelisted: requires follow-up; no false closure claimed.
- `SM-AUD-0360` Dynamic SQL construction should be whitelisted: requires follow-up; no false closure claimed.
- `SM-AUD-0375` Dynamic SQL construction should be whitelisted: requires follow-up; no false closure claimed.
- `SM-AUD-0383` Dynamic SQL construction should be whitelisted: requires follow-up; no false closure claimed.
- `SM-AUD-0395` INSERT OR IGNORE can hide duplicate/constraint bugs: requires follow-up; no false closure claimed.
- `SM-AUD-0398` Dynamic SQL construction should be whitelisted: requires follow-up; no false closure claimed.
- `SM-AUD-0399` Dynamic SQL construction should be whitelisted: requires follow-up; no false closure claimed.
- `SM-AUD-0400` Dynamic SQL construction should be whitelisted: requires follow-up; no false closure claimed.
- `SM-AUD-0506` INSERT OR IGNORE can hide duplicate/constraint bugs: requires follow-up; no false closure claimed.
- `SM-AUD-0596` Dynamic SQL construction should be whitelisted: requires follow-up; no false closure claimed.

## Grouped P2/P3 Closure Table
- benchmarks: 1 deferred/static rows retained for follow-up. IDs: SM-AUD-0085
- ci: 1 deferred/static rows retained for follow-up. IDs: SM-AUD-0084
- docs: 25 deferred/static rows retained for follow-up. IDs: SM-AUD-0078, SM-AUD-0079, SM-AUD-0080, SM-AUD-0081, SM-AUD-0082, SM-AUD-0195, SM-AUD-0196, SM-AUD-0197 ...
- error-default: 95 deferred/static rows retained for follow-up. IDs: SM-AUD-0216, SM-AUD-0228, SM-AUD-0229, SM-AUD-0230, SM-AUD-0231, SM-AUD-0297, SM-AUD-0298, SM-AUD-0299 ...
- hashmap-order: 20 deferred/static rows retained for follow-up. IDs: SM-AUD-0314, SM-AUD-0316, SM-AUD-0422, SM-AUD-0423, SM-AUD-0424, SM-AUD-0431, SM-AUD-0432, SM-AUD-0565 ...
- numeric-cast: 110 deferred/static rows retained for follow-up. IDs: SM-AUD-0302, SM-AUD-0303, SM-AUD-0304, SM-AUD-0305, SM-AUD-0306, SM-AUD-0307, SM-AUD-0315, SM-AUD-0317 ...
- permissions: 107 deferred/static rows retained for follow-up. IDs: SM-AUD-0083, SM-AUD-0089, SM-AUD-0090, SM-AUD-0091, SM-AUD-0092, SM-AUD-0093, SM-AUD-0094, SM-AUD-0095 ...
- runtime-clock: 56 deferred/static rows retained for follow-up. IDs: SM-AUD-0215, SM-AUD-0217, SM-AUD-0225, SM-AUD-0226, SM-AUD-0227, SM-AUD-0324, SM-AUD-0328, SM-AUD-0329 ...
- structure: 3 deferred/static rows retained for follow-up. IDs: SM-AUD-0086, SM-AUD-0087, SM-AUD-0088
- unwrap-expect: 80 deferred/static rows retained for follow-up. IDs: SM-AUD-0218, SM-AUD-0219, SM-AUD-0220, SM-AUD-0221, SM-AUD-0222, SM-AUD-0223, SM-AUD-0224, SM-AUD-0232 ...

## Unresolved / Deferred Rationale
This turn did not complete the full 608-finding fix-all contract. Remaining deferred rows are intentionally mapped in JSON instead of being silently ignored. The largest remaining hard buckets are full key-level integrity, full HNSW versioned sidecar format/checksums/startup health policy, projection/bridge validation, API bounds cleanup, root doc restructuring, and broad generated P2/P3 unwrap/cast/clock/hashmap audits.

## Final Acceptance Gate Output
`TMPDIR=/home/sikmindz/tmp CARGO_TARGET_DIR=/home/sikmindz/sm-hermetic-target bash 01_ACCEPTANCE_GATES.sh /home/sikmindz/sm-hermetic` passed and ended with `all semantic-memory acceptance gates passed`.

The requested audit closure definition is still not met because deferred findings remain mapped in `docs/audits/semantic-memory-fix-pass-20260511-findings-status.json`. The `/tmp` extraction attempt was blocked by tmpfs exhaustion, so archive extraction gates were rerun successfully under `/home/sikmindz`.
