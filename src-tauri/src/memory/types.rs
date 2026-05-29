use crate::retrieval::source_scope::SourceScope;
use serde::{Deserialize, Serialize};

pub const MEMORY_BACKEND_GLOSS_LOCAL: &str = "gloss-local";
pub const MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW: &str = "semantic-memory-preview";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryBackendStatus {
    pub backend_id: String,
    pub default_backend: String,
    pub active_backend: String,
    pub backend_used: String,
    pub available: bool,
    pub semantic_memory_feature_enabled: bool,
    pub semantic_memory_available: bool,
    pub semantic_memory_path: Option<String>,
    pub index_sync_status: String,
    pub sync_status: String,
    pub last_sync_at: Option<String>,
    pub last_sync_error: Option<String>,
    pub last_retrieval_receipt_id: Option<String>,
    pub last_receipt_ref: Option<String>,
    pub fallback_reason: Option<String>,
    pub degradation_markers: Vec<String>,
    pub backend_version_or_digest: Option<String>,
    pub degraded: bool,
    pub diagnostic: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalCapabilityDecisionV1 {
    pub requested_backend: String,
    pub effective_backend: String,
    pub decision_reason: Option<String>,
    pub build_feature_available: bool,
    pub runtime_enabled: bool,
    pub projection_ready: bool,
    pub dense_ready: bool,
    pub fallback_allowed: bool,
    pub degraded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryScopeEcho {
    pub mode: String,
    pub requested_source_ids: Vec<String>,
    pub effective_source_ids: Vec<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexSourceRequest {
    pub notebook_id: String,
    pub source_id: String,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexSourceReceipt {
    pub backend_id: String,
    pub notebook_id: String,
    pub source_id: String,
    pub receipt_id: String,
    pub indexed_chunks: usize,
    pub sync_status: String,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vector_artifact_receipt: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySearchRequest {
    pub notebook_id: String,
    pub source_scope: SourceScope,
    pub query: String,
    pub limit: usize,
    pub trace_id: Option<String>,
    pub allow_fallback: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySearchCandidate {
    pub chunk_id: String,
    pub source_id: String,
    pub notebook_id: Option<String>,
    pub source_title: Option<String>,
    pub citation_anchor: Option<String>,
    pub content: String,
    pub score: f64,
    pub backend: String,
    pub receipt_ref: Option<String>,
    pub degradation: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySearchResponse {
    pub backend_id: String,
    pub backend_requested: String,
    pub backend_used: String,
    pub source_scope_mode: String,
    pub selected_source_ids: Vec<String>,
    pub invalid_source_ids: Vec<String>,
    pub excluded_source_count: usize,
    pub scope: MemoryScopeEcho,
    pub candidates: Vec<MemorySearchCandidate>,
    pub receipt_id: String,
    pub provenance: serde_json::Value,
    pub fallback_reason: Option<String>,
    pub degradation_markers: Vec<String>,
    pub source_scope_preserved: bool,
    pub fallback_used: bool,
    pub degraded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalMode {
    Bm25Only,
    DenseOnly,
    HybridRrf,
    SemanticMemory,
    SourceOrderFallback,
    RawContentFallback,
    Unavailable,
}

impl RetrievalMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Bm25Only => "bm25_only",
            Self::DenseOnly => "dense_only",
            Self::HybridRrf => "hybrid_rrf",
            Self::SemanticMemory => "semantic_memory",
            Self::SourceOrderFallback => "source_order_fallback",
            Self::RawContentFallback => "raw_content_fallback",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalReasonCode {
    NativeIndexingDisabled,
    DenseEngineUnavailable,
    EmbedderUnavailable,
    IndexMissing,
    NoEmbeddedChunks,
    PartialEmbeddingCoverage,
    ScopeHasMissingEmbeddings,
    SemanticMemoryFeatureDisabled,
    SemanticMemoryBuildFeatureMissing,
    SemanticMemoryLinksMissing,
    SemanticMemoryLinksDegraded,
    SemanticMemoryTimeout,
    Bm25QuerySanitizedEmpty,
    Bm25NoMatches,
    SourceOrderFallback,
    RawContentFallback,
    DenseNoQueryMatches,
    NoRetrievalContext,
}

impl RetrievalReasonCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NativeIndexingDisabled => "native_indexing_disabled",
            Self::DenseEngineUnavailable => "dense_engine_unavailable",
            Self::EmbedderUnavailable => "embedder_unavailable",
            Self::IndexMissing => "index_missing",
            Self::NoEmbeddedChunks => "no_embedded_chunks",
            Self::PartialEmbeddingCoverage => "partial_embedding_coverage",
            Self::ScopeHasMissingEmbeddings => "scope_has_missing_embeddings",
            Self::SemanticMemoryFeatureDisabled => "semantic_memory_feature_disabled",
            Self::SemanticMemoryBuildFeatureMissing => "semantic_memory_build_feature_missing",
            Self::SemanticMemoryLinksMissing => "semantic_memory_links_missing",
            Self::SemanticMemoryLinksDegraded => "semantic_memory_links_degraded",
            Self::SemanticMemoryTimeout => "semantic_memory_timeout",
            Self::Bm25QuerySanitizedEmpty => "bm25_query_sanitized_empty",
            Self::Bm25NoMatches => "bm25_no_matches",
            Self::SourceOrderFallback => "source_order_fallback",
            Self::RawContentFallback => "raw_content_fallback",
            Self::DenseNoQueryMatches => "dense_no_query_matches",
            Self::NoRetrievalContext => "no_retrieval_context",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalEngineStatus {
    pub engine: String,
    pub attempted: bool,
    pub available: bool,
    pub contributed: bool,
    pub candidate_count: usize,
    pub elapsed_ms: u128,
    pub reason_code: Option<RetrievalReasonCode>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RetrievalCoverage {
    pub selected_sources: usize,
    pub total_chunks: usize,
    pub fts_indexed_chunks: usize,
    pub embedded_chunks: usize,
    pub missing_embeddings: usize,
    pub semantic_links_total: usize,
    pub semantic_links_healthy: usize,
    pub semantic_links_degraded: usize,
    pub dense_coverage_ratio: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalResult {
    pub chunk_id: Option<String>,
    pub source_id: String,
    pub title: Option<String>,
    pub content: String,
    pub score: f64,
    pub engine: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalOutcome {
    pub mode: RetrievalMode,
    pub results: Vec<RetrievalResult>,
    pub engines: Vec<RetrievalEngineStatus>,
    pub coverage: RetrievalCoverage,
    pub degraded: bool,
    pub fallback_chain: Vec<RetrievalReasonCode>,
    pub user_visible_summary: String,
    pub trace_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticMemoryLinkStatus {
    pub notebook_id: String,
    pub total_links: usize,
    pub synced_links: usize,
    pub stale_links: usize,
    pub failed_links: usize,
    pub missing_document_links: usize,
    #[serde(default)]
    pub degraded_links: usize,
    #[serde(default)]
    pub reason_codes: Vec<String>,
    pub last_sync_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryBackendComparison {
    pub local_backend_result: MemorySearchResponse,
    pub semantic_memory_result: Option<MemorySearchResponse>,
    pub intersection_by_chunk_id: Vec<String>,
    pub source_scope_violations: Vec<String>,
    pub unmapped_semantic_candidates: Vec<String>,
    pub local_latency_ms: u128,
    pub semantic_memory_latency_ms: Option<u128>,
    pub receipt_ids: Vec<String>,
    pub decision: String,
}

#[derive(Debug, Clone)]
pub struct SemanticCandidateEnvelope {
    pub sm_document_id: String,
    pub sm_chunk_id: Option<String>,
    pub content: String,
    pub score: f64,
}

#[derive(Debug, Clone)]
pub struct SemanticLinkRow {
    pub chunk_id: String,
    pub source_id: String,
    pub sm_document_id: Option<String>,
    pub sm_chunk_id: Option<String>,
    pub content_digest: String,
    pub sync_status: String,
}
