#![allow(deprecated)]

use crate::error::MemoryError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use stack_ids::{
    ClaimId, ClaimVersionId, EntityId, EnvelopeId, EpisodeId, RelationVersionId, ScopeKey,
};

/// Stable trace identifier used for cross-crate correlation and auditability.
///
/// ## Phase status: compatibility / migration-only
///
/// This is a crate-local `TraceId` retained for backward compatibility.
/// The canonical replacement is `stack_ids::TraceCtx`. Use
/// `TraceCtx::from_legacy_trace_id()` to convert.
///
/// **Removal condition**: removed when all internal usage migrates to `TraceCtx`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CompatTraceId(pub String);

#[deprecated(since = "0.5.0", note = "Use stack_ids::TraceCtx instead")]
pub type TraceId = CompatTraceId;

impl CompatTraceId {
    /// Create a trace ID from any owned string-like input.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the trace ID as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CompatTraceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for CompatTraceId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for CompatTraceId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

/// Role of a message in a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// System prompt / instructions.
    System,
    /// User message.
    User,
    /// Assistant (LLM) response.
    Assistant,
    /// Tool call result.
    Tool,
}

impl Role {
    /// Convert to the string stored in SQLite.
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }

    /// Parse from the string stored in SQLite.
    pub fn from_str_value(s: &str) -> Option<Self> {
        match s {
            "system" => Some(Role::System),
            "user" => Some(Role::User),
            "assistant" => Some(Role::Assistant),
            "tool" => Some(Role::Tool),
            _ => None,
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Role {
    type Err = MemoryError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_str_value(s).ok_or_else(|| MemoryError::Other(format!("Unknown role: '{}'", s)))
    }
}

/// Indicates whether a search result came from a fact, document chunk, message, or episode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchSourceType {
    /// Result is from the facts table.
    Facts,
    /// Result is from the chunks table.
    Chunks,
    /// Result is from the messages table.
    Messages,
    /// Result is from the episodes table.
    Episodes,
}

/// Controls whether privacy-sensitive search inputs are retained for replay.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayMode {
    /// Keep only receipt digests; callers must supply inputs to replay.
    #[default]
    NoReplay,
    /// Persist query text and filters alongside the durable receipt.
    StoreInputs,
}

/// Controls whether search receipt metadata is produced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptMode {
    /// Do not produce receipt metadata.
    #[default]
    Disabled,
    /// Produce receipt-ready metadata for explain/audit paths.
    ExplainOnly,
    /// Return receipt metadata to the caller.
    ReturnReceipt,
}

/// Controls whether search should prefer exact reference scoring or allow approximate backends.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExactnessProfile {
    /// Use the configured default backend policy.
    #[default]
    Default,
    /// Prefer exact brute-force f32 vector scoring over approximate sidecars.
    PreferExact,
    /// Permit approximate candidate generation, with exact rerank when configured.
    AllowApproximate,
}

/// Explicit search execution context for deterministic replay and receipt generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchContext {
    /// Timestamp used for time-sensitive scoring such as recency.
    pub evaluation_time: DateTime<Utc>,
    /// Receipt metadata mode.
    pub receipt_mode: ReceiptMode,
    /// Opt-in policy for storing privacy-sensitive inputs for complete replay.
    #[serde(default)]
    pub replay_mode: ReplayMode,
    /// Exactness policy for vector candidate generation.
    pub exactness_profile: ExactnessProfile,
    /// Optional caller-provided request/receipt correlation ID.
    pub request_id: Option<String>,
    /// Optional distributed trace identifier supplied by the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// Optional family ID tying retries/attempts for the same logical request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_family_id: Option<String>,
    /// Optional retry/attempt identifier supplied by the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    /// Receipt ID this search is replaying, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_of: Option<String>,
    /// Digest of raw query text when the caller provides one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_text_digest: Option<String>,
    /// Digest of raw or structured query input when supplied by the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_input_digest: Option<String>,
    /// Digest of structured filters when the caller provides one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_digest: Option<String>,
    /// Redaction state label for explain/replay surfaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redaction_state: Option<String>,
    /// Optional budget identity associated with the search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_id: Option<String>,
    /// Optional caller deadline associated with the search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_at: Option<DateTime<Utc>>,
}

impl SearchContext {
    /// Build a context using the current wall clock at the API boundary.
    pub fn default_now() -> Self {
        Self {
            evaluation_time: Utc::now(),
            receipt_mode: ReceiptMode::Disabled,
            replay_mode: ReplayMode::NoReplay,
            exactness_profile: ExactnessProfile::Default,
            request_id: None,
            trace_id: None,
            attempt_family_id: None,
            attempt_id: None,
            replay_of: None,
            query_text_digest: None,
            query_input_digest: None,
            filter_digest: None,
            redaction_state: None,
            budget_id: None,
            deadline_at: None,
        }
    }

    /// Build a replay context with an explicit evaluation timestamp.
    pub fn at(evaluation_time: DateTime<Utc>) -> Self {
        Self {
            evaluation_time,
            ..Self::default_now()
        }
    }

    /// Whether a receipt should be produced for this context.
    pub fn receipts_enabled(&self) -> bool {
        self.receipt_mode != ReceiptMode::Disabled
    }
}

impl Default for SearchContext {
    fn default() -> Self {
        Self::default_now()
    }
}

/// Receipt-ready vector/search execution metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorSearchReceiptV1 {
    /// Receipt schema version.
    #[serde(default = "default_vector_search_receipt_schema")]
    pub schema_version: String,
    /// Digest of the canonical stored receipt payload, when persisted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_digest: Option<String>,
    /// Receipt or request correlation ID.
    pub receipt_id: String,
    /// Timestamp used for deterministic scoring.
    pub evaluation_time: DateTime<Utc>,
    /// Optional distributed trace identifier supplied by the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// Optional family ID tying retries/attempts for the same logical request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_family_id: Option<String>,
    /// Optional retry/attempt identifier supplied by the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    /// Receipt ID this receipt replays, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_of: Option<String>,
    /// Stable BLAKE3 digest of the query embedding bytes, when available.
    pub query_embedding_digest: Option<String>,
    /// Digest of raw query text when supplied by the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_text_digest: Option<String>,
    /// Digest of raw or structured query input when supplied by the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_input_digest: Option<String>,
    /// Digest of structured filters when supplied by the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_digest: Option<String>,
    /// Redaction state label for explain/replay surfaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redaction_state: Option<String>,
    /// Optional budget identity associated with the search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_id: Option<String>,
    /// Optional caller deadline associated with the search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_at: Option<DateTime<Utc>>,
    /// Human-readable search profile.
    pub search_profile: String,
    /// Candidate backend used for vector retrieval.
    pub candidate_backend: String,
    /// Codec family used for derived vector artifacts, when applicable.
    pub codec_family: Option<String>,
    /// Codec profile digest used for derived vector artifacts, when applicable.
    pub codec_profile_digest: Option<String>,
    /// Alias for derived artifact profile digest used by v11-compatible hooks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_profile_digest: Option<String>,
    /// Number of derived artifacts considered by the vector path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_count: Option<usize>,
    /// Number of corrupt derived artifacts encountered by the vector path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_corruption_count: Option<usize>,
    /// Number of missing derived artifacts encountered by the vector path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_missing_count: Option<usize>,
    /// Manifest digest for the derived vector artifacts considered by the search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vector_artifact_manifest_digest: Option<String>,
    /// Active generation ID for derived vector artifacts, when used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_generation_id: Option<String>,
    /// Number of derived artifacts scanned by approximate candidate generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approximate_scanned_count: Option<usize>,
    /// Number of approximate candidates returned for exact f32 reranking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approximate_returned_count: Option<usize>,
    /// Number of authoritative raw f32 rows loaded during exact rerank.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_rows_loaded_count: Option<usize>,
    /// Filter strategy used by approximate candidate generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_strategy: Option<String>,
    /// Number of derived vector artifacts considered by the vector path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vector_artifact_count: Option<usize>,
    /// Number of missing derived vector artifacts encountered by the vector path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vector_artifact_missing_count: Option<usize>,
    /// Number of stale derived vector artifacts encountered by the vector path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vector_artifact_stale_count: Option<usize>,
    /// Number of candidates exact-reranked against authoritative f32 embeddings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact_rerank_count: Option<usize>,
    /// Number of approximate candidates produced by the candidate backend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approximate_candidate_count: Option<usize>,
    /// Explicit fallback reason, mirrored from fallback for evidence readers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    /// Structured receipt for derived candidate backends such as proveKV pool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_candidate: Option<DerivedCandidateReceiptV1>,
    /// Whether approximate codec/index scoring contributed to candidate generation.
    pub approximate: bool,
    /// Number of vector candidates requested from the backend.
    pub requested_candidates: usize,
    /// Number of candidates returned by the backend before SQL post-filtering.
    pub returned_candidates: usize,
    /// Number of vector candidates remaining after SQL filters and exact rerank.
    pub post_filter_candidates: usize,
    /// Whether a genuine sparse retrieval lane participated in fusion.
    #[serde(default)]
    pub sparse_enabled: bool,
    /// Configured sparse RRF weight when the lane was enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sparse_weight: Option<f64>,
    /// Number of sparse query entries used for dot-product retrieval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sparse_query_nonzero_count: Option<usize>,
    /// Number of filtered sparse candidates admitted to fusion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sparse_candidate_count: Option<usize>,
    /// Sparse representation labels observed among admitted candidates.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sparse_representations: Vec<String>,
    /// Per-result sparse ranks, allowing the third signal to be audited.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sparse_result_ranks: Vec<SparseRankReceiptV1>,
    /// Fallback path, if approximate retrieval degraded or was bypassed.
    pub fallback: Option<String>,
    /// Whether exact f32 rerank/reference scoring was used.
    pub exact_rerank: bool,
    /// Result IDs returned to the caller.
    pub result_ids: Vec<String>,
    /// Degradation notes visible to explain/audit paths.
    pub degradations: Vec<String>,
}

/// Inspectable sparse rank recorded in a durable search receipt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SparseRankReceiptV1 {
    /// Canonical result ID.
    pub result_id: String,
    /// One-based sparse rank.
    pub rank: usize,
}

/// Stable generation-level manifest for derived vector acceleration artifacts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DerivedVectorArtifactGenerationV1 {
    /// Stable schema marker.
    pub schema_version: String,
    /// Generation UUID.
    pub generation_id: String,
    /// Derived codec family.
    pub codec_family: String,
    /// Digest of the codec profile.
    pub codec_profile_digest: String,
    /// Digest over authoritative source rows used to build the generation.
    pub source_snapshot_digest: String,
    /// Number of authoritative source rows scanned.
    pub source_row_count: usize,
    /// Number of artifacts produced.
    pub artifact_count: usize,
    /// Authoritative source tables included in the build.
    pub source_tables: Vec<String>,
    /// Embedding dimension.
    pub dim: usize,
    /// Artifact wire encoding.
    pub encoding: String,
    /// Build timestamp.
    pub created_at: DateTime<Utc>,
    /// Optional build receipt ID.
    pub build_receipt_id: Option<String>,
    /// Digest of the artifact manifest for this generation.
    pub artifact_manifest_digest: String,
    /// Generation state.
    pub status: String,
    /// Structured or human-readable degradation markers.
    pub degradations: Vec<String>,
}

/// Stable generation-level manifest for derived proveKV/poly-kv pool candidate artifacts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProveKvPoolGenerationV1 {
    pub schema_version: String,
    pub generation_id: String,
    pub embedding_snapshot_digest: String,
    pub source_digest: String,
    pub pool_manifest_digest: String,
    pub codec_family: String,
    pub codec_profile: String,
    pub vector_dim: usize,
    pub item_count: usize,
    pub payload_bytes: u64,
    pub created_at: DateTime<Utc>,
}

/// Durable item-to-pool-index mapping for a proveKV/poly-kv pool generation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProveKvPoolItemMapEntryV1 {
    pub generation_id: String,
    pub item_id: String,
    pub source_type: String,
    pub pool_index: usize,
    pub embedding_digest: String,
}

/// Lifecycle status for a proveKV/poly-kv pool generation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProveKvPoolGenerationStatus {
    Disabled,
    Missing,
    Building,
    Ready,
    Stale,
    Failed,
}

/// Current proveKV/poly-kv pool artifact status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProveKvPoolArtifactStatusV1 {
    pub status: ProveKvPoolGenerationStatus,
    pub generation_id: Option<String>,
    pub embedding_snapshot_digest: Option<String>,
    pub pool_manifest_digest: Option<String>,
    pub item_count: usize,
    pub payload_bytes: u64,
    pub reason: Option<String>,
}

/// Receipt-like summary for rebuilding proveKV/poly-kv pool artifacts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProveKvPoolArtifactBuildReceiptV1 {
    pub schema_version: String,
    pub generation_id: String,
    pub embedding_snapshot_digest: String,
    pub source_digest: String,
    pub pool_manifest_digest: String,
    pub codec_family: String,
    pub codec_profile: String,
    pub vector_dim: usize,
    pub item_count: usize,
    pub payload_bytes: u64,
    pub exact_rerank_required: bool,
    pub created_at: DateTime<Utc>,
}

/// Structured search receipt for derived candidate generation followed by exact f32 rerank.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DerivedCandidateReceiptV1 {
    pub candidate_backend: String,
    pub codec_family: Option<String>,
    pub generation_id: Option<String>,
    pub embedding_snapshot_digest: Option<String>,
    pub pool_manifest_digest: Option<String>,
    pub exact_rerank: bool,
    pub approximate: bool,
    pub fallback: Option<String>,
    pub raw_candidate_count: usize,
    pub post_filter_count: usize,
    pub final_result_count: usize,
}

/// Receipt-like summary for rebuilding derived vector acceleration artifacts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorArtifactBuildReceiptV1 {
    /// Stable schema marker.
    pub schema_version: String,
    /// Derived codec family.
    pub codec_family: String,
    /// Digest of the codec profile used for all artifacts in the build.
    pub codec_profile_digest: String,
    /// Number of authoritative embedding rows scanned.
    pub source_row_count: usize,
    /// Number of artifacts written.
    pub artifact_count: usize,
    /// Active generation ID produced by the rebuild.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_id: Option<String>,
    /// Source snapshot digest used by the generation manifest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_snapshot_digest: Option<String>,
    /// Artifact manifest digest for this generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_manifest_digest: Option<String>,
    /// ID of the build receipt itself (same value stored in the generation manifest).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_receipt_id: Option<String>,
    /// Number of rows skipped because authoritative embeddings were invalid.
    pub skipped_row_count: usize,
    /// Wall-clock build duration in milliseconds.
    pub elapsed_ms: u128,
    /// Build timestamp.
    pub created_at: DateTime<Utc>,
    /// Non-fatal build notes.
    pub degradations: Vec<String>,
}

fn default_vector_search_receipt_schema() -> String {
    "vector_search_receipt_v1".to_string()
}

/// Product-facing answers derived from a search receipt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchReceiptAnswersV1 {
    /// Receipt or request correlation ID.
    pub receipt_id: String,
    /// Stable ID to attach to replay/audit logs.
    pub replay_receipt_id: String,
    /// Timestamp used for deterministic scoring.
    pub evaluation_time: DateTime<Utc>,
    /// Human-readable search profile.
    pub search_profile: String,
    /// Candidate backend used for retrieval.
    pub candidate_backend: String,
    /// Codec family used for derived vector artifacts, when applicable.
    pub codec_family: Option<String>,
    /// Codec profile digest used for derived vector artifacts, when applicable.
    pub codec_profile_digest: Option<String>,
    /// Exactness label suitable for UI/API surfaces.
    pub exactness: String,
    /// Whether approximate codec/index scoring contributed to candidate generation.
    pub approximate: bool,
    /// Whether exact f32 rerank/reference scoring was used.
    pub exact_rerank: bool,
    /// Fallback path, if approximate retrieval degraded or was bypassed.
    pub fallback: Option<String>,
    /// Whether degradations or fallback occurred.
    pub degraded: bool,
    /// Whether the receipt carries enough deterministic context for replay with the original query.
    pub replay_ready: bool,
    /// Whether derived vector/index artifacts can be rebuilt from authoritative rows and profiles.
    pub rebuild_ready: bool,
    /// Result IDs returned to the caller.
    pub result_ids: Vec<String>,
    /// Number of returned results.
    pub result_count: usize,
    /// Degradation notes visible to explain/audit paths.
    pub degradations: Vec<String>,
    /// Plain-language reasons results appeared.
    pub why_results_appeared: Vec<String>,
}

impl VectorSearchReceiptV1 {
    /// Convert low-level receipt metadata into answers for explain/replay UX.
    pub fn answers(&self) -> SearchReceiptAnswersV1 {
        let exactness = match (self.approximate, self.exact_rerank) {
            (true, true) => "approximate_candidate_generation_with_exact_rerank",
            (true, false) => "approximate",
            (false, true) => "exact_reference_with_rerank",
            (false, false) => "exact_reference",
        }
        .to_string();

        let mut why_results_appeared = Vec::new();
        why_results_appeared.push(format!(
            "retrieval used candidate backend '{}'",
            self.candidate_backend
        ));
        if self.exact_rerank {
            why_results_appeared.push("final vector ordering used exact f32 scoring".to_string());
        }
        if self.sparse_enabled {
            why_results_appeared.push(format!(
                "sparse dot-product retrieval admitted {} candidates",
                self.sparse_candidate_count.unwrap_or(0)
            ));
        }
        if let Some(fallback) = &self.fallback {
            why_results_appeared.push(format!("fallback path '{}' was used", fallback));
        }
        if let Some(codec_profile_digest) = &self.codec_profile_digest {
            why_results_appeared.push(format!(
                "derived vector artifacts used codec profile '{}'",
                codec_profile_digest
            ));
        } else {
            why_results_appeared.push("no derived codec profile was used".to_string());
        }
        if let Some(query_embedding_digest) = &self.query_embedding_digest {
            why_results_appeared.push(format!(
                "query embedding digest '{}' is recorded for replay checks",
                query_embedding_digest
            ));
        }

        SearchReceiptAnswersV1 {
            receipt_id: self.receipt_id.clone(),
            replay_receipt_id: self.receipt_id.clone(),
            evaluation_time: self.evaluation_time,
            search_profile: self.search_profile.clone(),
            candidate_backend: self.candidate_backend.clone(),
            codec_family: self.codec_family.clone(),
            codec_profile_digest: self.codec_profile_digest.clone(),
            exactness,
            approximate: self.approximate,
            exact_rerank: self.exact_rerank,
            fallback: self.fallback.clone(),
            degraded: self.fallback.is_some() || !self.degradations.is_empty(),
            replay_ready: self.query_embedding_digest.is_some(),
            rebuild_ready: self.query_embedding_digest.is_some()
                && self.exact_rerank
                && self.fallback.is_none()
                && (self
                    .vector_artifact_count
                    .or(self.artifact_count)
                    .is_some_and(|count| count > 0)
                    || (self.codec_family.is_none()
                        && self.candidate_backend.contains("brute_force_f32")
                        && !self.result_ids.is_empty())),
            result_ids: self.result_ids.clone(),
            result_count: self.result_ids.len(),
            degradations: self.degradations.clone(),
            why_results_appeared,
        }
    }
}

/// Search response shape for context-aware APIs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    /// Search results.
    pub results: Vec<SearchResult>,
    /// Optional receipt metadata.
    pub receipt: Option<VectorSearchReceiptV1>,
}

/// Caller-supplied chunk for manifest ingestion.
///
/// The external chunk ID is returned in the ingest mapping, but semantic-memory still
/// owns the durable chunk primary key and generates its own `sm_chunk_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkManifestEntry {
    /// Caller-owned chunk identifier.
    pub external_chunk_id: String,
    /// Already chunked content to embed and store.
    pub content: String,
    /// Optional caller-estimated token count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_count_estimate: Option<usize>,
    /// Optional caller-computed content digest for verification by adapters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_digest: Option<String>,
    /// Optional per-chunk metadata kept in the receipt mapping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Document-level options for chunk manifest ingestion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkManifestIngestOptions {
    /// Document title.
    pub title: String,
    /// Namespace/notebook scope.
    pub namespace: String,
    /// Optional file path, URL, or caller source identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    /// Optional document metadata stored with the semantic-memory document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Exact mapping returned for a single manifest chunk after a successful transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkManifestChunkMapping {
    /// Caller-owned chunk identifier supplied in the manifest.
    pub external_chunk_id: String,
    /// semantic-memory document id that owns the chunk.
    pub sm_document_id: String,
    /// semantic-memory chunk id generated and stored in `chunks.id`.
    pub sm_chunk_id: String,
    /// Position in the supplied manifest.
    pub chunk_index: usize,
    /// Stored chunk content digest, when supplied by caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_digest: Option<String>,
    /// Optional caller metadata echoed for adapter receipt/audit use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Successful chunk-manifest ingest receipt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkManifestIngestResult {
    /// semantic-memory document id generated for this manifest.
    pub sm_document_id: String,
    /// Namespace/notebook scope used for ingest.
    pub namespace: String,
    /// Receipt/request correlation id for adapters.
    pub receipt_id: String,
    /// Ordered external chunk to semantic-memory chunk mappings.
    pub chunks: Vec<ChunkManifestChunkMapping>,
}

/// Explained search response shape for context-aware APIs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplainedSearchResponse {
    /// Search results with scoring breakdowns.
    pub results: Vec<ExplainedResult>,
    /// Optional receipt metadata.
    pub receipt: Option<VectorSearchReceiptV1>,
}

/// Replay comparison for a durable search receipt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchReplayReportV1 {
    /// Durable receipt ID that was replayed.
    pub receipt_id: String,
    /// Newly generated receipt ID for the replay attempt.
    pub replay_receipt_id: String,
    /// Original durable receipt metadata.
    pub original_receipt: VectorSearchReceiptV1,
    /// Receipt produced by the replay attempt.
    pub replay_receipt: VectorSearchReceiptV1,
    /// Whether the caller-supplied query produced the same embedding digest.
    pub query_embedding_digest_matches: bool,
    /// Whether replay returned the same result IDs in the same order.
    pub result_ids_match: bool,
    /// Original result IDs missing from replay output.
    pub missing_result_ids: Vec<String>,
    /// Replay result IDs not present in the original receipt.
    pub added_result_ids: Vec<String>,
    /// Whether replay used the vector-only API family.
    pub vector_only: bool,
}

/// Common filter surface for imported projection queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectionQuery {
    /// Full scope to enforce.
    pub scope: ScopeKey,
    /// Optional free-text query applied to the projection's searchable fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_query: Option<String>,
    /// Valid-time as-of filter for versioned projection rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_at: Option<String>,
    /// Transaction-time cutoff for imported rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_at_or_before: Option<String>,
    /// Optional subject-entity filter for claim/relation queries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_entity_id: Option<EntityId>,
    /// Optional canonical-entity filter for alias queries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_entity_id: Option<EntityId>,
    /// Optional claim-state filter for claim-version queries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_state: Option<String>,
    /// Optional claim filter for claim/evidence queries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_id: Option<ClaimId>,
    /// Optional claim-version filter for evidence queries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_version_id: Option<ClaimVersionId>,
    /// Final result limit.
    pub limit: usize,
}

impl ProjectionQuery {
    pub fn new(scope: ScopeKey) -> Self {
        Self {
            scope,
            text_query: None,
            valid_at: None,
            recorded_at_or_before: None,
            subject_entity_id: None,
            canonical_entity_id: None,
            claim_state: None,
            claim_id: None,
            claim_version_id: None,
            limit: 10,
        }
    }
}

/// Public read shape for imported claim projection rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectionClaimVersion {
    pub claim_version_id: ClaimVersionId,
    pub claim_id: ClaimId,
    pub claim_state: String,
    pub projection_family: String,
    pub subject_entity_id: EntityId,
    pub predicate: String,
    pub object_anchor: serde_json::Value,
    pub scope_key: ScopeKey,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub recorded_at: String,
    pub preferred_open: bool,
    pub source_envelope_id: EnvelopeId,
    pub source_authority: String,
    pub trace_id: Option<String>,
    pub freshness: String,
    pub contradiction_status: String,
    pub supersedes_claim_version_id: Option<ClaimVersionId>,
    pub content: String,
    pub confidence: f32,
    pub metadata: Option<serde_json::Value>,
    pub source_exported_at: Option<String>,
    pub transformed_at: Option<String>,
}

/// Public read shape for imported relation projection rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectionRelationVersion {
    pub relation_version_id: RelationVersionId,
    pub subject_entity_id: EntityId,
    pub predicate: String,
    pub object_anchor: serde_json::Value,
    pub scope_key: ScopeKey,
    pub claim_id: Option<ClaimId>,
    pub source_episode_id: Option<EpisodeId>,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub recorded_at: String,
    pub preferred_open: bool,
    pub supersedes_relation_version_id: Option<RelationVersionId>,
    pub contradiction_status: String,
    pub source_confidence: f32,
    pub projection_family: String,
    pub source_envelope_id: EnvelopeId,
    pub source_authority: String,
    pub trace_id: Option<String>,
    pub freshness: String,
    pub metadata: Option<serde_json::Value>,
    pub source_exported_at: Option<String>,
    pub transformed_at: Option<String>,
}

/// Public read shape for imported episode projection rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectionEpisode {
    pub episode_id: EpisodeId,
    pub document_id: String,
    pub cause_ids: Vec<String>,
    pub effect_type: String,
    pub outcome: String,
    pub confidence: f32,
    pub experiment_id: Option<String>,
    pub scope_key: ScopeKey,
    pub source_envelope_id: EnvelopeId,
    pub source_authority: String,
    pub trace_id: Option<String>,
    pub recorded_at: String,
    pub metadata: Option<serde_json::Value>,
    pub source_exported_at: Option<String>,
    pub transformed_at: Option<String>,
}

/// Public read shape for imported entity-alias rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectionEntityAlias {
    pub canonical_entity_id: EntityId,
    pub alias_text: String,
    pub alias_source: String,
    pub match_evidence: Option<serde_json::Value>,
    pub confidence: f32,
    pub merge_decision: String,
    pub scope_key: ScopeKey,
    pub review_state: String,
    pub is_human_confirmed: bool,
    pub is_human_confirmed_final: bool,
    pub superseded_by_entity_id: Option<EntityId>,
    pub split_from_entity_id: Option<EntityId>,
    pub source_envelope_id: EnvelopeId,
    pub recorded_at: String,
    pub source_exported_at: Option<String>,
    pub transformed_at: Option<String>,
}

/// Public read shape for imported evidence-reference rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectionEvidenceRef {
    pub claim_id: ClaimId,
    pub claim_version_id: Option<ClaimVersionId>,
    pub fetch_handle: String,
    pub source_authority: String,
    pub source_envelope_id: EnvelopeId,
    pub scope_key: ScopeKey,
    pub recorded_at: String,
    pub metadata: Option<serde_json::Value>,
    pub source_exported_at: Option<String>,
    pub transformed_at: Option<String>,
}

/// A conversation session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// UUID v4.
    pub id: String,
    /// Channel identifier (e.g. "repl", "telegram").
    pub channel: String,
    /// ISO 8601 timestamp.
    pub created_at: String,
    /// ISO 8601 timestamp.
    pub updated_at: String,
    /// Optional JSON metadata.
    pub metadata: Option<serde_json::Value>,
    /// Number of messages (populated on list queries).
    pub message_count: u32,
}

/// A single message within a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Auto-increment ID.
    pub id: i64,
    /// Session this message belongs to.
    pub session_id: String,
    /// Role of the speaker.
    pub role: Role,
    /// Message text.
    pub content: String,
    /// Estimated token count (caller-provided).
    pub token_count: Option<u32>,
    /// ISO 8601 timestamp.
    pub created_at: String,
    /// Optional JSON metadata.
    pub metadata: Option<serde_json::Value>,
}

/// A discrete fact in the knowledge store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    /// UUID v4.
    pub id: String,
    /// Categorization namespace.
    pub namespace: String,
    /// The fact text.
    pub content: String,
    /// Where this fact came from.
    pub source: Option<String>,
    /// ISO 8601 timestamp.
    pub created_at: String,
    /// ISO 8601 timestamp.
    pub updated_at: String,
    /// Optional JSON metadata.
    pub metadata: Option<serde_json::Value>,
}

/// A source document that has been chunked and embedded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    /// UUID v4.
    pub id: String,
    /// Document title.
    pub title: String,
    /// File path, URL, or identifier.
    pub source_path: Option<String>,
    /// Categorization namespace.
    pub namespace: String,
    /// ISO 8601 timestamp.
    pub created_at: String,
    /// Optional JSON metadata.
    pub metadata: Option<serde_json::Value>,
    /// Number of chunks (populated on list queries).
    pub chunk_count: u32,
}

/// A chunk produced by the text splitter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextChunk {
    /// Position in the original document (0-based).
    pub index: usize,
    /// The chunk text.
    pub content: String,
    /// Rough token estimate (chars / 4).
    pub token_count_estimate: usize,
}

/// A single search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// The matched text content.
    pub content: String,

    /// Where this result came from.
    pub source: SearchSource,

    /// Combined RRF score. Higher = more relevant.
    pub score: f64,

    /// BM25 rank (1-based) if this result appeared in BM25 results.
    pub bm25_rank: Option<usize>,

    /// Vector rank (1-based) if this result appeared in vector results.
    pub vector_rank: Option<usize>,

    /// Cosine similarity score if computed.
    pub cosine_similarity: Option<f64>,
}

/// Source information for a search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchSource {
    /// Result came from the facts table.
    Fact {
        /// Fact UUID.
        fact_id: String,
        /// Fact namespace.
        namespace: String,
    },
    /// Result came from a document chunk.
    Chunk {
        /// Chunk UUID.
        chunk_id: String,
        /// Parent document UUID.
        document_id: String,
        /// Parent document title.
        document_title: String,
        /// Position within the document (0-based).
        chunk_index: usize,
    },
    /// Result came from a conversation message.
    Message {
        /// Message auto-increment ID.
        message_id: i64,
        /// Session UUID.
        session_id: String,
        /// Message role (user, assistant, etc.).
        role: String,
    },
    /// Result came from an episode (causal record). SearchSource::Episode variant.
    Episode {
        /// First-class episode identity (V9+). Falls back to `document_id + "-ep0"`
        /// for legacy data.
        episode_id: String,
        /// Document ID the episode is attached to.
        document_id: String,
        /// Type of effect (e.g. "test_failure", "regression").
        effect_type: String,
        /// Current outcome.
        outcome: String,
    },
    /// Result came from an imported projection row.
    Projection {
        /// Projection row family, such as `claim_version` or `relation_version`.
        projection_kind: String,
        /// Stable projection-row identity.
        projection_id: String,
        /// Full scope carried by the imported row.
        scope_key: ScopeKey,
        /// Validity start for versioned projections, if any.
        valid_from: Option<String>,
        /// Validity end for versioned projections, if any.
        valid_to: Option<String>,
        /// Authoritative importer-assigned recorded_at.
        recorded_at: String,
        /// Source envelope provenance.
        source_envelope_id: String,
        /// Source authority provenance.
        source_authority: String,
    },
}

impl SearchSource {
    /// Stable result ID used in receipts and replay logs.
    pub fn result_id(&self) -> String {
        match self {
            Self::Fact { fact_id, .. } => format!("fact:{fact_id}"),
            Self::Chunk { chunk_id, .. } => format!("chunk:{chunk_id}"),
            Self::Message { message_id, .. } => format!("msg:{message_id}"),
            Self::Episode { episode_id, .. } => format!("episode:{episode_id}"),
            Self::Projection { projection_id, .. } => format!("projection:{projection_id}"),
        }
    }

    /// Source family label used by explain/receipt surfaces.
    pub fn source_kind(&self) -> &'static str {
        match self {
            Self::Fact { .. } => "fact",
            Self::Chunk { .. } => "chunk",
            Self::Message { .. } => "message",
            Self::Episode { .. } => "episode",
            Self::Projection { .. } => "projection",
        }
    }

    /// Authoritative source row key without the receipt result prefix.
    pub fn source_id(&self) -> String {
        match self {
            Self::Fact { fact_id, .. } => fact_id.clone(),
            Self::Chunk { chunk_id, .. } => chunk_id.clone(),
            Self::Message { message_id, .. } => message_id.to_string(),
            Self::Episode { episode_id, .. } => episode_id.clone(),
            Self::Projection { projection_id, .. } => projection_id.clone(),
        }
    }
}

// ─── Episode Types ─────────────────────────────────────────────

/// Metadata for a causal episode (PRIMITIVES_CONTRACT §4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeMeta {
    /// IDs of the facts/chunks/messages that caused this episode.
    pub cause_ids: Vec<String>,
    /// Type of effect (e.g. "test_failure", "regression", "improvement").
    pub effect_type: String,
    /// Current outcome assessment.
    pub outcome: EpisodeOutcome,
    /// Confidence in the causal link (0.0 to 1.0).
    pub confidence: f32,
    /// Verification status.
    pub verification_status: VerificationStatus,
    /// Links to an EvidenceBundle.run_id (if experimentally verified).
    pub experiment_id: Option<String>,
    /// Bitemporal valid time — when this episode fact was true in the domain.
    pub valid_time: Option<chrono::DateTime<chrono::Utc>>,
    /// Content-addressed digest of the episode fact payload (for supersession chain).
    pub fact_digest: Option<String>,
}

/// Receipt for an as-of bitemporal episode query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeAsOfReceiptV1 {
    pub query_id: String,
    pub as_of_valid: chrono::DateTime<chrono::Utc>,
    pub as_of_recorded: chrono::DateTime<chrono::Utc>,
    pub episode_count: usize,
    pub episode_ids: Vec<String>,
    pub excluded_superseded: usize,
}

/// Outcome of an episode's causal hypothesis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EpisodeOutcome {
    /// Causal link confirmed by experiment.
    Confirmed,
    /// Causal link refuted by experiment.
    Refuted,
    /// Evidence is inconclusive.
    Inconclusive,
    /// Not yet tested.
    Pending,
}

impl EpisodeOutcome {
    /// Convert to the string stored in SQLite.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Refuted => "refuted",
            Self::Inconclusive => "inconclusive",
            Self::Pending => "pending",
        }
    }

    /// Parse from the string stored in SQLite.
    pub fn from_str_value(s: &str) -> Option<Self> {
        match s {
            "confirmed" => Some(Self::Confirmed),
            "refuted" => Some(Self::Refuted),
            "inconclusive" => Some(Self::Inconclusive),
            "pending" => Some(Self::Pending),
            _ => None,
        }
    }
}

impl std::fmt::Display for EpisodeOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Verification status for an episode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum VerificationStatus {
    /// Not yet verified.
    Unverified,
    /// Successfully verified.
    Verified {
        /// Method used for verification.
        method: String,
        /// When verification occurred (ISO 8601).
        at: String,
    },
    /// Verification attempt failed.
    Failed {
        /// Reason for failure.
        reason: String,
        /// When verification was attempted (ISO 8601).
        at: String,
    },
}

// ─── Score Breakdown ───────────────────────────────────────────

/// Detailed score breakdown for explainable search results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreBreakdown {
    /// Final fused RRF score.
    pub rrf_score: f64,
    /// Raw BM25 score reported by SQLite FTS5 (lower is better).
    pub bm25_score: Option<f64>,
    /// Raw vector similarity used for the final vector ordering.
    pub vector_score: Option<f64>,
    /// Raw sparse dot-product score used for sparse ordering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sparse_score: Option<f64>,
    /// Recency contribution added during fusion.
    pub recency_score: Option<f64>,
    /// BM25 rank (1-based).
    pub bm25_rank: Option<usize>,
    /// Vector rank (1-based).
    pub vector_rank: Option<usize>,
    /// Sparse rank (1-based).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sparse_rank: Option<usize>,
    /// Rank from the underlying vector retrieval source before any exact rerank.
    pub vector_source_rank: Option<usize>,
    /// Similarity score from the underlying vector retrieval source before rerank.
    pub vector_source_score: Option<f64>,
    /// BM25 RRF contribution to the final score.
    pub bm25_contribution: Option<f64>,
    /// Vector RRF contribution to the final score.
    pub vector_contribution: Option<f64>,
    /// Sparse RRF contribution to the final score.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sparse_contribution: Option<f64>,
    /// Whether the vector ordering was reranked with exact f32 cosine similarity.
    pub vector_reranked_from_f32: bool,
    /// Configured BM25 fusion weight.
    pub bm25_weight: f64,
    /// Configured vector fusion weight.
    pub vector_weight: f64,
    /// Configured sparse fusion weight.
    #[serde(default)]
    pub sparse_weight: f64,
    /// Configured recency weight when recency is enabled.
    pub recency_weight: Option<f64>,
    /// Configured RRF decay constant.
    pub rrf_k: f64,
}

/// Search result with full score explanation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplainedResult {
    /// The search result.
    pub result: SearchResult,
    /// Score breakdown.
    pub breakdown: ScoreBreakdown,
}

/// Product-facing answer for one explained result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplainedResultAnswerV1 {
    /// Stable result ID used in receipts and replay logs.
    pub result_id: String,
    /// Source family label.
    pub source_kind: String,
    /// Authoritative source row key without the receipt result prefix.
    pub source_id: String,
    /// Plain-language reasons this result appeared.
    pub why_this_result: Vec<String>,
    /// Whether the result matched the text/BM25 lane.
    pub text_match: bool,
    /// Whether the result matched the vector lane.
    pub vector_match: bool,
    /// Whether recency contributed to the score.
    pub recency_applied: bool,
    /// Whether exact f32 rerank/reference scoring was used for the vector lane.
    pub exact_vector_rerank: bool,
    /// Final fused score.
    pub final_score: f64,
}

impl ExplainedResult {
    /// Convert a detailed score breakdown into a practical "why this result" answer.
    pub fn answer(&self) -> ExplainedResultAnswerV1 {
        let text_match = self.breakdown.bm25_rank.is_some();
        let vector_match = self.breakdown.vector_rank.is_some();
        let recency_applied = self.breakdown.recency_score.is_some();
        let mut why_this_result = Vec::new();

        if let Some(rank) = self.breakdown.bm25_rank {
            why_this_result.push(format!("text match rank {rank} contributed to fusion"));
        }
        if let Some(rank) = self.breakdown.vector_rank {
            why_this_result.push(format!("vector match rank {rank} contributed to fusion"));
        }
        if let Some(rank) = self.breakdown.sparse_rank {
            why_this_result.push(format!("sparse match rank {rank} contributed to fusion"));
        }
        if recency_applied {
            why_this_result.push("recency contributed to the fused score".to_string());
        }
        if self.breakdown.vector_reranked_from_f32 {
            why_this_result.push("vector score was checked with exact f32 rerank".to_string());
        }
        if why_this_result.is_empty() {
            why_this_result.push("result survived filtering and deterministic ranking".to_string());
        }

        ExplainedResultAnswerV1 {
            result_id: self.result.source.result_id(),
            source_kind: self.result.source.source_kind().to_string(),
            source_id: self.result.source.source_id(),
            why_this_result,
            text_match,
            vector_match,
            recency_applied,
            exact_vector_rerank: self.breakdown.vector_reranked_from_f32,
            final_score: self.result.score,
        }
    }
}

// ─── Graph Types (PRIMITIVES_CONTRACT §8) ──────────────────────

/// Trait for querying the memory store as a graph.
pub trait GraphView: Send + Sync {
    /// Find neighboring nodes up to `max_depth` hops away.
    fn neighbors(
        &self,
        node_id: &str,
        direction: GraphDirection,
        max_depth: usize,
    ) -> Result<Vec<GraphEdge>, MemoryError>;

    /// Find a path between two nodes (BFS, max depth).
    fn path(
        &self,
        from: &str,
        to: &str,
        max_depth: usize,
    ) -> Result<Option<Vec<String>>, MemoryError>;
}

/// Direction for graph traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphDirection {
    /// Follow outgoing edges.
    Outgoing,
    /// Follow incoming edges.
    Incoming,
    /// Follow edges in both directions.
    Both,
}

/// An edge in the memory graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    /// Source node ID.
    pub source: String,
    /// Target node ID.
    pub target: String,
    /// Type of relationship.
    pub edge_type: GraphEdgeType,
    /// Edge weight (interpretation depends on edge_type).
    pub weight: f64,
    /// Optional metadata.
    pub metadata: Option<serde_json::Value>,
}

/// Type of relationship between graph nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphEdgeType {
    /// Semantic similarity. GraphEdgeType::Semantic variant.
    Semantic {
        /// Cosine similarity between embeddings.
        cosine_similarity: f32,
    },
    /// Temporal proximity. GraphEdgeType::Temporal variant.
    Temporal {
        /// Time delta in seconds.
        delta_secs: u64,
    },
    /// Causal relationship. GraphEdgeType::Causal variant.
    Causal {
        /// Confidence in the causal link.
        confidence: f32,
        /// EvidenceBundle run_ids supporting this link.
        evidence_ids: Vec<String>,
    },
    /// Entity co-occurrence. GraphEdgeType::Entity variant.
    Entity {
        /// Relationship type (e.g. "mentions", "modifies").
        relation: String,
    },
}

/// Embedding displacement between two text embeddings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingDisplacement {
    /// Cosine similarity between the two embeddings.
    pub cosine_similarity: f32,
    /// Euclidean distance between the two embeddings.
    pub euclidean_distance: f32,
    /// Magnitude of the first embedding.
    pub magnitude_a: f32,
    /// Magnitude of the second embedding.
    pub magnitude_b: f32,
}

/// Database statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStats {
    /// Total number of facts.
    pub total_facts: u64,
    /// Total number of documents.
    pub total_documents: u64,
    /// Total number of chunks across all documents.
    pub total_chunks: u64,
    /// Total number of conversation sessions.
    pub total_sessions: u64,
    /// Total number of messages across all sessions.
    pub total_messages: u64,
    /// Database file size in bytes.
    pub database_size_bytes: u64,
    /// Currently configured embedding model.
    pub embedding_model: Option<String>,
    /// Currently configured embedding dimensions.
    pub embedding_dimensions: Option<usize>,
    /// Compile-time selected vector candidate backend.
    pub vector_backend: String,
    /// Backend implementation generation bound into search cache identity.
    pub vector_backend_generation: String,
}

/// Per-surface deletion counts for namespace removal.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct NamespaceDeleteReport {
    /// Facts deleted from the namespace.
    pub facts: usize,
    /// Documents deleted from the namespace.
    pub documents: usize,
    /// Document chunks deleted from the namespace.
    pub chunks: usize,
    /// Messages deleted through namespaced sessions.
    pub messages: usize,
    /// Sessions deleted for the namespace.
    pub sessions: usize,
    /// Episodes deleted with namespaced documents.
    pub episodes: usize,
    /// Projection/import rows deleted or invalidated.
    pub projection_rows: usize,
    /// HNSW pending operations queued by the deletion.
    pub hnsw_ops: usize,
}
