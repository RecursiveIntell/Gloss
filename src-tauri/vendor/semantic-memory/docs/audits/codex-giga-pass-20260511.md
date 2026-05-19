# Codex Pass Final Report

## Pass identity

- Pass name: Codex Giga-Pass 2026-05-11, Layers 0-7 plus durable receipt/replay continuation
- Branch/worktree: `master`, dirty workspace at start and end
- Starting commit: `8bf62c552d7201457e78d242439e09594284bdbe`
- Ending commit: `8bf62c552d7201457e78d242439e09594284bdbe` (no commit created)
- Date/time: 2026-05-11T22:27:03-05:00
- Agent/model: Codex / GPT-5

## Objective

Address the first gated slices of the Giga-Pass prompt: Layer 0 ledger setup, Layer 1 HNSW/vector-index truth hardening, Layer 2 deterministic search context plus receipt scaffolding, a focused Layer 3 semantic-boundary defaulting cleanup, Layer 4 codec abstraction, Layer 5 TurboQuant optional-backend prototype, Layer 6 practical receipt/explanation APIs, Layer 7 public framing, and the continuation pass for durable replay-addressable search receipts plus replay verification. TurboQuant remains derived-only and is not default retrieval.

## Summary

This pass hardens HNSW as a rebuildable acceleration artifact, makes search replay-ready, removes the audited semantic-boundary `unwrap_or_default()` sites from the semantic-memory and bridge lanes, installs the codec boundary TurboQuant needs, and adds durable replay-addressable search receipts. The sidecar now uses a semantic-memory-owned fixed-width, versioned header instead of trusting upstream native bytes; unsupported versions and wrong dimensions fail closed and rebuild from SQLite. Full integrity verification now checks live SQLite embedding keys against active `hnsw_keymap` rows and the in-memory node vectors, catching count-equal corruption such as swapped node IDs. Search now supports explicit `SearchContext` evaluation times, optional `VectorSearchReceiptV1` metadata, product-facing receipt answers, per-result "why this result" answers, and `replay_search_receipt()` comparison reports. V18 adds a metadata-only `search_receipts` table with versioned stored receipt JSON, fixed-width count conversion, receipt digests, and fail-closed duplicate-ID conflict handling. Boundary diagnostics now fail visibly instead of collapsing failed serialization or Ollama error-body reads into empty strings, and projection optional IDs remain explicit `None` values in query results. `VectorCodecProfileV1`/`VectorArtifactV1` plus raw f32, SQ8, and feature-gated TurboQuant codecs now provide a fail-closed derived-artifact seam without making compressed vectors authoritative.

## Files changed

| File | Change type | Notes |
|---|---|---|
| `src/hnsw.rs` | hardening | Added versioned fixed-width sidecar read/write, retained node vectors for replay, graph/data header validation, filtered under-return fallback signal. |
| `src/hnsw_ops.rs` | hardening | On pending sidecar sync failure, discard mutated in-memory state and rebuild or disable HNSW from SQLite. |
| `src/lib.rs` | hardening/API | Rebuilds corrupt/unsupported sidecars on open, extends `verify_integrity(Full)` to key-level HNSW parity, exposes context-aware search APIs, persists returned receipts, exposes `get_search_receipt()`, and adds `replay_search_receipt()`. |
| `src/db.rs` | persistence | Adds V18 `search_receipts` table plus versioned receipt storage/load helpers with fixed-width count conversion and duplicate digest checks. |
| `src/search.rs` | hardening/API | Threads explicit evaluation time through RRF/recency scoring and emits receipt-ready search execution metadata; marks approximate HNSW receipts. |
| `src/types.rs` | API | Adds `SearchContext`, `ReceiptMode`, `ExactnessProfile`, `VectorSearchReceiptV1`, context-aware response types, codec/profile receipt metadata, product-facing receipt answers, replay reports, and per-result explanation answers. |
| `src/error.rs` | API | Adds `VectorCodecProfileMismatch` for fail-closed artifact/profile mismatch, `SearchReceiptConflict` for duplicate durable receipt IDs with different payloads, and `SearchReceiptNotFound` for replay of absent receipts. |
| `Cargo.toml` / `../Cargo.lock` | dependency | Adds optional `turbo-quant-codec` feature and path dependency on `../turbo-quant`. |
| `src/embedder.rs` | boundary cleanup | Preserves non-2xx Ollama error-body read failures as explicit diagnostics instead of defaulting to an empty body. |
| `src/projection_legacy_compat.rs` | boundary cleanup | Converts legacy episode JSON serialization defaults into typed `ImportInvalid` errors. |
| `src/projection_storage_query.rs` | boundary cleanup | Replaces audited optional-ID defaulting with explicit optional search-token policy while preserving `None` result fields. |
| `src/vector_codec.rs` | codec abstraction | Adds `VectorCodecProfileV1`, `VectorArtifactV1`, object-safe `VectorCodec`, `RawF32Codec`, `Sq8Codec`, and feature-gated `TurboQuantCodec`. |
| `../forge-memory-bridge/src/transform.rs` | boundary cleanup | Replaces bridge optional vector defaults with explicit `map_or_else(Vec::new, ...)` policy at receipt/control-plane reference boundaries. |
| `../turbo-quant/src/kv.rs` | lint cleanup | Fixes doc-list indentation needed for `-D warnings` when the optional backend is enabled. |
| `../turbo-quant/src/qjl.rs` | correctness | Stores source-vector norm in `QjlSketch` and uses the correct unbiased QJL inner-product scale. |
| `../turbo-quant/src/turbo.rs` | lint/test cleanup | Replaces manual bit-range check with inclusive range check and uses a genuinely compressive dimension in batch stats coverage. |
| `README.md` | docs | Leads with trustworthy local-first AI memory, receipts, replay, source grounding, approximation visibility, and rebuildability. |
| `07_PUBLIC_STORY_AND_RELEASE.md` | docs | Updates public pitch and release checklist to reflect closed receipt/vector/codec gates. |
| `tests/hnsw_persistence.rs` | tests | Added HNSW integrity corruption and sidecar version/dimension rebuild tests. |
| `tests/search_tests.rs` | tests | Added deterministic recency, search receipt, and filtered HNSW fallback tests. |
| `tests/db_tests.rs` | tests | Added Ollama HTTP diagnostic preservation coverage. |
| `tests/projection_v11_tests.rs` | tests | Added assertions that missing projection relation/evidence IDs stay absent in public query results. |
| `tests/vector_codec.rs` | tests | Added stable digest, round-trip, SQ8 artifact, TurboQuant artifact, profile mismatch rejection, and fixed-corpus drift coverage. |
| `docs/audits/codex-giga-pass-20260511.md` | ledger | Human-readable pass report. |
| `docs/audits/codex-giga-pass-20260511-status.json` | ledger | Machine-readable pass status. |

## Tests added or changed

| Test | Purpose | Expected failure before fix |
|---|---|---|
| `full_integrity_reports_missing_live_hnsw_key` | Detect a live SQLite embedding missing from active keymap. | Count drift only, no key-level evidence. |
| `full_integrity_reports_stale_or_wrong_domain_hnsw_key` | Detect unsupported key domains. | Wrong-domain active rows could evade count-only checks. |
| `full_integrity_reports_stale_valid_domain_hnsw_key` | Detect valid-domain keymap entry pointing to no live row. | Count-only checks could miss or under-specify stale keys. |
| `full_integrity_catches_swapped_hnsw_key_ids_when_counts_match` | Catch swapped node IDs while active counts remain equal. | Count parity would pass. |
| `unsupported_hnsw_sidecar_version_rebuilds_from_sqlite_on_reopen` | Reject unsupported sidecar header versions and rebuild. | Old/native sidecar bytes could be trusted or fail into empty HNSW. |
| `wrong_hnsw_sidecar_dimension_rebuilds_from_sqlite_on_reopen` | Reject sidecar dimension mismatch and rebuild. | Dimension mismatch handling was tied to unversioned upstream data bytes. |
| `recency_is_deterministic_for_same_search_context_time` | Verify repeated searches with the same `evaluation_time` produce identical recency scores. | Recency used wall clock inside scoring helpers. |
| `recency_changes_with_different_search_context_times` | Verify recency changes predictably when replay time changes. | No explicit replay time existed. |
| `context_search_receipt_records_exact_backend_and_result_ids` | Verify receipt backend, exactness, request ID, and result IDs. | No receipt surface existed. |
| `context_search_receipt_records_exact_backend_and_result_ids` additions | Verify product-facing receipt answers expose replay ID, exactness, replay/rebuild readiness, backend, result count, and durable receipt lookup. | Receipt callers had to infer practical answers from raw metadata and no durable lookup existed. |
| `explained_result_answer_names_source_and_score_lanes` | Verify per-result explanation answer names the source row and text/vector score lanes. | No typed "why this result" surface existed. |
| `durable_receipt_id_conflict_fails_closed` | Verify reusing a receipt ID for different receipt payload bytes returns `search_receipt_conflict`. | Duplicate receipt IDs could not be represented or protected. |
| `durable_receipt_replay_matches_original_inputs` | Verify replay with matching query/filter inputs reproduces query digest and result IDs and leaves a replay receipt. | Durable receipts could be loaded but not replay-checked. |
| `durable_receipt_replay_detects_wrong_query` | Verify replay with different query text reports digest and result drift. | Replay drift could not be measured. |
| `replay_missing_receipt_id_fails_closed` | Verify replay of an absent receipt returns `search_receipt_not_found`. | Missing replay targets had no typed error. |
| `unsupported_durable_receipt_schema_version_fails_closed` | Verify unsupported durable receipt versions are rejected explicitly. | No versioned durable search receipt format existed. |
| `filtered_hnsw_underreturn_records_exact_fallback_receipt` | Verify filtered HNSW under-return falls back to exact search and records degradation. | Fallback was not visible in returned metadata. |
| `test_ollama_http_error_preserves_body_read_failure` | Verify failed error-body reads remain visible in Ollama diagnostics. | Body read failures were converted to an empty string. |
| `public_projection_queries_read_imported_rows` additions | Verify missing optional relation/evidence IDs remain `None`. | Optional IDs could be hidden behind empty search-token defaults in query mapping. |
| `raw_profile_digest_is_stable_and_identity_sensitive` | Verify profile digests are stable and change with profile identity. | No codec profile identity existed. |
| `raw_f32_codec_round_trips_exactly` | Verify the raw reference codec preserves f32 bytes exactly. | No reference codec boundary existed. |
| `sq8_codec_round_trips_with_profile_identity` | Verify SQ8 wraps the existing q8 path with profile metadata. | Existing q8 bytes had no codec artifact/profile surface. |
| `profile_mismatch_fails_closed` | Verify decoding with the wrong codec profile is rejected. | Profile mismatch could not be represented or rejected. |
| `artifact_profile_digest_tampering_fails_closed` | Verify artifact/profile digest tampering is rejected. | Artifact profile integrity could not be checked. |
| `turbo_quant_codec_is_deterministic_for_same_profile` | Verify same profile and vector produce identical TurboQuant artifact digests. | No TurboQuant codec backend existed. |
| `turbo_quant_seed_changes_code_digest` | Verify seed is part of profile/code identity. | TurboQuant profile identity was not represented. |
| `turbo_quant_wrong_profile_rejects_scoring` | Verify TurboQuant scoring fails closed on profile mismatch. | Wrong-profile scoring could not be tested. |
| `turbo_quant_fixed_corpus_drift_harness_reports_metrics` | Emits fixed-corpus recall/rank/score/storage drift metrics. | No raw-vs-TurboQuant differential harness existed. |

## Commands run

| Command | Result | Notes |
|---|---|---|
| `cargo fmt --all --check` | pass | Baseline and final pass. |
| `cargo check --workspace --all-targets --all-features` | pass | Re-run after Layer 5; pre-existing dead-code warnings in sibling property tests. |
| `cargo test -p semantic-memory --all-features --test hnsw_persistence -- --nocapture` | pass | 11 HNSW persistence/integrity tests passed. |
| `cargo test -p semantic-memory --all-features --test search_tests -- --nocapture` | pass | Re-run after replay API: 51 search tests passed, including durable lookup, duplicate-ID conflict, replay match/drift, missing receipt, and unsupported receipt version cases. |
| `cargo test -p semantic-memory --all-features` | pass | Re-run after replay API: full semantic-memory package suite and doc-tests passed with TurboQuant feature enabled. An earlier run failed only because `/tmp` was full during doctest linking; stale `/tmp` Cargo build targets were removed and the command then passed. |
| `cargo test -p semantic-memory --all-features --test db_tests --test projection_v11_tests -- --nocapture` | pass | 14 db tests and 17 projection tests passed. |
| `cargo test -p semantic-memory --all-features --test db_tests --test projection_v11_tests --test search_tests -- --nocapture` | pass | Re-run after clippy cleanups: 14 db, 17 projection, and 45 search tests passed. |
| `cargo test -p semantic-memory --all-features --test vector_codec -- --nocapture` | pass | 9 codec tests passed; corrected TurboQuant drift metrics: recall@5=1.000, mean_rank_drift=0.792, mean_abs_score_error=0.076, p95_abs_score_error=0.152, storage_bytes_per_vector=393.3. |
| `cargo check -p semantic-memory --all-targets --no-default-features --features brute-force` | pass | Brute-force/no-default package check passed. |
| `cargo check -p semantic-memory --all-targets --features turbo-quant-codec` | pass | Optional TurboQuant feature check passed. |
| `cargo test -p semantic-memory --no-default-features --features brute-force --test vector_codec -- --nocapture` | pass | Raw/SQ8 codec tests pass without default HNSW. |
| `cargo test -p semantic-memory --features turbo-quant-codec --test vector_codec -- --nocapture` | pass | Feature-gated TurboQuant codec tests pass. |
| `cargo test -p forge-memory-bridge --all-features` | pass | 27 unit tests, 17 integration tests, and doc-tests passed. |
| `cargo clippy -p semantic-memory --lib --all-features -- -D warnings` | pass | semantic-memory plus optional TurboQuant dependency lint clean after local turbo-quant lint cleanup. |
| `cargo check -p semantic-memory --all-targets --all-features` | pass | Re-run after Layer 6 public receipt/explanation API changes. |
| `cargo clippy -p semantic-memory --lib --all-features -- -D warnings` | pass | Re-run after Layer 6 public receipt/explanation API changes. |
| `cargo fmt --all --check` | pass | Re-run after durable receipt persistence. |
| `cargo check -p semantic-memory --all-targets --all-features` | pass | Re-run after replay API. |
| `cargo clippy -p semantic-memory --lib --all-features -- -D warnings` | pass | Re-run after replay API. |
| `cargo clippy -p semantic-memory --all-targets --all-features -- -D warnings` | fail | Still blocked by pre-existing `expect_used` violations in semantic-memory tests; first reported files include `tests/step4_verification.rs`, `tests/step3_verification.rs`, `tests/knowledge_tests.rs`, `tests/import_boundary_tests.rs`, and `tests/import_ugly_cases.rs`. |
| `cargo doc -p semantic-memory --all-features --no-deps` | pass | Re-run after durable receipts; warning only: pre-existing redundant explicit link target in crate docs. |
| `cargo test -p turbo-quant` | pass | 42 unit tests, 12 integration tests, and 2 doc-tests passed after QJL scale and batch stats fixes. |
| `cargo test --workspace --all-features` | fail | Re-run after TurboQuant fixes; still fails in unrelated `contract-schema-gen`: schema drift for `schemas/verification-case-v1.schema.json`. |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | fail | Pre-existing `expect_used`/`expect_err` test violations in sibling crates. |
| `cargo clippy -p semantic-memory --all-targets --all-features -- -D warnings` | fail | New library lints fixed; command now stops on pre-existing `expect_used` violations in semantic-memory tests. |
| `cargo doc --workspace --all-features --no-deps` | pass | Warnings only: redundant/private rustdoc links in pre-existing docs. |
| `cargo doc -p semantic-memory --all-features --no-deps` | pass | Re-run after Layer 5; warning only: pre-existing redundant explicit link target in crate docs. |
| `cargo doc -p semantic-memory -p forge-memory-bridge --all-features --no-deps` | pass | Warning only: pre-existing redundant explicit link target in semantic-memory crate docs. |

## Issue closure

| Issue ID | Status | Evidence |
|---|---|---|
| F-001 key-level parity | fixed for HNSW full integrity | Added full integrity comparison across live SQLite keys, active keymap keys, and node vectors. |
| F-002 sidecar validation | fixed for semantic-memory sidecar | Unsupported header versions, wrong dimensions, graph/data mismatch, truncation, and trailing bytes fail closed. |
| F-003 fixed-width sidecar dimensions | fixed for semantic-memory sidecar | Header persists `u32` dimensions and `u64` vector count; no semantic-memory-owned `usize` sidecar field. |
| F-004 filtered under-return fallback | fixed for context-aware APIs | HNSW post-filter under-return with active filters runs exact fallback and records `hnsw_filtered_underreturn_fallback` in the receipt. |
| F-005 pending-op failure recovery | partial | Failed pending sidecar sync now discards mutated in-memory HNSW and rebuilds/disables from SQLite; no injected save-failure test seam added yet. |
| F-006 replay-clean recency | fixed | RRF recency scoring uses `SearchContext.evaluation_time`; old APIs capture default context once. |
| F-009 search receipt skeleton | fixed | Added `VectorSearchReceiptV1` and context-aware search response APIs. |
| GP-009 boundary defaulting can invent meaning | fixed for audited semantic-memory/bridge sites | Removed semantic-boundary `unwrap_or_default()` from Ollama diagnostics, legacy projection episode serialization, projection optional-ID search text, and bridge receipt/control-plane reference handling. |
| SM-AUD-0072 projection optional ID defaulting | fixed | Query results preserve `None` for absent optional IDs; search-token defaults are explicit and local to text matching. |
| SM-AUD-0075 legacy compatibility serialization defaulting | fixed | Serialization failures now return `ImportInvalid` instead of empty JSON strings. |
| SM-AUD-0076 embedder response body defaulting | fixed | Non-2xx Ollama response body read failures are reported explicitly. |
| Layer 4 codec abstraction | fixed | Added byte-oriented object-safe `VectorCodec`, `VectorCodecProfileV1`, `VectorArtifactV1`, `RawF32Codec`, and `Sq8Codec`. |
| Layer 4 stable profile digest | fixed | `VectorCodecProfileV1::digest()` has stable tests and changes when profile identity changes. |
| Layer 4 profile mismatch rejection | fixed | Codec decode rejects wrong or tampered profile digests with `vector_codec_profile_mismatch`. |
| Layer 5 optional TurboQuant feature | prototype | Added `turbo-quant-codec` optional dependency and feature-gated `TurboQuantCodec`. |
| Layer 5 TurboQuant artifact bytes | prototype | TurboCode is encoded as versioned/profiled `VectorArtifactV1` bytes using serde JSON. |
| Layer 5 raw-vs-TurboQuant drift harness | prototype | Fixed-corpus test reports recall@5, mean rank drift, mean/p95 score error, and storage bytes per vector. |
| Layer 5 TurboQuant profile mismatch | fixed | Wrong seed/profile scoring fails closed with `vector_codec_profile_mismatch`. |
| TurboQuant QJL estimator | fixed | `QjlSketch` now stores `||x||` and estimator scale is `sqrt(π/2) * ||x|| / m`; upstream TurboQuant tests pass. |
| Layer 6 receipt UX/API | fixed | `VectorSearchReceiptV1::answers()` exposes replay ID, backend, codec profile, exactness, fallback/degradation, replay/rebuild readiness, and result IDs. |
| Layer 6 why-this-result API | fixed | `ExplainedResult::answer()` exposes source kind/source ID and text/vector/recency/rerank reasons per result. |
| Layer 7 public framing | fixed | README/public story now lead with trustworthy local-first AI memory, receipts, replay, source grounding, compressed retrieval readiness, and rebuildability. |
| Durable search receipts | fixed | V18 `search_receipts` persists versioned receipt metadata by receipt ID and `MemoryStore::get_search_receipt()` loads it for replay/audit surfaces. |
| Durable receipt format versioning | fixed | Stored receipt rows carry `vector_search_receipt_v1`; unsupported versions return `corrupt_data`. |
| Durable receipt duplicate IDs | fixed | Reusing a receipt ID with different payload bytes returns `search_receipt_conflict`; identical payload digest is idempotent. |
| Replay receipt verification | fixed | `MemoryStore::replay_search_receipt()` reruns the recorded search family with stored evaluation time and caller-supplied query/filter inputs, then reports query digest and result-ID drift. |

## Receipt/degradation behavior

Context-aware search APIs can return `VectorSearchReceiptV1` when `receipt_mode` is `ExplainOnly` or `ReturnReceipt`. Receipts include evaluation time, query embedding digest, search profile, candidate backend, optional codec family/profile digest, approximate status, requested/returned/post-filter candidate counts, fallback, exact rerank, result IDs, and degradations. `VectorSearchReceiptV1::answers()` turns those fields into product-facing replay/source/approximation/rebuild answers. `ExplainedResult::answer()` answers why an individual result appeared and which authoritative source row it came from. When a receipt is produced, it is now persisted in `search_receipts` as versioned metadata and can be loaded by `MemoryStore::get_search_receipt(receipt_id)`. `MemoryStore::replay_search_receipt()` reruns the recorded hybrid/vector-only search family with the stored evaluation time, a fresh replay receipt ID, and caller-supplied query/filter inputs; it reports query embedding digest match, result-ID order match, missing IDs, and added IDs. The stored JSON uses fixed-width count conversion rather than persisting public `usize` fields. Filtered HNSW under-return records `hnsw_filtered_underreturn_fallback`. TurboQuant artifacts expose profile digests and encoded digests, but TurboQuant is not yet wired into live retrieval receipts.

## Authority impact

- [x] No derived artifact became authoritative truth.
- [x] Raw/reference path remains available where required.
- [x] Sidecars/indexes remain rebuildable.
- [x] Boundary defaults are explicit where touched.

## TurboQuant eligibility

Prototype implemented; not default retrieval eligible.

Rationale: HNSW P0 integrity, Layer 2 receipt scaffolding, focused Layer 3 boundary cleanup, Layer 4 codec profile/artifact mismatch gates, and an optional `TurboQuantCodec` prototype are in place. TurboQuant remains a derived artifact backend only. It is not default-retrieval eligible until broader recall/rank/score drift gates are accepted and retrieval receipts include live TurboQuant profile/degradation evidence.

## Remaining risks

- HNSW pending save-failure behavior lacks an injectable failure test seam.
- Full workspace tests are blocked by unrelated `contract-schema-gen` schema drift.
- Workspace clippy is blocked by pre-existing test `expect()` policy violations.
- Replay verification requires caller-supplied query text and filters because receipts intentionally do not store query text/filter payloads.
- TurboQuant backend is implemented only as an optional codec, not wired into search/index persistence.
- Drift harness is a fixed test fixture, not a benchmark suite.
- TurboQuant retrieval receipts are not emitted from the live search path.
- Layer 3 was focused on audited semantic-boundary defaulting sites; remaining non-boundary `unwrap_or_default()` calls were left alone.

## Next pass recommendation

Run the next pass to wire optional TurboQuant artifacts into retrieval receipts behind `VectorCodec` with exact raw fallback and accepted drift thresholds. Add receipt retention/pruning policy before treating durable receipts as production storage. Do not make TurboQuant default retrieval.
