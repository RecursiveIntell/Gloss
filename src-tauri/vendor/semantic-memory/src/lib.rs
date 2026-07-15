// MEM-011 fix: removed broad crate-level allows for unused_imports,
// unused_variables, and unreachable_code. These hide dead feature paths
// and overstate clippy-green signal. Individual modules may add targeted
// allows with issue IDs where necessary.
#![allow(deprecated)]
#![allow(
    clippy::bool_assert_comparison,
    clippy::collapsible_if,
    clippy::empty_line_after_doc_comments,
    clippy::expect_used,
    clippy::field_reassign_with_default,
    clippy::if_same_then_else,
    clippy::iter_cloned_collect,
    clippy::let_and_return,
    clippy::manual_div_ceil,
    clippy::manual_pattern_char_comparison,
    clippy::manual_range_contains,
    clippy::manual_slice_size_calculation,
    clippy::manual_unwrap_or_default,
    clippy::needless_range_loop,
    clippy::ptr_arg,
    clippy::redundant_closure,
    clippy::skip_while_next,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::unnecessary_cast,
    clippy::unnecessary_sort_by
)]

//! # semantic-memory
//!
//! Local-first semantic memory backed by authoritative SQLite state and an optional recoverable
//! HNSW sidecar.
//!
//! The crate stores facts, chunked documents, conversation messages, and searchable episodes in
//! SQLite. Search combines BM25 (FTS5) and vector retrieval with Reciprocal Rank Fusion, and
//! `search_explained()` returns the exact scoring breakdown from the live pipeline.
//!
//! Concurrency uses one writer connection plus a pool of WAL-enabled reader connections.
//! Durable writes are committed to SQLite first; any required HNSW sidecar mutations are journaled
//! in SQLite and replayed on open, flush, rebuild, or reconcile.
//!
//! `search()` targets facts, document chunks, and episodes by default. Message retrieval is
//! available through `search_conversations()` or by opting into
//! [`SearchSourceType::Messages`].
//!
//! Integrity tooling is strict about malformed stored data: invalid roles, JSON, enums, embedding
//! blobs, quantized blobs, and sidecar drift are surfaced through `verify_integrity()` instead of
//! being silently converted into defaults. `reconcile()` can rebuild FTS or fully re-embed and
//! rebuild derived state from SQLite.
//!
//! `store.graph_view()` exposes a deterministic graph traversal layer over namespaces, facts,
//! documents, chunks, sessions, messages, episodes, and semantic/temporal/causal links derived
//! from SQLite state.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use semantic_memory::{MemoryConfig, MemoryStore};
//!
//! # async fn example() -> Result<(), semantic_memory::MemoryError> {
//! let store = MemoryStore::open(MemoryConfig::default())?;
//!
//! // Store a fact
//! store.add_fact("general", "Rust was first released in 2015", None, None).await?;
//!
//! // Search
//! let results = store.search("when was Rust released", None, None, None).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Operational Notes
//!
//! - SQLite is authoritative for all durable records and embeddings.
//! - HNSW is an acceleration sidecar. Pending sidecar mutations are journaled in SQLite, so a
//!   sidecar failure does not imply the SQLite write rolled back.
//! - WAL mode plus pooled reader connections allows concurrent reads while writes serialize through
//!   the writer connection.
//! - `search_explained()` reflects the exact ranking math used by the active search pipeline,
//!   including reranking from exact f32 cosine similarity when configured.

// At least one search backend must be enabled.
#[cfg(not(any(feature = "hnsw", feature = "brute-force", feature = "usearch-backend")))]
compile_error!(
    "At least one search backend feature must be enabled: 'hnsw', 'usearch-backend', or 'brute-force'"
);

#[cfg(all(feature = "hnsw", feature = "usearch-backend"))]
compile_error!("Cannot enable both 'hnsw' and 'usearch-backend' features simultaneously");

mod authority;
pub mod authority_contracts;
pub mod chunker;
pub mod config;
pub(crate) mod conversation;
pub(crate) mod db;
/// Bounded evidence-gap retrieval and state-aware reranking over existing authority/search paths.
pub mod evidence_gap;
mod forgetting;
mod procedural_memory;
pub mod transition_contracts;
mod transition_verifier;
pub use db::{bytes_to_embedding, decode_f32_le, embedding_to_bytes};
pub use evidence_gap::{
    rerank_state_aware, EvidenceAblationReceiptV1, EvidenceGapOutcomeV1, EvidenceGapReasonV1,
    EvidenceGapRequestV1, EvidenceGapV1, EvidencePacketItemV1, EvidencePacketV1,
    EvidenceRetrievalRouteV1, EvidenceRouteReceiptV1, EvidenceTerminalOutcome,
    EvidenceTerminalOutcomeV1, StateRerankCandidateV1, StateRerankWeightsV1, EVIDENCE_GAP_V1,
    EVIDENCE_PACKET_V1, EVIDENCE_ROUTE_RECEIPT_V1,
};
/// Archived pure-Rust implementations replaced by C kernels.
#[allow(dead_code)]
pub mod archive;
/// Phase 9b: benchmark harness for routing quality.
#[cfg(feature = "benchmark")]
pub mod benchmark;
/// Leiden community detection with contradiction tracking.
#[cfg(feature = "community")]
pub mod community;
/// Phase 8: simplified compression governor (importance scoring only).
#[cfg(feature = "compression-governor")]
pub mod compression_governor;
/// Content-based contradiction detection (lexical, deterministic).
#[cfg(feature = "decoder")]
pub mod contradiction_detect;
/// Phase 6: decoder architecture (syndromes and corrections).
#[cfg(feature = "decoder")]
pub mod decoder;
/// Discord-structured second-order retrieval (graph-neighbour discovery).
#[cfg(feature = "discord")]
pub mod discord;
pub(crate) mod documents;
pub mod embedder;
pub(crate) mod episodes;
pub mod error;
/// Contradiction-detection evaluation harness (RAMDocs-style P/R/F1).
#[cfg(feature = "decoder")]
pub mod eval_contradiction;
/// Factor graph unification of heterogeneous graph edges (semantic,
/// temporal, causal, entity) with belief propagation. The single most
/// novel combination: unified probabilistic reasoning over all edge types.
#[cfg(feature = "integration")]
pub mod factor_graph;
mod graph;
/// First-class stored graph edges (durable, typed relationships).
pub(crate) mod graph_edges;
#[cfg(feature = "hnsw")]
pub mod hnsw;
#[cfg(feature = "hnsw")]
mod hnsw_backend;
#[cfg(feature = "hnsw")]
mod hnsw_ops;
/// Claim-bounded scoring and receipt invariants for the hostile memory benchmark.
pub mod hostile_benchmark;

/// Deterministic CPU-only hubness scoring over dense embedding collections.
pub mod hubness;
/// Phase 10: cross-feature integration wiring.
#[cfg(feature = "integration")]
pub mod integration;
mod json_compat_import;
pub(crate) mod knowledge;
/// Immutable origin-bound authority labels and governed access decisions.
pub mod origin_authority;
pub use authority::MemoryAuthority;
pub use authority_contracts::{
    AuthorityAdmission, AuthorityFaultStage, AuthorityOperationKind, AuthorityPermit,
    AuthorityReceiptV1, AuthoritySnapshotId, AuthorityStateV1, CapabilityManifestV1, Confidence,
    CosineSimilarity, InjectionDecisionV1, InjectionDisposition, MemoryEnvelopeV1,
    NonNegativeWeight, Probability, RetrievalEpoch, RetrievalResponseV1, RetrievalWitnessV1,
    StageOutcomeV1, SupersessionReceiptV1,
};
pub use forgetting::{
    ForgettingClosureReceiptV1, ForgettingClosureRequestV1, ForgettingDispositionV1,
    ForgettingEpochsV1, ForgettingSurfaceRefV1, ForgettingVerificationV1,
    FORGETTING_CLOSURE_RECEIPT_V1,
};
pub use knowledge::StateView;
pub use origin_authority::{
    evaluate_governed_access_v1, AudienceV1, AuthorityScopeV1, AuthorityScopesV1,
    CallerPrincipalV1, DelegationElevationLeaseV1, ElevationRequirementV1, GovernedAccessPurposeV1,
    GovernedAccessRequestV1, GovernedFactAccessV1, GovernedFactListResponseV1,
    GovernedGraphResponseV1, GovernedProjectionResponseV1, GovernedReplayResponseV1,
    GovernedSearchResponseV1, GovernedStateResolutionResponseV1, NamespaceScopeV1,
    OriginAuthorityDecisionV1, OriginAuthorityLabelV1, OriginAuthorityRecordV1, OriginClassV1,
    OriginDerivationKindV1, OriginRiskV1, PolicyDecisionV1, RevocationStatusV1, SubjectPrincipalV1,
};
pub use procedural_memory::{
    validate_procedure_artifact_v1, verify_procedure_lifecycle_receipt_v1,
    verify_procedure_test_receipt_v1, AllowedProcedureToolV1, ApplicabilityOperatorV1,
    ApplicabilityPredicateV1, GovernedProcedureDecisionV1, GovernedProcedureRetrievalV1,
    ProceduralMemoryArtifactV1, ProcedureAccessPathV1, ProcedureActionPermitV1, ProcedureActionV1,
    ProcedureCapabilityV1, ProcedureEffectV1, ProcedureEvidenceTestEnvelopeV1,
    ProcedureFixtureReceiptV1, ProcedureFixtureV1, ProcedureLifecycleDispositionV1,
    ProcedureLifecyclePermitV1, ProcedureLifecycleReceiptV1, ProcedurePreconditionV1,
    ProcedureRetrievalRequestV1, ProcedureRevocationV1, ProcedureRiskV1, ProcedureStepV1,
    ProcedureTestReceiptV1, ProcedureValidationV1, PROCEDURAL_MEMORY_ARTIFACT_V1,
    PROCEDURE_LIFECYCLE_RECEIPT_V1, PROCEDURE_TEST_RECEIPT_V1,
};
pub use shadow_policy::{
    compare_shadow_execution_v1, evaluate_shadow_policy_promotion_v1, shadow_policy_digest,
    ActiveShadowPolicyV1, PromotionDecisionReceiptV1, PromotionDispositionV1, PromotionEvidenceV1,
    PromotionGateDecisionV1, ShadowEvaluationWindowV1, ShadowExecutionComparisonV1,
    ShadowPolicyKindV1, ShadowPolicyPromotionPermitV1, ShadowPolicyProposalV1,
    ShadowPolicyProvenanceV1, ShadowPolicyRiskV1, ShadowPolicyStatusV1,
    PROMOTION_DECISION_RECEIPT_V1, SHADOW_POLICY_PROPOSAL_V1,
};
pub use state_epistemics::{
    answer_policy_for, resolve_dependency_states, AnswerDisposition, AnswerPolicy,
    AnswerPolicyDecision, BeliefAlternativeV1, DependencyResolutionV1, DependencyState,
    PremiseStatus, ResolvedAssertionV1, ResolvedMemoryAnswerV1, StateDependencyEdgeV1,
    StateResolutionMode, StateResolutionReceiptV1, StateResolvedRetrievalResponseV1,
    STATE_RESOLUTION_RECEIPT_V1, STATE_RESOLVED_RETRIEVAL_V1,
};
pub use transition_contracts::{
    ActiveHeadSimulationV1, AssertionDraftV1, DependencySimulationV1, MemoryTransitionCandidateV1,
    MemoryTransitionOutcomeV1, MemoryTransitionRecordV1, MemoryTransitionVerificationV1,
    OmittedSourceSpanV1, SourceArtifactV1, SourceSpanRefV1, SupersessionDraftV1,
    TransitionDisposition, TransitionOperation, UnsupportedAssertionSpanV1, VerificationScore,
};
/// ColBERT-style late interaction multi-vector retrieval.
#[cfg(feature = "late-interaction")]
pub mod late_interaction;
/// Matryoshka Representation Learning: multi-resolution embedding truncation.
#[cfg(feature = "matryoshka")]
pub mod matryoshka;
/// Multiscale retrieval scheduling pipeline (staged search with budgets).
#[cfg(feature = "multiscale")]
pub mod pipeline;
/// Compatibility-only legacy import surface.
///
/// This module exists only for migration compatibility with pre-V11 import paths.
#[deprecated(
    since = "0.6.0",
    note = "Legacy V10 import path is migration-only. Use `import_projection_batch()` with `ProjectionImportBatchV3` on the canonical lane."
)]
#[doc(hidden)]
#[cfg(feature = "poly-kv-codec")]
pub mod poly_kv_bridge;
mod pool;
mod projection_batch;
mod projection_derivation;
pub mod projection_import;
mod projection_lane;
mod projection_legacy_compat;
pub(crate) mod projection_storage;
/// Phase 2: semiring provenance (Boolean/Tropical/Probability/Confidence).
#[cfg(feature = "provenance")]
pub mod provenance;
pub mod quantize;
pub mod quantize_governed;
/// Contextual reinstatement scoring building blocks.
pub mod reinstatement;
/// RL-trained retrieval routing on receipt replay data.
#[cfg(feature = "rl-routing")]
pub mod rl_routing;
/// Phase 9: adaptive retrieval routing (query-aware stage selection).
#[cfg(feature = "routing")]
pub mod routing;
pub mod search;
pub mod shadow_policy;
pub mod state_epistemics;
pub mod storage;
mod store_support;
/// Reasoning subgraph pruning with lawful subtraction.
#[cfg(feature = "subgraph-pruning")]
pub mod subgraph_pruning;
/// Phase 7: lawful subtraction engine.
#[cfg(feature = "subtraction")]
pub mod subtraction;
/// Phase 3: temporal field provenance (computed temporal_weight scores).
#[cfg(feature = "temporal")]
pub mod temporal;
pub mod tokenizer;
/// Persistent homology and topological void detection for knowledge graphs.
#[cfg(feature = "topology")]
pub mod topology;
pub mod types;
#[cfg(feature = "usearch-backend")]
mod usearch_backend;
pub mod vector_backend;
pub mod vector_codec;
pub mod vector_snapshot;

// Re-export primary public types.
pub use config::{
    ChunkingConfig, ChunkingStrategy, DerivedVectorBackendPolicy, EmbeddingConfig, MemoryConfig,
    MemoryLimits, PoolConfig, SearchConfig,
};
pub use db::{IntegrityReport, ReconcileAction, VerifyMode};
#[cfg(feature = "candle-embedder")]
pub use embedder::CandleEmbedder;
pub use embedder::{
    BgeM3DeriveConfig, BgeM3Embedder, EmbedBatchFuture, EmbedFuture, Embedder, MockEmbedder,
    MultiEmbedBatchFuture, MultiEmbedFuture, MultiFunctionEmbedder, MultiFunctionEmbedding,
    MultiVectorEmbedding, OllamaEmbedder, OptionalMultiEmbedBatchFuture, OptionalMultiEmbedFuture,
    SparseWeights,
};
pub use error::MemoryError;
#[cfg(feature = "hnsw")]
pub use hnsw::{HnswConfig, HnswHit, HnswIndex};
// Type aliases for the new VectorBackend trait. The Hnsw* names are kept
// for source compatibility; new code should prefer the Vector* names.
pub use graph_edges::{AddGraphEdgeParams, StoredGraphEdge};
pub(crate) use projection_lane::projection_import_failure_id;
pub use projection_lane::{
    ProjectionImportFailureReceiptEntry, ProjectionImportLogEntry, ProjectionImportResult,
};
pub use quantize::{pack_quantized, unpack_quantized, QuantizedVector, Quantizer};
pub use storage::StoragePaths;
pub use tokenizer::{EstimateTokenCounter, TokenCounter};
pub use types::{
    ChunkManifestChunkMapping, ChunkManifestEntry, ChunkManifestIngestOptions,
    ChunkManifestIngestResult, DerivedCandidateReceiptV1, Document, EmbeddingDisplacement,
    EpisodeAsOfReceiptV1, EpisodeMeta, EpisodeOutcome, ExactnessProfile, ExplainedResult,
    ExplainedResultAnswerV1, ExplainedSearchResponse, Fact, GraphDirection, GraphEdge,
    GraphEdgeType, GraphView, MemoryStats, Message, NamespaceDeleteReport, ProjectionClaimVersion,
    ProjectionEntityAlias, ProjectionEpisode, ProjectionEvidenceRef, ProjectionQuery,
    ProjectionRelationVersion, ProveKvPoolArtifactBuildReceiptV1, ProveKvPoolArtifactStatusV1,
    ProveKvPoolGenerationStatus, ProveKvPoolGenerationV1, ProveKvPoolItemMapEntryV1, ReceiptMode,
    ReplayMode, Role, ScoreBreakdown, SearchContext, SearchReceiptAnswersV1, SearchReplayReportV1,
    SearchResponse, SearchResult, SearchSource, SearchSourceType, Session, SparseRankReceiptV1,
    TextChunk, VectorArtifactBuildReceiptV1, VectorSearchReceiptV1, VerificationStatus,
};
pub use vector_backend::{VectorBackend, VectorHit, VectorIndex, VectorIndexConfig};
#[cfg(feature = "turbo-quant-codec")]
pub use vector_codec::TurboQuantCodec;
pub use vector_codec::{
    RawF32Codec, Sq8Codec, VectorArtifactV1, VectorCodec, VectorCodecProfileV1,
};
pub use vector_snapshot::{build_embedding_snapshot, EmbeddingSnapshotRow, EmbeddingSnapshotV1};

use std::sync::Arc;

const MAX_TOP_K: usize = 1_000;
#[cfg(feature = "hnsw")]
const MAX_HNSW_CANDIDATES: usize = 10_000;

pub(crate) use store_support::{
    as_str_slice, build_episode_search_text, merge_trace_ctx, to_owned_string_vec,
    verification_status_for_outcome,
};

/// Deduplicate search results by content fingerprint within the same source type.
///
/// Removes results with near-identical content from the SAME source type
/// (fact vs chunk). Keeps cross-source-type results even if content matches,
/// since a fact and a chunk with identical content have different provenance.
fn dedup_by_content(results: Vec<types::SearchResult>) -> Vec<types::SearchResult> {
    use std::collections::HashSet;
    let mut seen: HashSet<String> = HashSet::new();
    let deduped_result: Vec<types::SearchResult> = results
        .into_iter()
        .filter(|r| {
            let fingerprint: String = r
                .content
                .split_whitespace()
                .take(30)
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            // Include source type (not full source with IDs) in the key
            // so cross-source-type results with identical content are kept,
            // but same-source-type results with identical content are deduped
            let source_type = match &r.source {
                types::SearchSource::Fact { .. } => "fact",
                types::SearchSource::Chunk { .. } => "chunk",
                types::SearchSource::Message { .. } => "message",
                types::SearchSource::Episode { .. } => "episode",
                types::SearchSource::Projection { .. } => "projection",
            };
            let key = format!("{}:{}", source_type, fingerprint);
            seen.insert(key)
        })
        .collect::<Vec<_>>();
    let mut deduped = deduped_result;

    // Pass 2: document diversity -- max 2 chunks per document_id
    let mut doc_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    deduped.retain(|r| {
        if let types::SearchSource::Chunk { document_id, .. } = &r.source {
            let count = doc_counts.entry(document_id.clone()).or_insert(0);
            if *count >= 2 {
                return false;
            }
            *count += 1;
        }
        true
    });

    // Pass 3: heuristic embedding similarity dedup within same source type.
    // When two same-type results have cosine scores within 0.01 of each other
    // and their first-30-word Jaccard similarity is ≥ 0.8, drop the lower scorer.
    {
        let word_set = |r: &types::SearchResult| -> std::collections::HashSet<String> {
            r.content
                .split_whitespace()
                .take(30)
                .map(|w| w.to_lowercase())
                .collect()
        };
        let source_type_tag = |r: &types::SearchResult| -> &'static str {
            match &r.source {
                types::SearchSource::Fact { .. } => "fact",
                types::SearchSource::Chunk { .. } => "chunk",
                types::SearchSource::Message { .. } => "message",
                types::SearchSource::Episode { .. } => "episode",
                types::SearchSource::Projection { .. } => "projection",
            }
        };
        let n = deduped.len();
        let mut drop: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for i in 0..n {
            if drop.contains(&i) {
                continue;
            }
            for j in (i + 1)..n {
                if drop.contains(&j) {
                    continue;
                }
                let ri = &deduped[i];
                let rj = &deduped[j];
                if source_type_tag(ri) != source_type_tag(rj) {
                    continue;
                }
                let (Some(ci), Some(cj)) = (ri.cosine_similarity, rj.cosine_similarity) else {
                    continue;
                };
                if (ci - cj).abs() > 0.01 {
                    continue;
                }
                let wi = word_set(ri);
                let wj = word_set(rj);
                let inter = wi.intersection(&wj).count();
                let uni = wi.union(&wj).count();
                if uni == 0 {
                    continue;
                }
                if inter as f64 / uni as f64 >= 0.8 {
                    if ri.score >= rj.score {
                        drop.insert(j);
                    } else {
                        drop.insert(i);
                        break;
                    }
                }
            }
        }
        if !drop.is_empty() {
            let mut idx = 0usize;
            deduped.retain(|_| {
                let keep = !drop.contains(&idx);
                idx += 1;
                keep
            });
        }
    }

    deduped
}

/// SimpleMem-style semantic content compression for search results.
///
/// Shortens result content to the first sentence plus key terms, capped at 150 chars.
/// This reduces token consumption for downstream LLM consumption while preserving
/// the most salient information.
///
/// The algorithm:
/// 1. Extract the first sentence (up to `. `, `! `, or `? `).
/// 2. If the first sentence is already <= 150 chars, return it.
/// 3. Otherwise, take the first 150 chars of the first sentence, trying to break
///    at a word boundary.
pub fn compress_search_results(results: Vec<types::SearchResult>) -> Vec<types::SearchResult> {
    results
        .into_iter()
        .map(|r| {
            let compressed = compress_content(&r.content);
            types::SearchResult {
                content: compressed,
                ..r
            }
        })
        .collect()
}

/// Compress a single content string to first sentence + key terms, capped at 150 chars.
fn compress_content(content: &str) -> String {
    const MAX_CHARS: usize = 150;

    // Find the first sentence boundary.
    let first_sentence = content
        .find(|c| c == '.' || c == '!' || c == '?')
        .map(|idx| {
            // Include the punctuation.
            let end = idx + 1;
            &content[..end.min(content.len())]
        })
        .unwrap_or(content);

    if first_sentence.chars().count() <= MAX_CHARS {
        return first_sentence.trim().to_string();
    }

    // Truncate to MAX_CHARS at a word boundary.
    let truncated: String = first_sentence.chars().take(MAX_CHARS).collect();
    if let Some(last_space) = truncated.rfind(' ') {
        let at_word_boundary = &truncated[..last_space];
        format!("{}…", at_word_boundary.trim())
    } else {
        format!("{}…", truncated.trim())
    }
}

#[cfg(feature = "hnsw")]
fn verify_hnsw_key_level_integrity(
    conn: &rusqlite::Connection,
    dimensions: usize,
    node_vectors: &std::collections::HashMap<usize, Vec<f32>>,
    sidecar_files_exist: bool,
) -> Result<Vec<String>, MemoryError> {
    let mut issues = Vec::new();
    let mut live_rows: std::collections::HashMap<String, Vec<f32>> =
        std::collections::HashMap::new();

    let mut live_stmt = conn.prepare(
        "SELECT 'fact:' || id, embedding FROM facts WHERE embedding IS NOT NULL
         UNION ALL
         SELECT 'chunk:' || id, embedding FROM chunks WHERE embedding IS NOT NULL
         UNION ALL
         SELECT 'msg:' || id, embedding FROM messages WHERE embedding IS NOT NULL
         UNION ALL
         SELECT 'episode:' || episode_id, embedding FROM episodes WHERE embedding IS NOT NULL",
    )?;
    let live_iter = live_stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
    })?;
    for row in live_iter {
        let (key, blob) = row?;
        match db::decode_f32_le(&blob, dimensions) {
            Ok(vector) => {
                live_rows.insert(key, vector);
            }
            Err(err) => issues.push(format!(
                "HNSW live embedding row {key} has invalid vector: {err}"
            )),
        }
    }

    if !live_rows.is_empty() && !sidecar_files_exist {
        issues.push(format!(
            "HNSW sidecar files are missing while {} embedded rows exist in SQLite",
            live_rows.len()
        ));
    }

    let keymap_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='hnsw_keymap'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !keymap_exists {
        if !live_rows.is_empty() {
            issues.push("HNSW keymap table missing while embedded SQLite rows exist".to_string());
        }
        return Ok(issues);
    }

    let mut active_keymap: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut keymap_stmt =
        conn.prepare("SELECT node_id, item_key FROM hnsw_keymap WHERE deleted = 0")?;
    let keymap_iter = keymap_stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in keymap_iter {
        let (node_id_raw, key) = row?;
        let Some((domain, raw_id)) = key.split_once(':') else {
            issues.push(format!("HNSW keymap entry has malformed key: {key}"));
            continue;
        };
        if !matches!(domain, "fact" | "chunk" | "msg" | "episode") || raw_id.is_empty() {
            issues.push(format!(
                "HNSW keymap entry has unsupported key domain: {key}"
            ));
            continue;
        }
        if domain == "msg" && raw_id.parse::<i64>().is_err() {
            issues.push(format!("HNSW message key has non-integer row id: {key}"));
            continue;
        }
        let node_id = match usize::try_from(node_id_raw) {
            Ok(node_id) => node_id,
            Err(err) => {
                issues.push(format!(
                    "HNSW keymap node_id {node_id_raw} is invalid: {err}"
                ));
                continue;
            }
        };
        active_keymap.insert(key, node_id);
    }

    for key in live_rows.keys() {
        if !active_keymap.contains_key(key) {
            issues.push(format!(
                "HNSW keymap missing live embedded SQLite row: {key}"
            ));
        }
    }

    for (key, node_id) in &active_keymap {
        let Some(live_vector) = live_rows.get(key) else {
            issues.push(format!(
                "HNSW keymap has stale active entry without live embedded SQLite row: {key}"
            ));
            continue;
        };
        let Some(index_vector) = node_vectors.get(node_id) else {
            issues.push(format!(
                "HNSW keymap entry {key} points to missing in-memory node vector {node_id}"
            ));
            continue;
        };
        if index_vector.len() != live_vector.len()
            || index_vector
                .iter()
                .zip(live_vector)
                .any(|(left, right)| left.to_bits() != right.to_bits())
        {
            issues.push(format!(
                "HNSW keymap entry {key} points to node {node_id} whose vector does not match the authoritative SQLite embedding"
            ));
        }
    }

    if active_keymap.len() != live_rows.len() {
        issues.push(format!(
            "HNSW keymap drift: {} active keymap rows vs {} embedded SQLite rows",
            active_keymap.len(),
            live_rows.len()
        ));
    }

    Ok(issues)
}

/// Compatibility-only public access to retained legacy surfaces.
#[doc(hidden)]
pub mod compat {
    #[deprecated(
        since = "0.5.0",
        note = "Legacy ImportEnvelope is migration-only. New integrations should use `ProjectionImportBatchV3` on the canonical lane."
    )]
    #[doc(hidden)]
    #[allow(deprecated)]
    pub mod legacy_import_envelope {
        pub use crate::projection_import::{
            ImportEnvelope, ImportProjectionFreshness, ImportReceipt, ImportRecord, ImportStatus,
        };
        pub use stack_ids::EnvelopeId;
    }

    #[deprecated(
        since = "0.5.0",
        note = "Legacy trace_id is migration-only. Use `stack_ids::TraceCtx`."
    )]
    #[doc(hidden)]
    #[allow(deprecated)]
    pub mod compat_trace_id {
        pub use crate::types::TraceId;
    }
}

/// Thread-safe handle to the memory database.
///
/// Clone is cheap (Arc internals). `Send + Sync`.
#[derive(Clone)]
pub struct MemoryStore {
    inner: Arc<MemoryStoreInner>,
}

struct MemoryStoreInner {
    pool: pool::SqlitePool,
    embedder: Box<dyn Embedder>,
    embedding_permits: Arc<tokio::sync::Semaphore>,
    config: MemoryConfig,
    paths: StoragePaths,
    token_counter: Arc<dyn TokenCounter>,
    /// LRU cache for query embeddings. Key is the text hash, value is the
    /// embedding vector. Capped at 256 entries (~768KB for 768d f32).
    embedding_cache: std::sync::Mutex<lru::LruCache<String, Vec<f32>>>,
    /// LRU cache for search results. Key is "query:top_k", value is results.
    /// Capped at 64 entries.
    search_cache: std::sync::Mutex<lru::LruCache<String, CachedSearchResult>>,
    pub(crate) authority_fault:
        Arc<std::sync::Mutex<Option<authority_contracts::AuthorityFaultStage>>>,
    #[cfg(feature = "hnsw")]
    hnsw_index: std::sync::RwLock<HnswIndex>,
}

/// Role of an embedding in the asymmetric retrieval model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EmbeddingPurpose {
    Query,
    Document,
}

const EMBEDDING_PROFILE_VERSION: &str = "asymmetric-purpose-v2";
const EMBEDDING_NORMALIZATION_PROFILE: &str = "provider-output-v1";

fn active_vector_backend() -> &'static str {
    #[cfg(feature = "hnsw")]
    {
        return "hnsw";
    }
    #[cfg(all(not(feature = "hnsw"), feature = "usearch-backend"))]
    {
        return "usearch";
    }
    #[cfg(all(
        not(feature = "hnsw"),
        not(feature = "usearch-backend"),
        feature = "brute-force"
    ))]
    {
        return "brute_force";
    }
    #[allow(unreachable_code)]
    "unknown"
}

fn active_vector_backend_generation() -> &'static str {
    #[cfg(feature = "hnsw")]
    {
        return "hnsw:hnsw_rs-0.3";
    }
    #[cfg(all(not(feature = "hnsw"), feature = "usearch-backend"))]
    {
        return "usearch:2.25.3";
    }
    #[cfg(all(
        not(feature = "hnsw"),
        not(feature = "usearch-backend"),
        feature = "brute-force"
    ))]
    {
        return "brute_force:f32-v1";
    }
    #[allow(unreachable_code)]
    "unknown:0"
}

fn search_cache_key(
    query: &str,
    top_k: usize,
    backend_generation: &str,
    corpus_epoch: RetrievalEpoch,
) -> String {
    format!(
        "{backend_generation}:epoch={}:{query}:{top_k}",
        corpus_epoch.0
    )
}

#[derive(Clone)]
struct CachedSearchResult {
    results: Vec<types::SearchResult>,
    retrieval_epoch: RetrievalEpoch,
}

#[cfg(feature = "hnsw")]
impl Drop for MemoryStoreInner {
    fn drop(&mut self) {
        if !self.paths.hnsw_dir.exists() {
            tracing::debug!(
                path = %self.paths.hnsw_dir.display(),
                "Skipping HNSW drop flush because the sidecar directory no longer exists"
            );
            return;
        }

        let pending_ops = match self.pool.with_read_conn(db::pending_index_op_count) {
            Ok(count) => count,
            Err(err) => {
                tracing::warn!("Failed to inspect pending HNSW work on drop: {}", err);
                0
            }
        };

        if pending_ops > 0 {
            if let Err(err) =
                hnsw_ops::recover_hnsw_sidecar_sync(&self.pool, &self.paths, &self.config.hnsw)
            {
                tracing::error!("Failed to recover and flush HNSW on drop: {}", err);
            }
            return;
        }

        let hnsw_guard = match self.hnsw_index.read() {
            Ok(g) => g,
            Err(_) => {
                tracing::warn!("HNSW RwLock poisoned on drop — skipping save");
                return;
            }
        };

        if let Err(err) = hnsw_ops::save_hnsw_sidecar(
            &hnsw_guard,
            &self.paths.hnsw_dir,
            &self.paths.hnsw_basename,
        ) {
            tracing::error!("Failed to save HNSW index on drop: {}", err);
        }

        // Flush key mappings to SQLite
        if let Err(e) = self
            .pool
            .with_write_conn(|conn| hnsw_guard.flush_keymap(conn))
        {
            tracing::error!("Failed to flush HNSW keymap on drop: {}", e);
        }
    }
}

fn nonzero_cache_capacity(value: usize) -> std::num::NonZeroUsize {
    match std::num::NonZeroUsize::new(value) {
        Some(value) => value,
        None => std::num::NonZeroUsize::MIN,
    }
}

impl MemoryStore {
    /// Return the capability-gated, append-only authority mutation surface.
    pub fn authority(&self) -> MemoryAuthority {
        MemoryAuthority::new(self.clone())
    }

    /// Run read-only work on a pooled reader connection on a blocking thread.
    ///
    /// This prevents SQLite I/O from stalling the tokio executor while allowing
    /// multiple concurrent readers under WAL mode.
    async fn with_read_conn<F, T>(&self, f: F) -> Result<T, MemoryError>
    where
        F: FnOnce(&rusqlite::Connection) -> Result<T, MemoryError> + Send + 'static,
        T: Send + 'static,
    {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || -> Result<T, MemoryError> {
            inner.pool.with_read_conn(f)
        })
        .await
        .map_err(|e| MemoryError::Other(format!("Blocking task panicked: {}", e)))?
    }

    /// Run write-capable work on the single writer connection on a blocking thread.
    async fn with_write_conn<F, T>(&self, f: F) -> Result<T, MemoryError>
    where
        F: FnOnce(&rusqlite::Connection) -> Result<T, MemoryError> + Send + 'static,
        T: Send + 'static,
    {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || -> Result<T, MemoryError> {
            inner.pool.with_write_conn(f)
        })
        .await
        .map_err(|e| MemoryError::Other(format!("Blocking task panicked: {}", e)))?
    }

    pub(crate) fn clear_search_cache(&self) {
        match self.inner.search_cache.lock() {
            Ok(mut cache) => cache.clear(),
            Err(err) => tracing::warn!(error = %err, "search cache lock poisoned; clear skipped"),
        }
    }

    pub(crate) fn clear_search_cache_strict(&self) -> Result<(), MemoryError> {
        let mut cache = self.inner.search_cache.lock().map_err(|_| {
            MemoryError::ForgettingClosureIncomplete {
                detail: "search cache lock is poisoned".into(),
            }
        })?;
        cache.clear();
        Ok(())
    }

    async fn persist_search_receipt(
        &self,
        receipt: &VectorSearchReceiptV1,
        query: &str,
        namespaces: Option<&[&str]>,
        source_types: Option<&[SearchSourceType]>,
        replay_mode: ReplayMode,
    ) -> Result<(), MemoryError> {
        let receipt = receipt.clone();
        let query = query.to_string();
        let namespaces = to_owned_string_vec(namespaces);
        let source_types = source_types.map(|values| values.to_vec());
        self.with_write_conn(move |conn| {
            db::store_search_receipt(conn, &receipt)?;
            if replay_mode == ReplayMode::StoreInputs {
                let namespace_refs = as_str_slice(&namespaces);
                db::store_replay_inputs(
                    conn,
                    &receipt.receipt_id,
                    &query,
                    namespace_refs.as_deref(),
                    source_types.as_deref(),
                )?;
            }
            Ok(())
        })
        .await
    }

    /// Run HNSW search on a blocking thread to avoid holding std::sync::RwLock
    /// across await points (CONC-001).
    #[cfg(feature = "hnsw")]
    async fn hnsw_search_blocking(
        &self,
        query_embedding: Vec<f32>,
        candidates: usize,
    ) -> Vec<HnswHit> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let guard = inner.hnsw_index.read().unwrap_or_else(|e| e.into_inner());
            match guard.search(&query_embedding, candidates) {
                Ok(hits) => hits,
                Err(e) => {
                    tracing::error!(
                        "HNSW search failed, falling back to brute-force vector search: {}",
                        e
                    );
                    Vec::new()
                }
            }
        })
        .await
        .unwrap_or_else(|e| {
            tracing::error!("HNSW search blocking task panicked: {}", e);
            Vec::new()
        })
    }

    #[cfg(feature = "hnsw")]
    fn sync_pending_hnsw_ops_blocking(&self) -> Result<usize, MemoryError> {
        hnsw_ops::sync_pending_hnsw_sidecar(&self.inner)
    }

    #[cfg(feature = "hnsw")]
    async fn sync_pending_hnsw_ops(&self) -> Result<usize, MemoryError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || hnsw_ops::sync_pending_hnsw_sidecar(&inner))
            .await
            .map_err(|e| MemoryError::Other(format!("Blocking task panicked: {}", e)))?
    }

    #[cfg(feature = "hnsw")]
    async fn sync_pending_hnsw_ops_best_effort(&self, operation: &'static str) {
        if let Err(err) = self.sync_pending_hnsw_ops().await {
            tracing::warn!(
                operation,
                error = %err,
                "SQLite write committed but HNSW sidecar sync is still pending"
            );
        } else {
            self.maybe_flush_hnsw();
        }
    }

    /// Open or create a memory store at the configured base directory.
    ///
    /// Creates the directory if it doesn't exist, opens/creates SQLite,
    /// runs migrations, and initializes the HNSW index.
    ///
    /// When the `candle-embedder` feature is enabled, this defaults to
    /// [`CandleEmbedder`] (in-process, pure-Rust, no Ollama required).
    /// Otherwise it defaults to [`OllamaEmbedder`].
    pub fn open(config: MemoryConfig) -> Result<Self, MemoryError> {
        let config = config.normalize_and_validate()?;
        #[cfg(feature = "candle-embedder")]
        let embedder: Box<dyn Embedder> = Box::new(CandleEmbedder::try_new(&config.embedding)?);
        #[cfg(not(feature = "candle-embedder"))]
        let embedder: Box<dyn Embedder> = Box::new(OllamaEmbedder::try_new(&config.embedding)?);
        Self::open_with_embedder(config, embedder)
    }

    /// Open with a custom embedder (for testing or non-Ollama providers).
    #[allow(unused_mut)] // `config` is mutated only when the `hnsw` feature is enabled
    pub fn open_with_embedder(
        mut config: MemoryConfig,
        embedder: Box<dyn Embedder>,
    ) -> Result<Self, MemoryError> {
        config = config.normalize_and_validate()?;
        if embedder.dimensions() != config.embedding.dimensions {
            return Err(MemoryError::DimensionMismatch {
                expected: config.embedding.dimensions,
                actual: embedder.dimensions(),
            });
        }
        config.embedding.model = embedder.model_name().to_string();

        let paths = StoragePaths::new(&config.base_dir);

        // Create directory if needed
        std::fs::create_dir_all(&paths.base_dir).map_err(|e| {
            MemoryError::StorageError(format!(
                "Failed to create directory {}: {}",
                paths.base_dir.display(),
                e
            ))
        })?;

        let pool = pool::SqlitePool::open(&paths.sqlite_path, &config.pool, &config.limits)?;
        // Purpose/profile changes invalidate every derived vector even when the provider model
        // and dimensions are unchanged. Binding the profile into durable metadata makes upgrades
        // fail visibly through `embeddings_dirty` instead of silently reusing old vectors.
        let mut embedding_metadata = config.embedding.clone();
        embedding_metadata.model = format!(
            "{}|{}|{}",
            embedding_metadata.model, EMBEDDING_NORMALIZATION_PROFILE, EMBEDDING_PROFILE_VERSION
        );
        pool.with_write_conn(|conn| db::check_embedding_metadata(conn, &embedding_metadata))?;

        // Ensure HNSW dimensions match the embedding config
        #[cfg(feature = "hnsw")]
        {
            config.hnsw.dimensions = config.embedding.dimensions;
        }

        let token_counter = config
            .token_counter
            .clone()
            .unwrap_or_else(tokenizer::default_token_counter);

        #[cfg(feature = "hnsw")]
        let hnsw_index = {
            let hnsw_config = config.hnsw.clone();

            let embeddings_dirty = pool.with_read_conn(db::is_embeddings_dirty)?;
            let pending_index_ops = pool.with_read_conn(db::pending_index_op_count)?;

            if embeddings_dirty {
                // Embedding model changed — old HNSW index is useless.
                // Create a fresh index; reembed_all() will rebuild it.
                tracing::warn!(
                    "Embedding model changed — creating fresh HNSW index (old index is stale)"
                );
                pool.with_write_conn(|conn| {
                    db::clear_all_pending_index_ops(conn)?;
                    db::set_sidecar_dirty(conn, false)?;
                    Ok(())
                })?;
                HnswIndex::new(hnsw_config)?
            } else if pending_index_ops > 0 || pool.with_read_conn(db::is_sidecar_dirty)? {
                tracing::warn!(
                    pending_index_ops,
                    "Recovering HNSW sidecar from SQLite because durable sidecar work exists"
                );
                hnsw_ops::recover_hnsw_sidecar_sync(&pool, &paths, &hnsw_config)?
            } else if paths.hnsw_files_exist() {
                tracing::info!("Loading HNSW index from {:?}", paths.hnsw_dir);
                match HnswIndex::load(&paths.hnsw_dir, &paths.hnsw_basename, hnsw_config.clone()) {
                    Ok(index) => {
                        // Load key mappings from SQLite
                        if let Err(e) = pool.with_write_conn(|conn| index.load_keymap(conn)) {
                            tracing::warn!("Failed to load HNSW key mappings: {}. Mappings will be empty until rebuild.", e);
                        }

                        // Stale index detection: compare HNSW entry count vs SQLite
                        // embedding count. A mismatch means the app crashed before
                        // flushing HNSW, or keys were lost.
                        let hnsw_count = index.len();
                        let sqlite_count: i64 = pool.with_read_conn(|conn| {
                            Ok(conn.query_row(
                                    "SELECT (SELECT COUNT(*) FROM facts WHERE embedding IS NOT NULL) +
                                        (SELECT COUNT(*) FROM chunks WHERE embedding IS NOT NULL) +
                                        (SELECT COUNT(*) FROM messages WHERE embedding IS NOT NULL) +
                                        (SELECT COUNT(*) FROM episodes WHERE embedding IS NOT NULL)",
                                    [],
                                    |row| row.get(0),
                                )?)
                        })?;

                        let drift = (sqlite_count - hnsw_count as i64).abs();
                        if drift > 0 {
                            tracing::warn!(
                                hnsw_count,
                                sqlite_count,
                                drift,
                                "HNSW index is stale — {} entries differ from SQLite. \
                                 Likely caused by unclean shutdown. Triggering inline rebuild.",
                                drift
                            );
                            // Discard the stale index and rebuild from SQLite
                            let rebuilt =
                                hnsw_ops::recover_hnsw_sidecar_sync(&pool, &paths, &hnsw_config)?;
                            tracing::info!(
                                active = rebuilt.len(),
                                "HNSW index rebuilt after stale detection"
                            );
                            rebuilt
                        } else {
                            tracing::info!(
                                "HNSW index loaded ({} active keys, in sync with SQLite)",
                                hnsw_count
                            );
                            index
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to load HNSW index: {}. Rebuilding sidecar from authoritative SQLite rows.",
                            e
                        );
                        hnsw_ops::recover_hnsw_sidecar_sync(&pool, &paths, &hnsw_config)?
                    }
                }
            } else {
                // Check if SQLite has embeddings that should be in the index.
                // This happens when: sidecar files were deleted, data dir was
                // partially copied, app crashed before first flush, or HNSW was
                // added after data already existed.
                let orphan_count: i64 = pool.with_read_conn(|conn| {
                    Ok(conn.query_row(
                        "SELECT (SELECT COUNT(*) FROM facts WHERE embedding IS NOT NULL) +
                                (SELECT COUNT(*) FROM chunks WHERE embedding IS NOT NULL) +
                                (SELECT COUNT(*) FROM messages WHERE embedding IS NOT NULL) +
                                (SELECT COUNT(*) FROM episodes WHERE embedding IS NOT NULL)",
                        [],
                        |row| row.get(0),
                    )?)
                })?;

                if orphan_count > 0 {
                    tracing::warn!(
                        orphan_count,
                        "HNSW sidecar files missing but {} embeddings exist in SQLite — \
                         rebuilding index inline",
                        orphan_count
                    );
                    let new_index =
                        hnsw_ops::recover_hnsw_sidecar_sync(&pool, &paths, &hnsw_config)?;
                    tracing::info!(
                        active = new_index.len(),
                        "HNSW index rebuilt from SQLite embeddings"
                    );
                    new_index
                } else {
                    tracing::info!("Creating new empty HNSW index (no embeddings in SQLite)");
                    HnswIndex::new(hnsw_config)?
                }
            }
        };

        let store = Self {
            inner: Arc::new(MemoryStoreInner {
                pool,
                embedder,
                embedding_permits: Arc::new(tokio::sync::Semaphore::new(
                    config.limits.max_embedding_concurrency,
                )),
                config,
                paths,
                token_counter,
                embedding_cache: std::sync::Mutex::new(lru::LruCache::new(nonzero_cache_capacity(
                    256,
                ))),
                search_cache: std::sync::Mutex::new(lru::LruCache::new(nonzero_cache_capacity(64))),
                authority_fault: Arc::new(std::sync::Mutex::new(None)),
                #[cfg(feature = "hnsw")]
                hnsw_index: std::sync::RwLock::new(hnsw_index),
            }),
        };

        #[cfg(feature = "hnsw")]
        if let Err(err) = store.sync_pending_hnsw_ops_blocking() {
            tracing::warn!(
                error = %err,
                "Failed to reconcile pending HNSW sidecar ops during open; sidecar replay remains pending"
            );
        }

        Ok(store)
    }

    async fn with_embedding_permit(
        &self,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, MemoryError> {
        self.inner
            .embedding_permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| MemoryError::Other(format!("embedding semaphore closed: {e}")))
    }

    async fn embed_text_internal(
        &self,
        text: &str,
        purpose: EmbeddingPurpose,
    ) -> Result<Vec<f32>, MemoryError> {
        // Check embedding cache first -- skip the compute for repeated queries
        let cache_key = format!(
            "{:?}|{}|{}|{}|{}|{}",
            purpose,
            self.inner.embedder.model_name(),
            self.inner.config.embedding.dimensions,
            EMBEDDING_NORMALIZATION_PROFILE,
            EMBEDDING_PROFILE_VERSION,
            text
        );
        {
            match self.inner.embedding_cache.lock() {
                Ok(mut cache) => {
                    if let Some(cached) = cache.get(&cache_key).cloned() {
                        return Ok(cached);
                    }
                }
                Err(err) => {
                    tracing::warn!(error = %err, "embedding cache lock poisoned; lookup skipped")
                }
            }
        }

        let _permit = self.with_embedding_permit().await?;
        // nomic-embed-text-v1.5 uses asymmetric prefixes:
        // "search_query:" for queries (search-time)
        // "search_document:" for documents (ingestion-time)
        // The prefix is added here so ALL embedder backends (Candle, Ollama)
        // get the same prefix without each backend needing to handle it.
        let prefixed = match purpose {
            EmbeddingPurpose::Query => format!("search_query: {text}"),
            EmbeddingPurpose::Document => format!("search_document: {text}"),
        };
        let embedding = self.inner.embedder.embed(&prefixed).await?;
        db::validate_embedding(&embedding, self.inner.config.embedding.dimensions)?;

        // Store in cache (keyed by original text, not prefixed)
        {
            match self.inner.embedding_cache.lock() {
                Ok(mut cache) => {
                    cache.put(cache_key, embedding.clone());
                }
                Err(err) => {
                    tracing::warn!(error = %err, "embedding cache lock poisoned; insert skipped")
                }
            }
        }

        Ok(embedding)
    }

    /// Embed text while retaining an embedder-provided sparse representation.
    /// Dense-only derivation is possible only through the explicit search config.
    async fn embed_text_with_sparse_internal(
        &self,
        text: &str,
        purpose: EmbeddingPurpose,
    ) -> Result<(Vec<f32>, Option<SparseWeights>, Option<String>), MemoryError> {
        let _permit = self.with_embedding_permit().await?;
        // Keep the established prefix used by embed_text_internal so enabling
        // sparse persistence does not silently change dense embedding semantics.
        let prefixed = match purpose {
            EmbeddingPurpose::Query => format!("search_query: {text}"),
            EmbeddingPurpose::Document => format!("search_document: {text}"),
        };
        if let Some(multi) = self.inner.embedder.embed_multi_optional(&prefixed).await? {
            db::validate_embedding(&multi.dense, self.inner.config.embedding.dimensions)?;
            if multi
                .sparse
                .entries
                .iter()
                .any(|(_, weight)| !weight.is_finite())
            {
                return Err(MemoryError::Other(
                    "embedder returned non-finite sparse weights".to_string(),
                ));
            }
            return Ok((
                multi.dense,
                Some(multi.sparse),
                Some(if self.inner.embedder.model_name().contains("bge-m3") {
                    "bge_m3_generated_sparse".to_string()
                } else {
                    "native_sparse".to_string()
                }),
            ));
        }

        let dense = self.inner.embedder.embed(&prefixed).await?;
        db::validate_embedding(&dense, self.inner.config.embedding.dimensions)?;
        if self.inner.config.search.derive_sparse_from_dense {
            let sparse = SparseWeights::from_dense(
                &dense,
                self.inner.config.search.sparse_derive_top_k,
                self.inner.config.search.sparse_derive_min_weight,
            );
            Ok((
                dense,
                Some(sparse),
                Some("generic_dense_derived_sparse".to_string()),
            ))
        } else {
            Ok((dense, None, None))
        }
    }

    async fn embed_batch_with_sparse_internal(
        &self,
        texts: Vec<String>,
        purpose: EmbeddingPurpose,
    ) -> Result<Vec<(Vec<f32>, Option<SparseWeights>, Option<String>)>, MemoryError> {
        let requested = texts.len();
        let _permit = self.with_embedding_permit().await?;
        let prefix = match purpose {
            EmbeddingPurpose::Query => "search_query",
            EmbeddingPurpose::Document => "search_document",
        };
        let prefixed: Vec<String> = texts
            .iter()
            .map(|text| format!("{prefix}: {text}"))
            .collect();
        if let Some(multi) = self
            .inner
            .embedder
            .embed_batch_multi_optional(prefixed.clone())
            .await?
        {
            if multi.len() != requested {
                return Err(MemoryError::EmbeddingBatchCountMismatch {
                    requested,
                    returned: multi.len(),
                });
            }
            let representation = if self.inner.embedder.model_name().contains("bge-m3") {
                "bge_m3_generated_sparse"
            } else {
                "native_sparse"
            };
            let mut output = Vec::with_capacity(requested);
            for value in multi {
                db::validate_embedding(&value.dense, self.inner.config.embedding.dimensions)?;
                if value
                    .sparse
                    .entries
                    .iter()
                    .any(|(_, weight)| !weight.is_finite())
                {
                    return Err(MemoryError::Other(
                        "embedder returned non-finite sparse weights".to_string(),
                    ));
                }
                output.push((
                    value.dense,
                    Some(value.sparse),
                    Some(representation.to_string()),
                ));
            }
            return Ok(output);
        }

        let dense = self.inner.embedder.embed_batch(prefixed).await?;
        db::validate_embedding_batch(&dense, requested, self.inner.config.embedding.dimensions)?;
        Ok(dense
            .into_iter()
            .map(|dense| {
                if self.inner.config.search.derive_sparse_from_dense {
                    let sparse = SparseWeights::from_dense(
                        &dense,
                        self.inner.config.search.sparse_derive_top_k,
                        self.inner.config.search.sparse_derive_min_weight,
                    );
                    (
                        dense,
                        Some(sparse),
                        Some("generic_dense_derived_sparse".to_string()),
                    )
                } else {
                    (dense, None, None)
                }
            })
            .collect())
    }

    async fn embed_batch_internal(
        &self,
        texts: Vec<String>,
        purpose: EmbeddingPurpose,
    ) -> Result<Vec<Vec<f32>>, MemoryError> {
        let requested = texts.len();

        // Check cache for each text
        let mut results: Vec<Option<Vec<f32>>> = Vec::with_capacity(requested);
        let mut misses: Vec<String> = Vec::new();
        let mut miss_indices: Vec<usize> = Vec::new();

        let cache_key = |text: &str| {
            format!(
                "{:?}|{}|{}|{}|{}|{}",
                purpose,
                self.inner.embedder.model_name(),
                self.inner.config.embedding.dimensions,
                EMBEDDING_NORMALIZATION_PROFILE,
                EMBEDDING_PROFILE_VERSION,
                text
            )
        };
        for (i, text) in texts.iter().enumerate() {
            match self.inner.embedding_cache.lock() {
                Ok(cache) => {
                    // `peek` is deliberately non-mutating. Until the entire provider
                    // batch validates, even LRU recency ordering is transactional.
                    if let Some(cached) = cache.peek(&cache_key(text)).cloned() {
                        results.push(Some(cached));
                    } else {
                        results.push(None);
                        miss_indices.push(i);
                        misses.push(text.clone());
                    }
                }
                Err(err) => {
                    tracing::warn!(error = %err, "embedding cache lock poisoned; lookup skipped");
                    results.push(None);
                    miss_indices.push(i);
                    misses.push(text.clone());
                }
            }
        }

        let _permit = self.with_embedding_permit().await?;

        // Add search_document: prefix for all documents (ingestion path)
        let prefix = match purpose {
            EmbeddingPurpose::Query => "search_query",
            EmbeddingPurpose::Document => "search_document",
        };
        let prefixed_misses: Vec<String> =
            misses.iter().map(|t| format!("{prefix}: {t}")).collect();

        let miss_embeddings = if prefixed_misses.is_empty() {
            Vec::new()
        } else {
            let embeddings = self.inner.embedder.embed_batch(prefixed_misses).await?;
            // Validate batch count before caching or assembling
            if embeddings.len() != misses.len() {
                return Err(MemoryError::EmbeddingBatchCountMismatch {
                    requested: misses.len(),
                    returned: embeddings.len(),
                });
            }
            // Validate the complete response before mutating the shared cache. A
            // partially valid provider response is one failed transaction.
            for embedding in &embeddings {
                db::validate_embedding(embedding, self.inner.config.embedding.dimensions)?;
            }
            // Cache the new embeddings (keyed by original text, not prefixed)
            match self.inner.embedding_cache.lock() {
                Ok(mut cache) => {
                    for (text, emb) in misses.iter().zip(embeddings.iter()) {
                        cache.put(cache_key(text), emb.clone());
                    }
                }
                Err(err) => {
                    tracing::warn!(error = %err, "embedding cache lock poisoned; batch insert skipped")
                }
            }
            embeddings
        };

        // Assemble results in order (all slots guaranteed to have data)
        let mut final_results = Vec::with_capacity(requested);
        let mut miss_idx = 0;
        for i in 0..requested {
            if let Some(emb) = &results[i] {
                final_results.push(emb.clone());
            } else {
                final_results.push(miss_embeddings[miss_idx].clone());
                miss_idx += 1;
            }
        }

        db::validate_embedding_batch(
            &final_results,
            requested,
            self.inner.config.embedding.dimensions,
        )?;
        Ok(final_results)
    }

    fn validate_embedding_dimensions(&self, embedding: &[f32]) -> Result<(), MemoryError> {
        db::validate_embedding(embedding, self.inner.config.embedding.dimensions)
    }

    fn validate_content(&self, field: &'static str, content: &str) -> Result<(), MemoryError> {
        if content.is_empty() {
            return Err(MemoryError::InvalidConfig {
                field,
                reason: "content must not be empty".to_string(),
            });
        }

        let limit = self.inner.config.limits.max_content_bytes;
        if content.len() > limit {
            return Err(MemoryError::ContentTooLarge {
                size: content.len(),
                limit,
            });
        }

        Ok(())
    }

    fn validate_confidence(confidence: f32) -> Result<(), MemoryError> {
        if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
            return Err(MemoryError::InvalidConfig {
                field: "episodes.confidence",
                reason: "confidence must be finite and within [0.0, 1.0]".to_string(),
            });
        }
        Ok(())
    }

    // ─── HNSW Management ───────────────────────────────────────

    /// Rebuild feature-gated TurboQuant artifacts from authoritative SQLite f32 embeddings.
    #[cfg(feature = "turbo-quant-codec")]
    pub async fn rebuild_vector_artifacts(
        &self,
    ) -> Result<VectorArtifactBuildReceiptV1, MemoryError> {
        let dim = self.inner.config.embedding.dimensions;
        let search = self.inner.config.search.clone();
        self.with_write_conn(move |conn| {
            db::rebuild_turbo_quant_artifacts(
                conn,
                dim,
                search.turbo_quant_bits,
                search.turbo_quant_projections,
                search.turbo_quant_seed,
            )
        })
        .await
    }

    /// Rebuild the HNSW index from SQLite f32 embeddings.
    ///
    /// Call this if sidecar files are missing, corrupted, or after `reembed_all()`.
    #[cfg(feature = "hnsw")]
    pub async fn rebuild_hnsw_index(
        &self,
    ) -> Result<crate::types::VectorArtifactBuildReceiptV1, MemoryError> {
        tracing::info!("Rebuilding HNSW index from SQLite embeddings...");
        let hnsw_config = self.inner.config.hnsw.clone();
        let (new_index, build_receipt) = self
            .with_read_conn(move |conn| hnsw_ops::rebuild_hnsw_from_sqlite(conn, &hnsw_config))
            .await?;

        {
            let mut guard = self
                .inner
                .hnsw_index
                .write()
                .unwrap_or_else(|e| e.into_inner());
            *guard = new_index.clone();
        }

        hnsw_ops::save_hnsw_sidecar(
            &new_index,
            &self.inner.paths.hnsw_dir,
            &self.inner.paths.hnsw_basename,
        )?;
        self.inner.pool.with_write_conn(|conn| {
            new_index.flush_keymap(conn)?;
            db::clear_all_pending_index_ops(conn)?;
            db::set_sidecar_dirty(conn, false)?;
            Ok(())
        })?;

        tracing::info!(active = new_index.len(), receipt_generation_id = ?build_receipt.generation_id, "HNSW index rebuilt");

        Ok(build_receipt)
    }

    /// Opportunistically flush HNSW if the configured interval has elapsed.
    ///
    /// Cheap no-op when `flush_interval_secs` is None or the interval hasn't
    /// elapsed yet (just an atomic load + epoch comparison).
    #[cfg(feature = "hnsw")]
    fn maybe_flush_hnsw(&self) {
        if let Some(interval) = self.inner.config.hnsw.flush_interval_secs {
            let guard = self
                .inner
                .hnsw_index
                .read()
                .unwrap_or_else(|e| e.into_inner());
            if guard.should_flush(interval) {
                drop(guard); // release read lock before flushing
                if let Err(e) = self.flush_hnsw() {
                    tracing::warn!("Opportunistic HNSW flush failed: {}", e);
                } else {
                    let guard = self
                        .inner
                        .hnsw_index
                        .read()
                        .unwrap_or_else(|e| e.into_inner());
                    guard.update_last_flush_epoch();
                    tracing::info!("Opportunistic HNSW flush completed");
                }
            }
        }
    }

    /// Persist the HNSW graph, vector data, and key mappings to disk.
    ///
    /// Called automatically on drop, but can be called explicitly for durability.
    #[cfg(feature = "hnsw")]
    pub fn flush_hnsw(&self) -> Result<(), MemoryError> {
        let pending_ops = self.inner.pool.with_read_conn(db::pending_index_op_count)?;
        if pending_ops > 0 {
            tracing::info!(
                pending_ops,
                "Flushing HNSW via authoritative SQLite rebuild because pending durable sidecar work exists"
            );
            let rebuilt = hnsw_ops::recover_hnsw_sidecar_sync(
                &self.inner.pool,
                &self.inner.paths,
                &self.inner.config.hnsw,
            )?;
            let mut guard = self
                .inner
                .hnsw_index
                .write()
                .unwrap_or_else(|e| e.into_inner());
            *guard = rebuilt;
            return Ok(());
        }

        let index = self
            .inner
            .hnsw_index
            .write()
            .unwrap_or_else(|e| e.into_inner());
        hnsw_ops::save_hnsw_sidecar(
            &index,
            &self.inner.paths.hnsw_dir,
            &self.inner.paths.hnsw_basename,
        )?;

        // Flush key mappings to SQLite
        self.inner.pool.with_write_conn(|conn| {
            index.flush_keymap(conn)?;
            db::clear_all_pending_index_ops(conn)?;
            db::set_sidecar_dirty(conn, false)?;
            Ok(())
        })?;
        Ok(())
    }

    /// Compact the HNSW index by rebuilding without tombstones.
    ///
    /// Only rebuilds if the deleted ratio exceeds the compaction threshold.
    #[cfg(feature = "hnsw")]
    pub async fn compact_hnsw(&self) -> Result<(), MemoryError> {
        if !self
            .inner
            .hnsw_index
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .needs_compaction()
        {
            tracing::info!("HNSW compaction not needed (deleted ratio below threshold)");
            return Ok(());
        }
        let _receipt = self.rebuild_hnsw_index().await?;
        Ok(())
    }

    // ─── Integrity & Diagnostics ────────────────────────────────

    /// Verify database integrity.
    ///
    /// In `Quick` mode, checks table existence and row counts.
    /// In `Full` mode, also verifies FTS consistency and runs SQLite integrity_check.
    pub async fn verify_integrity(
        &self,
        mode: db::VerifyMode,
    ) -> Result<db::IntegrityReport, MemoryError> {
        let use_writer = mode == db::VerifyMode::Full;
        let mut report = if use_writer {
            self.with_write_conn(move |conn| db::verify_integrity_sync(conn, mode))
                .await?
        } else {
            self.with_read_conn(move |conn| db::verify_integrity_sync(conn, mode))
                .await?
        };

        #[cfg(feature = "hnsw")]
        {
            let hnsw_vectors = self
                .inner
                .hnsw_index
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .vector_snapshot();
            let hnsw_dims = self.inner.config.embedding.dimensions;
            let hnsw_files_exist = self.inner.paths.hnsw_files_exist();

            let hnsw_issues = if use_writer {
                let hnsw_vectors = hnsw_vectors.clone();
                self.with_write_conn(move |conn| {
                    verify_hnsw_key_level_integrity(
                        conn,
                        hnsw_dims,
                        &hnsw_vectors,
                        hnsw_files_exist,
                    )
                })
                .await?
            } else {
                let hnsw_vectors = hnsw_vectors.clone();
                self.with_read_conn(move |conn| {
                    verify_hnsw_key_level_integrity(
                        conn,
                        hnsw_dims,
                        &hnsw_vectors,
                        hnsw_files_exist,
                    )
                })
                .await?
            };
            report.issues.extend(hnsw_issues);
        }

        report.ok = report.issues.is_empty();
        Ok(report)
    }

    /// Reconcile detected integrity issues.
    ///
    /// - `ReportOnly`: no-op, just returns the integrity report.
    /// - `RebuildFts`: rebuilds all FTS indexes from source data.
    /// - `ReEmbed`: re-embeds authoritative rows and then verifies integrity.
    pub async fn reconcile(
        &self,
        action: db::ReconcileAction,
    ) -> Result<db::IntegrityReport, MemoryError> {
        match action {
            db::ReconcileAction::ReportOnly => self.verify_integrity(db::VerifyMode::Full).await,
            db::ReconcileAction::RebuildFts => {
                self.with_write_conn(db::reconcile_fts).await?;
                #[cfg(feature = "hnsw")]
                self.sync_pending_hnsw_ops_best_effort("reconcile_rebuild_fts")
                    .await;
                self.verify_integrity(db::VerifyMode::Full).await
            }
            db::ReconcileAction::ReEmbed => {
                #[cfg(feature = "admin-ops")]
                {
                    self.reembed_all().await?;
                    self.verify_integrity(db::VerifyMode::Full).await
                }

                #[cfg(not(feature = "admin-ops"))]
                Err(MemoryError::Other(
                    "reconcile ReEmbed is admin-only; enable `admin-ops`".to_string(),
                ))
            }
        }
    }

    /// Get the current configuration.
    pub fn config(&self) -> &MemoryConfig {
        &self.inner.config
    }

    /// View the store as a derived graph over documents, chunks, facts, sessions, messages,
    /// episodes, namespaces, semantic similarity edges, and first-class stored graph edges.
    pub fn graph_view(&self) -> Arc<dyn GraphView> {
        graph::graph_view(self.inner.clone())
    }

    // ─── First-class stored graph edges ──────────────────────────

    /// Add a durable, typed graph edge between two nodes.
    ///
    /// Nodes are identified by prefixed IDs (e.g. `fact:<uuid>`,
    /// `namespace:<name>`, `document:<id>`). The edge type must be one of
    /// `GraphEdgeType::Semantic`, `Temporal`, `Causal`, or `Entity`.
    ///
    /// Insertion is idempotent on content digest — inserting the same edge
    /// twice returns the existing edge without creating a duplicate.
    ///
    /// Returns the stored edge including its assigned ID and recorded_at timestamp.
    pub async fn add_graph_edge(
        &self,
        source: &str,
        target: &str,
        edge_type: GraphEdgeType,
        weight: f64,
        metadata: Option<serde_json::Value>,
    ) -> Result<graph_edges::StoredGraphEdge, MemoryError> {
        let params = graph_edges::AddGraphEdgeParams {
            source: source.to_string(),
            target: target.to_string(),
            edge_type,
            weight,
            metadata,
            valid_time: None,
            recorded_time: None,
        };
        let edge = self
            .with_write_conn(move |conn| graph_edges::insert_graph_edge(conn, &params))
            .await?;
        self.clear_search_cache();
        Ok(edge)
    }

    /// Add a durable graph edge with explicit bitemporal timestamps.
    ///
    /// Use this when importing or correcting historical relationships where
    /// domain validity and system record time differ from the current wall clock.
    pub async fn add_graph_edge_at(
        &self,
        source: &str,
        target: &str,
        edge_type: GraphEdgeType,
        weight: f64,
        metadata: Option<serde_json::Value>,
        valid_time: &str,
        recorded_time: &str,
    ) -> Result<graph_edges::StoredGraphEdge, MemoryError> {
        let params = graph_edges::AddGraphEdgeParams {
            source: source.to_string(),
            target: target.to_string(),
            edge_type,
            weight,
            metadata,
            valid_time: Some(valid_time.to_string()),
            recorded_time: Some(recorded_time.to_string()),
        };
        let edge = self
            .with_write_conn(move |conn| graph_edges::insert_graph_edge(conn, &params))
            .await?;
        self.clear_search_cache();
        Ok(edge)
    }

    /// **DANGER**: legacy physical consolidation mutates a truth-bearing row.
    ///
    /// This migration-only operation is admin-only. Governed callers must use a
    /// source-grounded supersession transition rather than mutating a head.
    #[cfg(feature = "admin-ops")]
    pub async fn consolidate_facts(
        &self,
        keep_id: &str,
        supersede_id: &str,
        merged_content: &str,
    ) -> Result<(), MemoryError> {
        let keep_id = keep_id.to_string();
        let supersede_id = supersede_id.to_string();
        let merged_content = merged_content.to_string();
        self.with_write_conn(move |conn| {
            use rusqlite::params;

            // 1. Update the kept fact's content
            let (fts_rowid, old_content): (i64, String) = conn
                .query_row(
                    "SELECT fm.rowid, f.content
                     FROM facts f
                     JOIN facts_rowid_map fm ON fm.fact_id = f.id
                     WHERE f.id = ?1",
                    params![&keep_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|e| MemoryError::FactNotFound(format!("{}: {e}", keep_id)))?;

            conn.execute(
                "INSERT INTO facts_fts(facts_fts, rowid, content) VALUES('delete', ?1, ?2)",
                params![fts_rowid, old_content],
            )?;

            conn.execute(
                "UPDATE facts SET content = ?1, updated_at = datetime('now') WHERE id = ?2",
                params![&merged_content, &keep_id],
            )?;

            conn.execute(
                "INSERT INTO facts_fts(rowid, content) VALUES (?1, ?2)",
                params![fts_rowid, &merged_content],
            )?;

            // 2. Add supersession edge from kept to superseded
            let edge_type_json = r#"{"Entity":{"relation":"supersedes"}}"#;
            let source = format!("fact:{}", keep_id);
            let target = format!("fact:{}", supersede_id);
            conn.execute(
                "INSERT INTO graph_edges (source, target, edge_type, weight, recorded_at, is_invalidated)
                 VALUES (?1, ?2, ?3, 1.0, datetime('now'), 0)",
                params![&source, &target, edge_type_json],
            )?;

            Ok(())
        })
        .await?;
        self.clear_search_cache();
        Ok(())
    }

    /// List all stored graph edges involving a given node (as source or target),
    /// excluding invalidated edges.
    pub async fn list_graph_edges_for_node(
        &self,
        node_id: &str,
    ) -> Result<Vec<graph_edges::StoredGraphEdge>, MemoryError> {
        let node_id = node_id.to_string();
        self.with_read_conn(move |conn| graph_edges::list_graph_edges_for_node(conn, &node_id))
            .await
    }

    /// List graph edges involving a node as of explicit bitemporal cutoffs.
    ///
    /// `as_of_valid_time` is domain/business time; `as_of_recorded_time` is
    /// system knowledge time. This is the graph analogue of bitemporal as-of
    /// fact queries: it can reconstruct what the relationship graph knew at a
    /// prior recorded time, including edges invalidated later.
    pub async fn list_graph_edges_for_node_as_of(
        &self,
        node_id: &str,
        as_of_valid_time: &str,
        as_of_recorded_time: &str,
    ) -> Result<Vec<graph_edges::StoredGraphEdge>, MemoryError> {
        let node_id = node_id.to_string();
        let as_of_valid_time = as_of_valid_time.to_string();
        let as_of_recorded_time = as_of_recorded_time.to_string();
        self.with_read_conn(move |conn| {
            graph_edges::list_graph_edges_for_node_as_of(
                conn,
                &node_id,
                &as_of_valid_time,
                &as_of_recorded_time,
            )
        })
        .await
    }

    /// List ALL stored graph edges, excluding invalidated ones.
    pub async fn list_all_graph_edges(
        &self,
    ) -> Result<Vec<graph_edges::StoredGraphEdge>, MemoryError> {
        self.with_read_conn(graph_edges::list_all_graph_edges).await
    }

    /// List graph edges within N hops of the given seed node IDs.
    ///
    /// Performs a BFS expansion from the seeds, loading only edges in
    /// the local neighborhood. Much faster than `list_all_graph_edges`
    /// when you only need the subgraph around search results.
    ///
    /// - `seed_ids`: starting node IDs (typically search result IDs)
    /// - `max_hops`: BFS depth (1 = direct neighbors, 2 = neighbors of neighbors)
    /// - `max_nodes`: cap on total nodes visited (prevents hub explosion)
    pub async fn list_graph_edges_for_neighborhood(
        &self,
        seed_ids: Vec<String>,
        max_hops: usize,
        max_nodes: usize,
    ) -> Result<Vec<graph_edges::StoredGraphEdge>, MemoryError> {
        self.with_read_conn(move |conn| {
            graph_edges::list_graph_edges_for_neighborhood(conn, &seed_ids, max_hops, max_nodes)
        })
        .await
    }

    /// Invalidate a stored graph edge by ID. Append-only — the row is never deleted.
    pub async fn invalidate_graph_edge(
        &self,
        edge_id: &str,
        reason: &str,
    ) -> Result<(), MemoryError> {
        let edge_id = edge_id.to_string();
        let reason = reason.to_string();
        self.with_write_conn(move |conn| {
            graph_edges::invalidate_graph_edge(conn, &edge_id, &reason)
        })
        .await
    }

    /// Count non-invalidated stored graph edges.
    pub async fn count_graph_edges(&self) -> Result<usize, MemoryError> {
        self.with_read_conn(graph_edges::count_graph_edges).await
    }

    // ─── Search ─────────────────────────────────────────────────

    /// Hybrid search across facts, document chunks, and searchable episodes.
    pub async fn search(
        &self,
        query: &str,
        top_k: Option<usize>,
        namespaces: Option<&[&str]>,
        source_types: Option<&[SearchSourceType]>,
    ) -> Result<Vec<SearchResult>, MemoryError> {
        let compress = self.inner.config.search.compress_results;
        let results = self
            .search_with_context(
                query,
                top_k,
                namespaces,
                source_types,
                SearchContext::default_now(),
            )
            .await?
            .results;
        if compress {
            Ok(compress_search_results(results))
        } else {
            Ok(results)
        }
    }

    /// Hybrid search with an explicit deterministic context and optional receipt.
    pub async fn search_with_context(
        &self,
        query: &str,
        top_k: Option<usize>,
        namespaces: Option<&[&str]>,
        source_types: Option<&[SearchSourceType]>,
        context: SearchContext,
    ) -> Result<SearchResponse, MemoryError> {
        self.search_with_context_for_view(
            query,
            top_k,
            namespaces,
            source_types,
            context,
            StateView::Current,
        )
        .await
    }

    /// Hybrid fact search under an explicit authority-state view.
    pub async fn search_with_view(
        &self,
        query: &str,
        top_k: Option<usize>,
        namespaces: Option<&[&str]>,
        source_types: Option<&[SearchSourceType]>,
        view: StateView,
    ) -> Result<Vec<SearchResult>, MemoryError> {
        Ok(self
            .search_with_context_for_view(
                query,
                top_k,
                namespaces,
                source_types,
                SearchContext::default_now(),
                view,
            )
            .await?
            .results)
    }

    async fn search_with_context_for_view(
        &self,
        query: &str,
        top_k: Option<usize>,
        namespaces: Option<&[&str]>,
        source_types: Option<&[SearchSourceType]>,
        context: SearchContext,
        view: StateView,
    ) -> Result<SearchResponse, MemoryError> {
        let k = top_k
            .unwrap_or(self.inner.config.search.default_top_k)
            .min(MAX_TOP_K);

        // Fail closed: result caching is solely for ordinary current approximate
        // retrieval. Any governed/explained/exact/replay request must execute so
        // its receipt and execution semantics cannot be inherited from another
        // request. Recency-enabled searches also retain their evaluation-time
        // semantics by bypassing this cache.
        let cache_eligible = matches!(view, StateView::Current)
            && namespaces.is_none()
            && source_types.is_none()
            && context.receipt_mode == ReceiptMode::Disabled
            && context.replay_mode == ReplayMode::NoReplay
            && context.exactness_profile == ExactnessProfile::Default
            && self.inner.config.search.recency_half_life_days.is_none()
            && context.request_id.is_none()
            && context.trace_id.is_none()
            && context.attempt_family_id.is_none()
            && context.attempt_id.is_none()
            && context.replay_of.is_none()
            && context.query_text_digest.is_none()
            && context.query_input_digest.is_none()
            && context.filter_digest.is_none()
            && context.redaction_state.is_none()
            && context.budget_id.is_none()
            && context.deadline_at.is_none();
        let cache_epoch = if cache_eligible {
            Some(self.authority().current_retrieval_epoch().await?)
        } else {
            None
        };
        let cache_key = cache_epoch
            .map(|epoch| search_cache_key(query, k, active_vector_backend_generation(), epoch));
        if let Some(ref key) = cache_key {
            match self.inner.search_cache.lock() {
                Ok(mut cache) => {
                    if let Some(cached) = cache.get(key) {
                        if let Some(retrieval_epoch) = &cache_epoch {
                            if *retrieval_epoch == cached.retrieval_epoch {
                                return Ok(SearchResponse {
                                    results: cached.results.clone(),
                                    receipt: None,
                                });
                            }
                        } else {
                            return Ok(SearchResponse {
                                results: cached.results.clone(),
                                receipt: None,
                            });
                        }
                        cache.pop(key);
                    }
                }
                Err(err) => {
                    tracing::warn!(error = %err, "search cache lock poisoned; lookup skipped")
                }
            }
        }

        let (query_embedding, query_sparse) = if self.inner.config.search.sparse_weight > 0.0 {
            let (dense, sparse, _) = self
                .embed_text_with_sparse_internal(query, EmbeddingPurpose::Query)
                .await?;
            (dense, sparse)
        } else {
            (
                self.embed_text_internal(query, EmbeddingPurpose::Query)
                    .await?,
                None,
            )
        };

        #[cfg(feature = "hnsw")]
        let hnsw_hits = if context.exactness_profile == ExactnessProfile::PreferExact
            || self.inner.config.search.uses_turbo_quant_backend()
        {
            Vec::new()
        } else {
            let candidates = self
                .inner
                .config
                .search
                .candidate_pool_size
                .max(k.saturating_mul(3))
                .min(MAX_HNSW_CANDIDATES);
            self.hnsw_search_blocking(query_embedding.clone(), candidates)
                .await
        };

        let q = query.to_string();
        let config = self.inner.config.search.clone();
        let ns_owned = to_owned_string_vec(namespaces);
        let st_owned: Option<Vec<SearchSourceType>> = source_types.map(|s| s.to_vec());
        let context_owned = context.clone();

        #[cfg(feature = "hnsw")]
        let hnsw_hits_owned = hnsw_hits;

        let mut response = self
            .with_read_conn(move |conn| {
                if db::is_embeddings_dirty(conn)? {
                    tracing::warn!(
                        "Embeddings are stale after model change — search quality is degraded. \
                     Call reembed_all() to regenerate embeddings."
                    );
                }
                let ns_refs = as_str_slice(&ns_owned);
                let ns_slice: Option<&[&str]> = ns_refs.as_deref();
                let st_slice: Option<&[SearchSourceType]> = st_owned.as_deref();

                #[cfg(feature = "hnsw")]
                {
                    let mut execution = if hnsw_hits_owned.is_empty() {
                        search::hybrid_search_detailed_with_context(
                            conn,
                            &q,
                            &query_embedding,
                            query_sparse.as_ref(),
                            &config,
                            &context_owned,
                            k,
                            ns_slice,
                            st_slice,
                            None,
                        )
                    } else {
                        search::hybrid_search_with_hnsw_detailed_with_context(
                            conn,
                            &q,
                            &query_embedding,
                            query_sparse.as_ref(),
                            &config,
                            &context_owned,
                            k,
                            ns_slice,
                            st_slice,
                            None,
                            &hnsw_hits_owned,
                        )
                    }?;
                    if context_owned.receipts_enabled()
                        && context_owned.exactness_profile == ExactnessProfile::PreferExact
                    {
                        if let Some(receipt) = execution.receipt.as_mut() {
                            receipt.search_profile = "hybrid_prefer_exact".to_string();
                        }
                    }
                    Ok(SearchResponse {
                        results: dedup_by_content(
                            execution
                                .results
                                .into_iter()
                                .map(|result| result.result)
                                .collect(),
                        ),
                        receipt: execution.receipt,
                    })
                }
                #[cfg(not(feature = "hnsw"))]
                {
                    let execution = search::hybrid_search_detailed_with_context(
                        conn,
                        &q,
                        &query_embedding,
                        query_sparse.as_ref(),
                        &config,
                        &context_owned,
                        k,
                        ns_slice,
                        st_slice,
                        None,
                    )?;
                    Ok(SearchResponse {
                        results: dedup_by_content(
                            execution
                                .results
                                .into_iter()
                                .map(|result| result.result)
                                .collect(),
                        ),
                        receipt: execution.receipt,
                    })
                }
            })
            .await?;
        let raw_results = std::mem::take(&mut response.results);
        response.results = self
            .filter_search_results(raw_results, view.clone())
            .await?;
        response.results.truncate(k);
        if let Some(receipt) = &response.receipt {
            self.persist_search_receipt(
                receipt,
                query,
                namespaces,
                source_types,
                context.replay_mode,
            )
            .await?;
        }
        if let (Some(ref key), Some(retrieval_epoch)) = (cache_key.as_ref(), cache_epoch) {
            match self.inner.search_cache.lock() {
                Ok(mut cache) => {
                    cache.put(
                        key.to_string(),
                        CachedSearchResult {
                            results: response.results.clone(),
                            retrieval_epoch,
                        },
                    );
                }
                Err(err) => {
                    tracing::warn!(error = %err, "search cache lock poisoned; insert skipped")
                }
            }
        }
        Ok(response)
    }

    async fn filter_search_results(
        &self,
        results: Vec<SearchResult>,
        view: StateView,
    ) -> Result<Vec<SearchResult>, MemoryError> {
        self.with_read_conn(move |conn| {
            results
                .into_iter()
                .filter_map(|result| match &result.source {
                    SearchSource::Fact { fact_id, .. } => {
                        match knowledge::fact_is_visible_with_view(conn, fact_id, &view) {
                            Ok(true) => Some(Ok(result)),
                            Ok(false) => None,
                            Err(error) => Some(Err(error)),
                        }
                    }
                    SearchSource::Episode { episode_id, .. } => {
                        let invalidated = conn.query_row(
                            "SELECT EXISTS(SELECT 1 FROM forgetting_artifact_invalidations
                             WHERE surface_kind = 'episode' AND artifact_id = ?1)",
                            rusqlite::params![episode_id],
                            |row| row.get::<_, bool>(0),
                        );
                        match invalidated {
                            Ok(false) => Some(Ok(result)),
                            Ok(true) => None,
                            Err(error) => Some(Err(MemoryError::from(error))),
                        }
                    }
                    SearchSource::Projection { projection_id, .. } => {
                        let invalidated = conn.query_row(
                            "SELECT EXISTS(SELECT 1 FROM forgetting_artifact_invalidations
                             WHERE surface_kind = 'projection' AND artifact_id = ?1)",
                            rusqlite::params![projection_id],
                            |row| row.get::<_, bool>(0),
                        );
                        match invalidated {
                            Ok(false) => Some(Ok(result)),
                            Ok(true) => None,
                            Err(error) => Some(Err(MemoryError::from(error))),
                        }
                    }
                    _ => Some(Ok(result)),
                })
                .collect()
        })
        .await
    }

    /// Full-text search only (no embeddings needed).
    pub async fn search_fts_only(
        &self,
        query: &str,
        top_k: Option<usize>,
        namespaces: Option<&[&str]>,
        source_types: Option<&[SearchSourceType]>,
    ) -> Result<Vec<SearchResult>, MemoryError> {
        let k = top_k
            .unwrap_or(self.inner.config.search.default_top_k)
            .min(MAX_TOP_K);
        let q = query.to_string();
        let config = self.inner.config.search.clone();
        let ns_owned = to_owned_string_vec(namespaces);
        let st_owned: Option<Vec<SearchSourceType>> = source_types.map(|s| s.to_vec());
        let results = self
            .with_read_conn(move |conn| {
                let ns_refs = as_str_slice(&ns_owned);
                let ns_slice: Option<&[&str]> = ns_refs.as_deref();
                let st_slice: Option<&[SearchSourceType]> = st_owned.as_deref();
                search::fts_only_search(conn, &q, &config, k, ns_slice, st_slice, None)
            })
            .await?;
        self.filter_search_results(results, StateView::Current)
            .await
    }

    /// Full-text-only search with an explicit deterministic context and optional receipt.
    pub async fn search_fts_only_with_context(
        &self,
        query: &str,
        top_k: Option<usize>,
        namespaces: Option<&[&str]>,
        source_types: Option<&[SearchSourceType]>,
        context: SearchContext,
    ) -> Result<SearchResponse, MemoryError> {
        let k = top_k
            .unwrap_or(self.inner.config.search.default_top_k)
            .min(MAX_TOP_K);
        let q = query.to_string();
        let config = self.inner.config.search.clone();
        let ns_owned = to_owned_string_vec(namespaces);
        let st_owned: Option<Vec<SearchSourceType>> = source_types.map(|s| s.to_vec());
        let context_owned = context.clone();
        let mut response = self
            .with_read_conn(move |conn| {
                let ns_refs = as_str_slice(&ns_owned);
                let execution = search::fts_only_search_detailed_with_context(
                    conn,
                    &q,
                    &config,
                    &context_owned,
                    k,
                    ns_refs.as_deref(),
                    st_owned.as_deref(),
                    None,
                )?;
                Ok(SearchResponse {
                    results: execution
                        .results
                        .into_iter()
                        .map(|result| result.result)
                        .collect(),
                    receipt: execution.receipt,
                })
            })
            .await?;
        response.results = self
            .filter_search_results(response.results, StateView::Current)
            .await?;
        if let Some(receipt) = &response.receipt {
            self.persist_search_receipt(
                receipt,
                query,
                namespaces,
                source_types,
                context.replay_mode,
            )
            .await?;
        }
        Ok(response)
    }

    /// Vector similarity search only (no FTS).
    pub async fn search_vector_only(
        &self,
        query: &str,
        top_k: Option<usize>,
        namespaces: Option<&[&str]>,
        source_types: Option<&[SearchSourceType]>,
    ) -> Result<Vec<SearchResult>, MemoryError> {
        Ok(self
            .search_vector_only_with_context(
                query,
                top_k,
                namespaces,
                source_types,
                SearchContext::default_now(),
            )
            .await?
            .results)
    }

    /// Vector similarity search with an explicit deterministic context and optional receipt.
    pub async fn search_vector_only_with_context(
        &self,
        query: &str,
        top_k: Option<usize>,
        namespaces: Option<&[&str]>,
        source_types: Option<&[SearchSourceType]>,
        context: SearchContext,
    ) -> Result<SearchResponse, MemoryError> {
        let k = top_k
            .unwrap_or(self.inner.config.search.default_top_k)
            .min(MAX_TOP_K);
        let query_embedding = self
            .embed_text_internal(query, EmbeddingPurpose::Query)
            .await?;

        #[cfg(feature = "hnsw")]
        let hnsw_hits = if context.exactness_profile == ExactnessProfile::PreferExact
            || self.inner.config.search.uses_turbo_quant_backend()
        {
            Vec::new()
        } else {
            let candidates = self
                .inner
                .config
                .search
                .candidate_pool_size
                .max(k.saturating_mul(3))
                .min(MAX_HNSW_CANDIDATES);
            self.hnsw_search_blocking(query_embedding.clone(), candidates)
                .await
        };

        let config = self.inner.config.search.clone();
        let ns_owned = to_owned_string_vec(namespaces);
        let st_owned: Option<Vec<SearchSourceType>> = source_types.map(|s| s.to_vec());
        let context_owned = context.clone();

        #[cfg(feature = "hnsw")]
        let hnsw_hits_owned = hnsw_hits;

        let mut response = self
            .with_read_conn(move |conn| {
                if db::is_embeddings_dirty(conn)? {
                    tracing::warn!(
                        "Embeddings are stale after model change — search quality is degraded. \
                     Call reembed_all() to regenerate embeddings."
                    );
                }
                let ns_refs = as_str_slice(&ns_owned);
                let ns_slice: Option<&[&str]> = ns_refs.as_deref();
                let st_slice: Option<&[SearchSourceType]> = st_owned.as_deref();

                #[cfg(feature = "hnsw")]
                {
                    let mut execution = if hnsw_hits_owned.is_empty() {
                        search::vector_only_search_detailed_with_context(
                            conn,
                            &query_embedding,
                            &config,
                            &context_owned,
                            k,
                            ns_slice,
                            st_slice,
                            None,
                        )
                    } else {
                        search::vector_only_search_with_hnsw_detailed_with_context(
                            conn,
                            &query_embedding,
                            &config,
                            &context_owned,
                            k,
                            ns_slice,
                            st_slice,
                            None,
                            &hnsw_hits_owned,
                        )
                    }?;
                    if context_owned.receipts_enabled()
                        && context_owned.exactness_profile == ExactnessProfile::PreferExact
                    {
                        if let Some(receipt) = execution.receipt.as_mut() {
                            receipt.search_profile = "vector_only_prefer_exact".to_string();
                        }
                    }
                    Ok(SearchResponse {
                        results: execution
                            .results
                            .into_iter()
                            .map(|result| result.result)
                            .collect(),
                        receipt: execution.receipt,
                    })
                }
                #[cfg(not(feature = "hnsw"))]
                {
                    let execution = search::vector_only_search_detailed_with_context(
                        conn,
                        &query_embedding,
                        &config,
                        &context_owned,
                        k,
                        ns_slice,
                        st_slice,
                        None,
                    )?;
                    Ok(SearchResponse {
                        results: execution
                            .results
                            .into_iter()
                            .map(|result| result.result)
                            .collect(),
                        receipt: execution.receipt,
                    })
                }
            })
            .await?;
        response.results = self
            .filter_search_results(response.results, StateView::Current)
            .await?;
        if let Some(receipt) = &response.receipt {
            self.persist_search_receipt(
                receipt,
                query,
                namespaces,
                source_types,
                context.replay_mode,
            )
            .await?;
        }
        Ok(response)
    }

    // ─── Explainable Search ───────────────────────────────────

    /// Search with full score breakdown for each result.
    pub async fn search_explained(
        &self,
        query: &str,
        top_k: Option<usize>,
        namespaces: Option<&[&str]>,
        source_types: Option<&[SearchSourceType]>,
    ) -> Result<Vec<types::ExplainedResult>, MemoryError> {
        Ok(self
            .search_explained_with_context(
                query,
                top_k,
                namespaces,
                source_types,
                SearchContext::default_now(),
            )
            .await?
            .results)
    }

    /// Search with full score breakdown under an explicit deterministic context.
    pub async fn search_explained_with_context(
        &self,
        query: &str,
        top_k: Option<usize>,
        namespaces: Option<&[&str]>,
        source_types: Option<&[SearchSourceType]>,
        context: SearchContext,
    ) -> Result<types::ExplainedSearchResponse, MemoryError> {
        let k = top_k
            .unwrap_or(self.inner.config.search.default_top_k)
            .min(MAX_TOP_K);
        let (query_embedding, query_sparse) = if self.inner.config.search.sparse_weight > 0.0 {
            let (dense, sparse, _) = self
                .embed_text_with_sparse_internal(query, EmbeddingPurpose::Query)
                .await?;
            (dense, sparse)
        } else {
            (
                self.embed_text_internal(query, EmbeddingPurpose::Query)
                    .await?,
                None,
            )
        };

        #[cfg(feature = "hnsw")]
        let hnsw_hits = if context.exactness_profile == ExactnessProfile::PreferExact {
            Vec::new()
        } else {
            let candidates = self
                .inner
                .config
                .search
                .candidate_pool_size
                .max(k.saturating_mul(3))
                .min(MAX_HNSW_CANDIDATES);
            self.hnsw_search_blocking(query_embedding.clone(), candidates)
                .await
        };

        let q = query.to_string();
        let config = self.inner.config.search.clone();
        let ns_owned = to_owned_string_vec(namespaces);
        let st_owned: Option<Vec<SearchSourceType>> = source_types.map(|value| value.to_vec());
        let context_owned = context.clone();

        #[cfg(feature = "hnsw")]
        let hnsw_hits_owned = hnsw_hits;

        let response = self
            .with_read_conn(move |conn| {
                let ns_refs = as_str_slice(&ns_owned);
                let ns_slice: Option<&[&str]> = ns_refs.as_deref();
                let st_slice: Option<&[SearchSourceType]> = st_owned.as_deref();

                #[cfg(feature = "hnsw")]
                {
                    let mut execution = if hnsw_hits_owned.is_empty() {
                        search::hybrid_search_detailed_with_context(
                            conn,
                            &q,
                            &query_embedding,
                            query_sparse.as_ref(),
                            &config,
                            &context_owned,
                            k,
                            ns_slice,
                            st_slice,
                            None,
                        )
                    } else {
                        search::hybrid_search_with_hnsw_detailed_with_context(
                            conn,
                            &q,
                            &query_embedding,
                            query_sparse.as_ref(),
                            &config,
                            &context_owned,
                            k,
                            ns_slice,
                            st_slice,
                            None,
                            &hnsw_hits_owned,
                        )
                    }?;
                    if context_owned.receipts_enabled()
                        && context_owned.exactness_profile == ExactnessProfile::PreferExact
                    {
                        if let Some(receipt) = execution.receipt.as_mut() {
                            receipt.search_profile = "hybrid_prefer_exact".to_string();
                        }
                    }
                    Ok(types::ExplainedSearchResponse {
                        results: execution.results,
                        receipt: execution.receipt,
                    })
                }
                #[cfg(not(feature = "hnsw"))]
                {
                    let execution = search::hybrid_search_detailed_with_context(
                        conn,
                        &q,
                        &query_embedding,
                        query_sparse.as_ref(),
                        &config,
                        &context_owned,
                        k,
                        ns_slice,
                        st_slice,
                        None,
                    )?;
                    Ok(types::ExplainedSearchResponse {
                        results: execution.results,
                        receipt: execution.receipt,
                    })
                }
            })
            .await?;
        if let Some(receipt) = &response.receipt {
            self.persist_search_receipt(
                receipt,
                query,
                namespaces,
                source_types,
                context.replay_mode,
            )
            .await?;
        }
        Ok(response)
    }

    /// Load a durable search receipt by receipt/request ID.
    pub async fn get_search_receipt(
        &self,
        receipt_id: &str,
    ) -> Result<Option<VectorSearchReceiptV1>, MemoryError> {
        let receipt_id = receipt_id.to_string();
        self.with_read_conn(move |conn| db::get_search_receipt(conn, &receipt_id))
            .await
    }

    /// Return whether a durable receipt has opt-in inputs for complete replay.
    pub async fn search_replay_inputs_available(
        &self,
        receipt_id: &str,
    ) -> Result<bool, MemoryError> {
        let receipt_id = receipt_id.to_string();
        self.with_read_conn(move |conn| Ok(db::get_replay_inputs(conn, &receipt_id)?.is_some()))
            .await
    }

    /// Replay a durable receipt using its opt-in stored query and filters.
    pub async fn replay_search_from_stored_inputs(
        &self,
        receipt_id: &str,
    ) -> Result<SearchReplayReportV1, MemoryError> {
        self.get_search_receipt(receipt_id).await?.ok_or_else(|| {
            MemoryError::SearchReceiptNotFound {
                receipt_id: receipt_id.to_string(),
            }
        })?;
        let replay_receipt_id = receipt_id.to_string();
        let inputs = self
            .with_read_conn(move |conn| db::get_replay_inputs(conn, &replay_receipt_id))
            .await?
            .ok_or_else(|| {
                MemoryError::Other(format!(
                    "search receipt '{receipt_id}' has no stored replay inputs"
                ))
            })?;
        let namespace_refs: Option<Vec<&str>> = inputs
            .namespaces
            .as_ref()
            .map(|values| values.iter().map(String::as_str).collect());
        self.replay_search_receipt(
            receipt_id,
            &inputs.query_text,
            None,
            namespace_refs.as_deref(),
            inputs.source_types.as_deref(),
        )
        .await
    }

    /// Replay a durable search receipt with caller-supplied query text and filters.
    ///
    /// Receipts intentionally do not store query text or filter values. The
    /// caller supplies those inputs, and the stored receipt supplies the
    /// deterministic evaluation time and retrieval family for comparison.
    pub async fn replay_search_receipt(
        &self,
        receipt_id: &str,
        query: &str,
        top_k: Option<usize>,
        namespaces: Option<&[&str]>,
        source_types: Option<&[SearchSourceType]>,
    ) -> Result<SearchReplayReportV1, MemoryError> {
        let invalidation_id = receipt_id.to_string();
        let invalidated = self
            .with_read_conn(move |conn| {
                conn.query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM forgetting_artifact_invalidations
                         WHERE surface_kind = 'search_receipt' AND artifact_id = ?1
                     )",
                    rusqlite::params![invalidation_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(MemoryError::from)
            })
            .await?;
        if invalidated {
            return Err(MemoryError::ForgettingClosureIncomplete {
                detail: format!(
                    "search receipt '{receipt_id}' was invalidated by selective forgetting"
                ),
            });
        }
        let original_receipt = self.get_search_receipt(receipt_id).await?.ok_or_else(|| {
            MemoryError::SearchReceiptNotFound {
                receipt_id: receipt_id.to_string(),
            }
        })?;

        let vector_only = original_receipt.search_profile.starts_with("vector_only");
        let fts_only = original_receipt.search_profile.starts_with("fts_only");
        let replay_top_k = top_k.or_else(|| Some(original_receipt.result_ids.len().max(1)));
        let replay_receipt_id = format!("{receipt_id}:replay:{}", uuid::Uuid::new_v4());
        let mut context = SearchContext::at(original_receipt.evaluation_time);
        context.receipt_mode = ReceiptMode::ReturnReceipt;
        context.request_id = Some(replay_receipt_id.clone());
        context.trace_id = original_receipt.trace_id.clone();
        context.attempt_family_id = original_receipt
            .attempt_family_id
            .clone()
            .or_else(|| Some(original_receipt.receipt_id.clone()));
        context.attempt_id = Some(replay_receipt_id.clone());
        context.replay_of = Some(original_receipt.receipt_id.clone());
        context.query_text_digest = original_receipt.query_text_digest.clone();
        context.query_input_digest = original_receipt.query_input_digest.clone();
        context.filter_digest = original_receipt.filter_digest.clone();
        context.redaction_state = original_receipt.redaction_state.clone();
        context.budget_id = original_receipt.budget_id.clone();
        context.exactness_profile = if original_receipt.approximate {
            ExactnessProfile::AllowApproximate
        } else {
            ExactnessProfile::PreferExact
        };

        let replay_response = if vector_only {
            self.search_vector_only_with_context(
                query,
                replay_top_k,
                namespaces,
                source_types,
                context,
            )
            .await?
        } else if fts_only {
            self.search_fts_only_with_context(
                query,
                replay_top_k,
                namespaces,
                source_types,
                context,
            )
            .await?
        } else {
            self.search_with_context(query, replay_top_k, namespaces, source_types, context)
                .await?
        };
        let replay_receipt = replay_response
            .receipt
            .ok_or_else(|| MemoryError::Other("replay did not produce a receipt".to_string()))?;

        let query_embedding_digest_matches =
            original_receipt.query_embedding_digest == replay_receipt.query_embedding_digest;
        let result_ids_match = original_receipt.result_ids == replay_receipt.result_ids;
        let missing_result_ids = original_receipt
            .result_ids
            .iter()
            .filter(|id| !replay_receipt.result_ids.contains(*id))
            .cloned()
            .collect();
        let added_result_ids = replay_receipt
            .result_ids
            .iter()
            .filter(|id| !original_receipt.result_ids.contains(*id))
            .cloned()
            .collect();

        Ok(SearchReplayReportV1 {
            receipt_id: original_receipt.receipt_id.clone(),
            replay_receipt_id,
            original_receipt,
            replay_receipt,
            query_embedding_digest_matches,
            result_ids_match,
            missing_result_ids,
            added_result_ids,
            vector_only,
        })
    }

    // ─── Embedding Displacement ───────────────────────────────

    /// Compute embedding displacement between two texts.
    pub async fn embedding_displacement(
        &self,
        text_a: &str,
        text_b: &str,
    ) -> Result<types::EmbeddingDisplacement, MemoryError> {
        let emb_a = self
            .embed_text_internal(text_a, EmbeddingPurpose::Query)
            .await?;
        let emb_b = self
            .embed_text_internal(text_b, EmbeddingPurpose::Query)
            .await?;
        Self::embedding_displacement_from_vecs(&emb_a, &emb_b)
    }

    /// Compute embedding displacement from pre-computed vectors.
    pub fn embedding_displacement_from_vecs(
        a: &[f32],
        b: &[f32],
    ) -> Result<types::EmbeddingDisplacement, MemoryError> {
        if a.len() != b.len() {
            return Err(MemoryError::DimensionMismatch {
                expected: a.len(),
                actual: b.len(),
            });
        }
        let cosine_sim = search::cosine_similarity(a, b)?;

        let euclidean_dist: f32 = a
            .iter()
            .zip(b.iter())
            .map(|(x, y)| (x - y) * (x - y))
            .sum::<f32>()
            .sqrt();

        let mag_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let mag_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

        Ok(types::EmbeddingDisplacement {
            cosine_similarity: cosine_sim,
            euclidean_distance: euclidean_dist,
            magnitude_a: mag_a,
            magnitude_b: mag_b,
        })
    }

    // ─── Utility ────────────────────────────────────────────────

    /// Chunk text using the configured strategy and token counter.
    pub fn chunk_text(&self, text: &str) -> Vec<TextChunk> {
        chunker::chunk_text(
            text,
            &self.inner.config.chunking,
            self.inner.token_counter.as_ref(),
        )
    }

    /// Embed a single text via the configured provider.
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>, MemoryError> {
        self.embed_query(text).await
    }

    /// Embed retrieval text using the query role.
    pub async fn embed_query(&self, text: &str) -> Result<Vec<f32>, MemoryError> {
        self.embed_text_internal(text, EmbeddingPurpose::Query)
            .await
    }

    /// Embed stored content using the document role.
    pub async fn embed_document(&self, text: &str) -> Result<Vec<f32>, MemoryError> {
        self.embed_text_internal(text, EmbeddingPurpose::Document)
            .await
    }

    /// Embed multiple stored texts in a batch.
    pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, MemoryError> {
        self.embed_documents_batch(texts).await
    }

    /// Embed multiple stored texts using the document role.
    pub async fn embed_documents_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, MemoryError> {
        let owned: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        self.embed_batch_internal(owned, EmbeddingPurpose::Document)
            .await
    }

    /// Embed multiple retrieval texts using the query role.
    pub async fn embed_queries_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, MemoryError> {
        let owned: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        self.embed_batch_internal(owned, EmbeddingPurpose::Query)
            .await
    }

    /// Get database statistics.
    pub async fn stats(&self) -> Result<MemoryStats, MemoryError> {
        let db_path = self.inner.paths.sqlite_path.clone();
        self.with_read_conn(move |conn| {
            let total_facts: u64 =
                conn.query_row("SELECT COUNT(*) FROM facts", [], |r| r.get(0))?;
            let total_documents: u64 =
                conn.query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))?;
            let total_chunks: u64 =
                conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
            let total_sessions: u64 =
                conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))?;
            let total_messages: u64 =
                conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))?;

            let db_size = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);

            let (model, dims): (Option<String>, Option<usize>) = conn
                .query_row(
                    "SELECT model_name, dimensions FROM embedding_metadata WHERE id = 1",
                    [],
                    |r| Ok((Some(r.get(0)?), Some(r.get(1)?))),
                )
                .unwrap_or((None, None));

            Ok(MemoryStats {
                total_facts,
                total_documents,
                total_chunks,
                total_sessions,
                total_messages,
                database_size_bytes: db_size,
                embedding_model: model,
                embedding_dimensions: dims,
                vector_backend: active_vector_backend().to_string(),
                vector_backend_generation: active_vector_backend_generation().to_string(),
            })
        })
        .await
    }

    /// Return distinct scope_domain values stored in document metadata.
    ///
    /// Queries `json_extract(metadata, '$.scope_domain')` across all documents
    /// and returns the unique non-null values. Used by the Recall app to populate
    /// the scope picker dynamically instead of relying on a hardcoded list.
    pub async fn list_scope_domains(&self) -> Result<Vec<String>, MemoryError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT json_extract(metadata, '$.scope_domain') \
                 FROM documents \
                 WHERE json_extract(metadata, '$.scope_domain') IS NOT NULL",
            )?;
            let domains: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .filter_map(|r| r.ok())
                .collect();
            Ok(domains)
        })
        .await
    }

    /// Check if embeddings need re-generation after a model change.
    pub async fn embeddings_are_dirty(&self) -> Result<bool, MemoryError> {
        self.with_read_conn(db::is_embeddings_dirty).await
    }

    /// Re-embed all facts, chunks, messages, and episodes. Call after changing embedding models.
    #[cfg(feature = "admin-ops")]
    pub async fn reembed_all(&self) -> Result<usize, MemoryError> {
        let mut count = 0usize;
        let batch_size = self.inner.config.embedding.batch_size;
        let dims = self.inner.config.embedding.dimensions;

        // ─── Facts ──────────────────────────────────────────────────
        let fact_contents: Vec<(String, String)> = self
            .with_read_conn(|conn| {
                let mut stmt = conn.prepare("SELECT id, content FROM facts")?;
                let result = stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(result)
            })
            .await?;

        let mut fact_count = 0usize;
        for batch in fact_contents.chunks(batch_size) {
            let texts: Vec<String> = batch.iter().map(|(_, c)| c.clone()).collect();
            let embeddings = self
                .embed_batch_with_sparse_internal(texts, EmbeddingPurpose::Document)
                .await?;

            let quantizer = Quantizer::new(dims);
            let updates: Vec<_> = batch
                .iter()
                .zip(embeddings.iter())
                .map(|((id, _), (emb, sparse, representation))| {
                    // INTENTIONAL: q8 quantization is an optional search optimization; missing q8 is non-fatal
                    let q8 = quantizer
                        .quantize(emb)
                        .map(|qv| quantize::pack_quantized(&qv))
                        .ok();
                    (
                        id.clone(),
                        db::embedding_to_bytes(emb),
                        q8,
                        sparse.clone(),
                        representation.clone(),
                    )
                })
                .collect();

            self.with_write_conn(move |conn| {
                db::with_transaction(conn, |tx| {
                    for (fid, bytes, q8, sparse, representation) in &updates {
                        tx.execute(
                            "UPDATE facts SET embedding = ?1, embedding_q8 = ?2, updated_at = datetime('now') WHERE id = ?3",
                            rusqlite::params![bytes, q8.as_deref(), fid],
                        )?;
                        #[cfg(feature = "hnsw")]
                        db::queue_pending_index_op(
                            tx,
                            &format!("fact:{fid}"),
                            "fact",
                            db::IndexOpKind::Upsert,
                        )?;
                        db::invalidate_derived_vector_artifact(tx, &format!("fact:{fid}"))?;
                        if let Some((weights, representation)) =
                            sparse.as_ref().zip(representation.as_deref())
                        {
                            db::store_sparse_vector(
                                tx,
                                &format!("fact:{fid}"),
                                weights,
                                representation,
                            )?;
                        } else {
                            db::delete_sparse_vector(tx, &format!("fact:{fid}"))?;
                        }
                    }
                    Ok(())
                })
            })
            .await?;

            fact_count += batch.len();
            count += batch.len();
            if fact_count % 100 == 0 || fact_count == count {
                tracing::info!(fact_count, "Re-embedded {} facts so far", fact_count);
            }
        }

        // ─── Chunks ─────────────────────────────────────────────────
        let chunk_data: Vec<(String, String)> = self
            .with_read_conn(|conn| {
                let mut stmt = conn.prepare("SELECT id, content FROM chunks")?;
                let result = stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(result)
            })
            .await?;

        let mut chunk_count = 0usize;
        for batch in chunk_data.chunks(batch_size) {
            let texts: Vec<String> = batch.iter().map(|(_, c)| c.clone()).collect();
            let embeddings = self
                .embed_batch_with_sparse_internal(texts, EmbeddingPurpose::Document)
                .await?;

            let quantizer = Quantizer::new(dims);
            let updates: Vec<_> = batch
                .iter()
                .zip(embeddings.iter())
                .map(|((id, _), (emb, sparse, representation))| {
                    // INTENTIONAL: q8 quantization is an optional search optimization; missing q8 is non-fatal
                    let q8 = quantizer
                        .quantize(emb)
                        .map(|qv| quantize::pack_quantized(&qv))
                        .ok();
                    (
                        id.clone(),
                        db::embedding_to_bytes(emb),
                        q8,
                        sparse.clone(),
                        representation.clone(),
                    )
                })
                .collect();

            self.with_write_conn(move |conn| {
                db::with_transaction(conn, |tx| {
                    for (cid, bytes, q8, sparse, representation) in &updates {
                        tx.execute(
                            "UPDATE chunks SET embedding = ?1, embedding_q8 = ?2 WHERE id = ?3",
                            rusqlite::params![bytes, q8.as_deref(), cid],
                        )?;
                        #[cfg(feature = "hnsw")]
                        db::queue_pending_index_op(
                            tx,
                            &format!("chunk:{cid}"),
                            "chunk",
                            db::IndexOpKind::Upsert,
                        )?;
                        db::invalidate_derived_vector_artifact(tx, &format!("chunk:{cid}"))?;
                        if let Some((weights, representation)) =
                            sparse.as_ref().zip(representation.as_deref())
                        {
                            db::store_sparse_vector(
                                tx,
                                &format!("chunk:{cid}"),
                                weights,
                                representation,
                            )?;
                        } else {
                            db::delete_sparse_vector(tx, &format!("chunk:{cid}"))?;
                        }
                    }
                    Ok(())
                })
            })
            .await?;

            chunk_count += batch.len();
            count += batch.len();
            if chunk_count % 100 == 0 {
                tracing::info!(chunk_count, "Re-embedded {} chunks so far", chunk_count);
            }
        }

        // ─── Messages ───────────────────────────────────────────────
        let message_data: Vec<(i64, String)> = self
            .with_read_conn(|conn| {
                let mut stmt = conn.prepare("SELECT id, content FROM messages")?;
                let result = stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(result)
            })
            .await?;

        let mut msg_count = 0usize;
        for batch in message_data.chunks(batch_size) {
            let texts: Vec<String> = batch.iter().map(|(_, c)| c.clone()).collect();
            let embeddings = self
                .embed_batch_with_sparse_internal(texts, EmbeddingPurpose::Document)
                .await?;

            let quantizer = Quantizer::new(dims);
            let updates: Vec<_> = batch
                .iter()
                .zip(embeddings.iter())
                .map(|((id, _), (emb, sparse, representation))| {
                    // INTENTIONAL: q8 quantization is an optional search optimization; missing q8 is non-fatal
                    let q8 = quantizer
                        .quantize(emb)
                        .map(|qv| quantize::pack_quantized(&qv))
                        .ok();
                    (
                        *id,
                        db::embedding_to_bytes(emb),
                        q8,
                        sparse.clone(),
                        representation.clone(),
                    )
                })
                .collect();

            self.with_write_conn(move |conn| {
                db::with_transaction(conn, |tx| {
                    for (mid, bytes, q8, sparse, representation) in &updates {
                        tx.execute(
                            "UPDATE messages SET embedding = ?1, embedding_q8 = ?2 WHERE id = ?3",
                            rusqlite::params![bytes, q8.as_deref(), mid],
                        )?;
                        #[cfg(feature = "hnsw")]
                        db::queue_pending_index_op(
                            tx,
                            &format!("msg:{mid}"),
                            "message",
                            db::IndexOpKind::Upsert,
                        )?;
                        db::invalidate_derived_vector_artifact(tx, &format!("msg:{mid}"))?;
                        if let Some((weights, representation)) =
                            sparse.as_ref().zip(representation.as_deref())
                        {
                            db::store_sparse_vector(
                                tx,
                                &format!("msg:{mid}"),
                                weights,
                                representation,
                            )?;
                        } else {
                            db::delete_sparse_vector(tx, &format!("msg:{mid}"))?;
                        }
                    }
                    Ok(())
                })
            })
            .await?;

            msg_count += batch.len();
            count += batch.len();
            if msg_count % 100 == 0 {
                tracing::info!(msg_count, "Re-embedded {} messages so far", msg_count);
            }
        }

        // ─── Episodes ───────────────────────────────────────────────
        let episode_data: Vec<(String, String)> = self
            .with_read_conn(|conn| {
                let mut stmt = conn.prepare("SELECT episode_id, search_text FROM episodes")?;
                let result = stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(result)
            })
            .await?;

        let mut episode_count = 0usize;
        for batch in episode_data.chunks(batch_size) {
            let texts: Vec<String> = batch.iter().map(|(_, text)| text.clone()).collect();
            let embeddings = self
                .embed_batch_with_sparse_internal(texts, EmbeddingPurpose::Document)
                .await?;

            let quantizer = Quantizer::new(dims);
            let updates: Vec<_> = batch
                .iter()
                .zip(embeddings.iter())
                .map(|((episode_id, _), (embedding, sparse, representation))| {
                    // INTENTIONAL: q8 quantization is an optional search optimization; missing q8 is non-fatal
                    let q8 = quantizer
                        .quantize(embedding)
                        .map(|vector| quantize::pack_quantized(&vector))
                        .ok();
                    (
                        episode_id.clone(),
                        db::embedding_to_bytes(embedding),
                        q8,
                        sparse.clone(),
                        representation.clone(),
                    )
                })
                .collect();

            self.with_write_conn(move |conn| {
                db::with_transaction(conn, |tx| {
                    for (episode_id, bytes, q8, sparse, representation) in &updates {
                        tx.execute(
                            "UPDATE episodes
                             SET embedding = ?1,
                                 embedding_q8 = ?2,
                                 updated_at = datetime('now')
                             WHERE episode_id = ?3",
                            rusqlite::params![bytes, q8.as_deref(), episode_id],
                        )?;
                        #[cfg(feature = "hnsw")]
                        db::queue_pending_index_op(
                            tx,
                            &episodes::episode_item_key(episode_id),
                            "episode",
                            db::IndexOpKind::Upsert,
                        )?;
                        db::invalidate_derived_vector_artifact(
                            tx,
                            &episodes::episode_item_key(episode_id),
                        )?;
                        let item_key = episodes::episode_item_key(episode_id);
                        if let Some((weights, representation)) =
                            sparse.as_ref().zip(representation.as_deref())
                        {
                            db::store_sparse_vector(tx, &item_key, weights, representation)?;
                        } else {
                            db::delete_sparse_vector(tx, &item_key)?;
                        }
                    }
                    Ok(())
                })
            })
            .await?;

            episode_count += batch.len();
            count += batch.len();
            if episode_count % 100 == 0 {
                tracing::info!(
                    episode_count,
                    "Re-embedded {} episodes so far",
                    episode_count
                );
            }
        }

        // Clear the dirty flag
        self.with_write_conn(db::clear_embeddings_dirty).await?;

        tracing::info!(
            facts = fact_count,
            chunks = chunk_count,
            messages = msg_count,
            episodes = episode_count,
            total = count,
            "Re-embedding complete"
        );

        // Rebuild HNSW after re-embedding
        #[cfg(feature = "hnsw")]
        {
            tracing::info!("Rebuilding HNSW index after re-embedding...");
            let _receipt = self.rebuild_hnsw_index().await?;
        }

        Ok(count)
    }

    /// Vacuum the database (reclaim space after deletions).
    pub async fn vacuum(&self) -> Result<(), MemoryError> {
        self.with_write_conn(|conn| {
            conn.execute_batch("VACUUM")?;
            Ok(())
        })
        .await
    }

    // ─── Routing policy persistence ──────────────────────────────

    /// Save a routing policy to the database as JSON.
    ///
    /// Creates the `routing_policy` table if it doesn't exist and upserts
    /// the serialized policy into the single-row table (id=1).
    #[cfg(feature = "rl-routing")]
    pub async fn save_routing_policy(
        &self,
        policy: &rl_routing::RoutingPolicy,
    ) -> Result<(), MemoryError> {
        let json = serde_json::to_string(policy)
            .map_err(|e| MemoryError::Other(format!("Failed to serialize routing policy: {e}")))?;
        let updated_at = chrono::Utc::now().to_rfc3339();
        self.with_write_conn(move |conn| {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS routing_policy (\
                 id INTEGER PRIMARY KEY, policy_json TEXT NOT NULL, updated_at TEXT NOT NULL)",
            )?;
            conn.execute(
                "INSERT INTO routing_policy (id, policy_json, updated_at) VALUES (1, ?1, ?2) \
                 ON CONFLICT(id) DO UPDATE SET policy_json = ?1, updated_at = ?2",
                rusqlite::params![json, updated_at],
            )?;
            Ok(())
        })
        .await
    }

    /// Load the persisted routing policy from the database.
    ///
    /// Returns `Ok(None)` if no policy has been saved yet.
    #[cfg(feature = "rl-routing")]
    pub async fn load_routing_policy(
        &self,
    ) -> Result<Option<rl_routing::RoutingPolicy>, MemoryError> {
        self.with_read_conn(move |conn| {
            // Check if table exists
            let table_exists: bool = conn
                .query_row(
                    "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type='table' AND name='routing_policy')",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(false);
            if !table_exists {
                return Ok(None);
            }
            let json: Option<String> = conn
                .query_row(
                    "SELECT policy_json FROM routing_policy WHERE id = 1",
                    [],
                    |row| row.get(0),
                )
                .ok();
            match json {
                Some(j) => {
                    let policy = serde_json::from_str(&j).map_err(|e| {
                        MemoryError::Other(format!("Failed to deserialize routing policy: {e}"))
                    })?;
                    Ok(Some(policy))
                }
                None => Ok(None),
            }
        })
        .await
    }

    // ─── Projection Import ─────────────────────────────────────

    /// Import a projection envelope atomically (V10 legacy path).
    ///
    /// ## Phase status: compatibility / migration-only
    ///
    /// This method is the V10 legacy import path. New integrations should use
    /// [`import_projection_batch()`](Self::import_projection_batch) instead,
    /// which accepts the canonical `ProjectionImportBatchV3` format from
    /// `forge-memory-bridge`.
    ///
    /// **Removal condition**: removed when all callers migrate to the bridge pipeline.
    ///
    /// **Idempotent**: re-importing the same envelope (same `envelope_id` +
    /// `schema_version` + `content_digest`) returns a receipt with
    /// `was_duplicate = true` and does not modify data.
    ///
    /// **Atomic**: all records are committed in a single transaction. On any
    /// failure the entire import is rolled back — no partial visibility.
    ///
    /// **Provenance**: each imported record's metadata is tagged with the
    /// envelope_id and source_authority for traceability.
    #[deprecated(
        since = "0.5.0",
        note = "Legacy V10 import envelope path is compatibility-only. Use `import_projection_batch()` and `ProjectionImportBatchV3` on the canonical lane."
    )]
    #[doc(hidden)]
    #[allow(deprecated)]
    pub async fn import_envelope(
        &self,
        envelope: &projection_import::ImportEnvelope,
    ) -> Result<projection_import::ImportReceipt, MemoryError> {
        projection_legacy_compat::import_envelope(self, envelope).await
    }

    /// Check whether an envelope has already been imported.
    #[deprecated(
        since = "0.5.0",
        note = "Legacy V10 import envelope status reads are compatibility-only. Prefer the projection import log."
    )]
    #[doc(hidden)]
    #[allow(deprecated)]
    pub async fn import_status(
        &self,
        envelope_id: &projection_import::EnvelopeId,
    ) -> Result<Vec<projection_import::ImportReceipt>, MemoryError> {
        projection_legacy_compat::import_status(self, envelope_id).await
    }

    /// List recent imports, optionally filtered by namespace.
    #[deprecated(
        since = "0.5.0",
        note = "Legacy V10 import log access is compatibility-only. Prefer new projection-import metadata."
    )]
    #[doc(hidden)]
    #[allow(deprecated)]
    pub async fn list_imports(
        &self,
        namespace: Option<&str>,
        limit: usize,
    ) -> Result<Vec<projection_import::ImportReceipt>, MemoryError> {
        projection_legacy_compat::list_imports(self, namespace, limit).await
    }

    /// Get the most recent successful import timestamp for a namespace.
    #[allow(deprecated)]
    pub async fn last_import_at(&self, namespace: &str) -> Result<Option<String>, MemoryError> {
        projection_legacy_compat::last_import_at(self, namespace).await
    }

    /// Query imported claim projection rows through the supported public read surface.
    pub async fn query_claim_versions(
        &self,
        query: ProjectionQuery,
    ) -> Result<Vec<ProjectionClaimVersion>, MemoryError> {
        self.with_read_conn(move |conn| projection_storage::query_claim_versions(conn, &query))
            .await
    }

    /// Query imported relation projection rows through the supported public read surface.
    pub async fn query_relation_versions(
        &self,
        query: ProjectionQuery,
    ) -> Result<Vec<ProjectionRelationVersion>, MemoryError> {
        self.with_read_conn(move |conn| projection_storage::query_relation_versions(conn, &query))
            .await
    }

    /// Query imported episode projection rows through the supported public read surface.
    pub async fn query_episodes(
        &self,
        query: ProjectionQuery,
    ) -> Result<Vec<ProjectionEpisode>, MemoryError> {
        self.with_read_conn(move |conn| projection_storage::query_episode_rows(conn, &query))
            .await
    }

    /// Query imported entity-alias rows through the supported public read surface.
    pub async fn query_entity_aliases(
        &self,
        query: ProjectionQuery,
    ) -> Result<Vec<ProjectionEntityAlias>, MemoryError> {
        self.with_read_conn(move |conn| projection_storage::query_entity_aliases(conn, &query))
            .await
    }

    /// Query imported evidence-reference rows through the supported public read surface.
    pub async fn query_evidence_refs(
        &self,
        query: ProjectionQuery,
    ) -> Result<Vec<ProjectionEvidenceRef>, MemoryError> {
        self.with_read_conn(move |conn| projection_storage::query_evidence_refs(conn, &query))
            .await
    }

    /// Governed projection reads fail closed until imported rows have durable origin labels.
    /// The ungoverned projection methods above remain the explicit storage compatibility surface;
    /// no governed method delegates to them after authorization.
    pub async fn query_claim_versions_governed(
        &self,
        query: ProjectionQuery,
        request: GovernedAccessRequestV1,
    ) -> Result<GovernedProjectionResponseV1<ProjectionClaimVersion>, MemoryError> {
        let query_namespace = query.scope.namespace.clone();
        let rows = if query_namespace == request.scope.namespace {
            self.with_read_conn(move |conn| projection_storage::query_claim_versions(conn, &query))
                .await?
        } else {
            Vec::new()
        };
        let mut decisions = Vec::new();
        for row in &rows {
            decisions.push(origin_authority::evaluate_governed_access_v1(
                row.claim_version_id.as_str(),
                Some(&row.scope_key.namespace),
                None,
                None,
                &request,
            ));
        }
        if query_namespace != request.scope.namespace {
            decisions.push(origin_authority::evaluate_governed_access_v1(
                "projection:query",
                Some(&query_namespace),
                None,
                None,
                &request,
            ));
        }
        Ok(GovernedProjectionResponseV1 {
            items: Vec::new(),
            decisions,
        })
    }

    pub async fn query_relation_versions_governed(
        &self,
        query: ProjectionQuery,
        request: GovernedAccessRequestV1,
    ) -> Result<GovernedProjectionResponseV1<ProjectionRelationVersion>, MemoryError> {
        let query_namespace = query.scope.namespace.clone();
        let rows = if query_namespace == request.scope.namespace {
            self.with_read_conn(move |conn| {
                projection_storage::query_relation_versions(conn, &query)
            })
            .await?
        } else {
            Vec::new()
        };
        let mut decisions = Vec::new();
        for row in &rows {
            decisions.push(origin_authority::evaluate_governed_access_v1(
                row.relation_version_id.as_str(),
                Some(&row.scope_key.namespace),
                None,
                None,
                &request,
            ));
        }
        if query_namespace != request.scope.namespace {
            decisions.push(origin_authority::evaluate_governed_access_v1(
                "projection:query",
                Some(&query_namespace),
                None,
                None,
                &request,
            ));
        }
        Ok(GovernedProjectionResponseV1 {
            items: Vec::new(),
            decisions,
        })
    }

    pub async fn query_episodes_governed(
        &self,
        query: ProjectionQuery,
        request: GovernedAccessRequestV1,
    ) -> Result<GovernedProjectionResponseV1<ProjectionEpisode>, MemoryError> {
        let query_namespace = query.scope.namespace.clone();
        let rows = if query_namespace == request.scope.namespace {
            self.with_read_conn(move |conn| projection_storage::query_episode_rows(conn, &query))
                .await?
        } else {
            Vec::new()
        };
        let mut decisions = Vec::new();
        for row in &rows {
            decisions.push(origin_authority::evaluate_governed_access_v1(
                row.episode_id.as_str(),
                Some(&row.scope_key.namespace),
                None,
                None,
                &request,
            ));
        }
        if query_namespace != request.scope.namespace {
            decisions.push(origin_authority::evaluate_governed_access_v1(
                "projection:query",
                Some(&query_namespace),
                None,
                None,
                &request,
            ));
        }
        Ok(GovernedProjectionResponseV1 {
            items: Vec::new(),
            decisions,
        })
    }

    pub async fn query_entity_aliases_governed(
        &self,
        query: ProjectionQuery,
        request: GovernedAccessRequestV1,
    ) -> Result<GovernedProjectionResponseV1<ProjectionEntityAlias>, MemoryError> {
        let query_namespace = query.scope.namespace.clone();
        let rows = if query_namespace == request.scope.namespace {
            self.with_read_conn(move |conn| projection_storage::query_entity_aliases(conn, &query))
                .await?
        } else {
            Vec::new()
        };
        let mut decisions = Vec::new();
        for row in &rows {
            decisions.push(origin_authority::evaluate_governed_access_v1(
                &format!(
                    "entity_alias:{}:{}",
                    row.canonical_entity_id.as_str(),
                    row.alias_text
                ),
                Some(&row.scope_key.namespace),
                None,
                None,
                &request,
            ));
        }
        if query_namespace != request.scope.namespace {
            decisions.push(origin_authority::evaluate_governed_access_v1(
                "projection:query",
                Some(&query_namespace),
                None,
                None,
                &request,
            ));
        }
        Ok(GovernedProjectionResponseV1 {
            items: Vec::new(),
            decisions,
        })
    }

    pub async fn query_evidence_refs_governed(
        &self,
        query: ProjectionQuery,
        request: GovernedAccessRequestV1,
    ) -> Result<GovernedProjectionResponseV1<ProjectionEvidenceRef>, MemoryError> {
        let query_namespace = query.scope.namespace.clone();
        let rows = if query_namespace == request.scope.namespace {
            self.with_read_conn(move |conn| projection_storage::query_evidence_refs(conn, &query))
                .await?
        } else {
            Vec::new()
        };
        let mut decisions = Vec::new();
        for row in &rows {
            decisions.push(origin_authority::evaluate_governed_access_v1(
                &format!(
                    "evidence_ref:{}:{}",
                    row.claim_id.as_str(),
                    row.fetch_handle
                ),
                Some(&row.scope_key.namespace),
                None,
                None,
                &request,
            ));
        }
        if query_namespace != request.scope.namespace {
            decisions.push(origin_authority::evaluate_governed_access_v1(
                "projection:query",
                Some(&query_namespace),
                None,
                None,
                &request,
            ));
        }
        Ok(GovernedProjectionResponseV1 {
            items: Vec::new(),
            decisions,
        })
    }

    /// Execute raw SQL. For testing only — not part of the stable public API.
    #[cfg(any(test, feature = "testing"))]
    pub async fn raw_execute(&self, sql: &str, params: Vec<String>) -> Result<usize, MemoryError> {
        let sql = sql.to_string();
        self.with_write_conn(move |conn| {
            let param_refs: Vec<&dyn rusqlite::types::ToSql> = params
                .iter()
                .map(|s| s as &dyn rusqlite::types::ToSql)
                .collect();
            Ok(conn.execute(&sql, &*param_refs)?)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedder::{EmbedBatchFuture, EmbedFuture};
    use crate::types::{SearchResult, SearchSource};

    struct PartiallyInvalidBatchEmbedder {
        dimensions: usize,
    }

    impl Embedder for PartiallyInvalidBatchEmbedder {
        fn embed<'a>(&'a self, _text: &'a str) -> EmbedFuture<'a> {
            let embedding = vec![1.0; self.dimensions];
            Box::pin(async move { Ok(embedding) })
        }

        fn embed_batch<'a>(&'a self, texts: Vec<String>) -> EmbedBatchFuture<'a> {
            let dimensions = self.dimensions;
            Box::pin(async move {
                let mut embeddings = vec![vec![1.0; dimensions]; texts.len()];
                if let Some(value) = embeddings.get_mut(1).and_then(|row| row.first_mut()) {
                    *value = f32::NAN;
                }
                Ok(embeddings)
            })
        }

        fn model_name(&self) -> &str {
            "partially-invalid-batch"
        }

        fn dimensions(&self) -> usize {
            self.dimensions
        }
    }

    fn make_result(content: &str) -> SearchResult {
        SearchResult {
            content: content.to_string(),
            source: SearchSource::Fact {
                fact_id: "test".to_string(),
                namespace: "test".to_string(),
            },
            score: 1.0,
            bm25_rank: Some(1),
            vector_rank: Some(1),
            cosine_similarity: Some(0.9),
        }
    }

    #[test]
    fn compress_search_results_shortens_long_content() {
        let long = "This is a very long sentence that definitely exceeds the one hundred fifty character limit. It goes on and on with lots of detail that should be truncated. More text here.";
        let results = vec![make_result(long)];
        let compressed = compress_search_results(results);
        assert!(
            compressed[0].content.len() <= 152, // 150 + ellipsis char
            "compressed content should be at most ~150 chars, got {}",
            compressed[0].content.len()
        );
        assert!(
            compressed[0].content.ends_with('…') || compressed[0].content.ends_with('.'),
            "compressed content should end with ellipsis or sentence punctuation"
        );
    }

    #[test]
    fn compress_search_results_preserves_short_content() {
        let short = "Short sentence.";
        let results = vec![make_result(short)];
        let compressed = compress_search_results(results);
        assert_eq!(compressed[0].content, "Short sentence.");
    }

    #[test]
    fn compress_search_results_preserves_first_sentence() {
        let content = "First sentence. Second sentence that is longer.";
        let results = vec![make_result(content)];
        let compressed = compress_search_results(results);
        assert_eq!(compressed[0].content, "First sentence.");
    }

    #[test]
    fn compress_search_results_empty_content() {
        let results = vec![make_result("")];
        let compressed = compress_search_results(results);
        assert_eq!(compressed[0].content, "");
    }

    #[test]
    fn compress_search_results_is_utf8_safe_at_character_limit() {
        let content = format!("{}。 trailing sentence", "記憶🧠e\u{301}".repeat(80));
        let compressed = compress_search_results(vec![make_result(&content)]);
        assert!(compressed[0].content.chars().count() <= 151);
        assert!(compressed[0].content.ends_with('…'));
    }

    #[tokio::test]
    async fn invalid_batch_leaves_embedding_cache_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let config = MemoryConfig {
            base_dir: dir.path().to_path_buf(),
            ..MemoryConfig::default()
        };
        let store = MemoryStore::open_with_embedder(
            config,
            Box::new(PartiallyInvalidBatchEmbedder { dimensions: 768 }),
        )
        .unwrap();
        store.embed_document("already cached").await.unwrap();
        store.embed_document("more recently cached").await.unwrap();
        let before: Vec<_> = store
            .inner
            .embedding_cache
            .lock()
            .unwrap()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();

        let error = store
            .embed_documents_batch(&["already cached", "new valid", "new invalid"])
            .await
            .unwrap_err();
        assert_eq!(error.kind(), "non_finite_embedding_value");

        let after: Vec<_> = store
            .inner
            .embedding_cache
            .lock()
            .unwrap()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        assert_eq!(after, before, "failed batches must not mutate cache state");
    }

    #[tokio::test]
    async fn stats_reports_selected_vector_backend() {
        let dir = tempfile::tempdir().unwrap();
        let config = MemoryConfig {
            base_dir: dir.path().to_path_buf(),
            ..MemoryConfig::default()
        };
        let store = MemoryStore::open_with_embedder(
            config,
            Box::new(PartiallyInvalidBatchEmbedder { dimensions: 768 }),
        )
        .unwrap();
        let stats = store.stats().await.unwrap();
        assert_eq!(stats.vector_backend, "usearch");
        assert!(stats.vector_backend_generation.starts_with("usearch:"));
    }

    #[test]
    fn search_cache_identity_changes_with_corpus_epoch() {
        let before = search_cache_key("same query", 5, "usearch:g1", RetrievalEpoch(7));
        let after_write = search_cache_key("same query", 5, "usearch:g1", RetrievalEpoch(8));
        let after_backend_rebuild =
            search_cache_key("same query", 5, "usearch:g2", RetrievalEpoch(8));
        assert_ne!(before, after_write);
        assert_ne!(after_write, after_backend_rebuild);
    }

    #[tokio::test]
    async fn fact_write_changes_the_live_search_cache_identity() {
        let dir = tempfile::tempdir().unwrap();
        let config = MemoryConfig {
            base_dir: dir.path().to_path_buf(),
            search: SearchConfig {
                min_similarity: -1.0,
                ..SearchConfig::default()
            },
            ..MemoryConfig::default()
        };
        let store = MemoryStore::open_with_embedder(
            config,
            Box::new(PartiallyInvalidBatchEmbedder { dimensions: 768 }),
        )
        .unwrap();
        store
            .add_fact("cache", "alpha before write", None, None)
            .await
            .unwrap();
        store.search("alpha", Some(5), None, None).await.unwrap();
        let before = store
            .inner
            .search_cache
            .lock()
            .unwrap()
            .iter()
            .next()
            .unwrap()
            .0
            .clone();

        store
            .add_fact("cache", "beta after write", None, None)
            .await
            .unwrap();
        store.search("alpha", Some(5), None, None).await.unwrap();
        let after = store
            .inner
            .search_cache
            .lock()
            .unwrap()
            .iter()
            .next()
            .unwrap()
            .0
            .clone();
        assert_ne!(
            before, after,
            "a corpus write must force a revision-bound miss"
        );
    }

    /// MEM-012: UTF-8 text compaction must not panic on multi-byte characters.
    #[tokio::test]
    async fn utf8_truncation_does_not_panic_on_multibyte() {
        let config = MemoryConfig {
            embedding: crate::config::EmbeddingConfig {
                dimensions: 768,
                ..Default::default()
            },
            ..Default::default()
        };
        let store = MemoryStore::open_with_embedder(
            config,
            Box::new(crate::embedder::MockEmbedder::new(768)),
        )
        .unwrap();

        // CJK, emoji, combining marks — must not panic
        let texts = [
            "这是一段中文文本，需要被正确处理而不是 panic".to_string(),
            "🎉🎊🎈 Party time with emoji 🎂🎉".to_string(),
            "e\u{0301} = é (combining mark test)".to_string(),
            "Многобайтовые символы UTF-8".to_string(),
        ];

        for text in &texts {
            // This exercises the compaction/truncation paths
            let _ = store.add_fact("utf8-test", text, None, None).await;
            let results = store.search(text, Some(1), None, None).await;
            assert!(results.is_ok(), "search must not panic on UTF-8: {}", text);
        }
    }
}
