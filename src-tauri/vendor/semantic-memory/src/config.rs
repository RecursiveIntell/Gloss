use crate::error::MemoryError;
use crate::tokenizer::TokenCounter;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// One deterministic correction applied while normalizing numeric configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigCorrection {
    /// Fully-qualified configuration field name.
    pub field: String,
    /// Why the supplied value was invalid.
    pub reason: String,
    /// Human-readable description of the replacement or clamp.
    pub action: String,
}

/// Structured record of all corrections applied during normalization.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigCorrectionReport {
    /// Corrections in deterministic field order.
    pub corrections: Vec<ConfigCorrection>,
}

impl ConfigCorrectionReport {
    /// True when no caller-supplied field required correction.
    pub fn is_empty(&self) -> bool {
        self.corrections.is_empty()
    }

    fn corrected(&mut self, field: &str, reason: &str, action: String) {
        self.corrections.push(ConfigCorrection {
            field: field.to_string(),
            reason: reason.to_string(),
            action,
        });
    }
}

/// Configuration for the memory system.
#[derive(Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Base directory for all storage files (SQLite + HNSW sidecar files).
    /// Replaces the v0.1.0 `database_path` field.
    pub base_dir: PathBuf,

    /// Embedding provider configuration.
    pub embedding: EmbeddingConfig,

    /// Search tuning parameters.
    pub search: SearchConfig,

    /// Chunking parameters.
    pub chunking: ChunkingConfig,

    /// Connection pool configuration.
    pub pool: PoolConfig,

    /// Resource limits.
    pub limits: MemoryLimits,

    /// Custom token counter. None = use EstimateTokenCounter (chars / 4).
    #[serde(skip)]
    pub token_counter: Option<Arc<dyn TokenCounter>>,

    /// HNSW index configuration.
    #[cfg(feature = "hnsw")]
    #[serde(skip)]
    pub hnsw: crate::hnsw::HnswConfig,
}

impl std::fmt::Debug for MemoryConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("MemoryConfig");
        s.field("base_dir", &self.base_dir)
            .field("embedding", &self.embedding)
            .field("search", &self.search)
            .field("chunking", &self.chunking)
            .field("pool", &self.pool)
            .field("limits", &self.limits)
            .field(
                "token_counter",
                &self.token_counter.as_ref().map(|_| "custom"),
            );
        #[cfg(feature = "hnsw")]
        s.field("hnsw", &self.hnsw);
        s.finish()
    }
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            base_dir: PathBuf::from("memory"),
            embedding: EmbeddingConfig::default(),
            search: SearchConfig::default(),
            chunking: ChunkingConfig::default(),
            pool: PoolConfig::default(),
            limits: MemoryLimits::default(),
            token_counter: None,
            #[cfg(feature = "hnsw")]
            hnsw: crate::hnsw::HnswConfig::default(),
        }
    }
}

impl MemoryConfig {
    /// Normalize configuration and return every recoverable numeric correction.
    pub fn normalize_with_report(mut self) -> Result<(Self, ConfigCorrectionReport), MemoryError> {
        self.embedding.normalize_and_validate()?;
        let (limits, report) = self.limits.normalize_with_report();
        self.limits = limits;
        let timeout_cap_secs = self.limits.embedding_timeout.as_secs().max(1);
        self.embedding.timeout_secs = self.embedding.timeout_secs.min(timeout_cap_secs);
        self.search
            .normalize_and_validate(self.embedding.dimensions)?;
        self.chunking.normalize_and_validate()?;
        self.pool.normalize_and_validate()?;
        #[cfg(feature = "hnsw")]
        {
            self.hnsw.dimensions = self.embedding.dimensions;
        }
        Ok((self, report))
    }

    /// Normalize and validate configuration into a concrete runtime shape.
    ///
    /// This is the single canonical config entry point used by store creation.
    pub fn normalize_and_validate(self) -> Result<Self, MemoryError> {
        Ok(self.normalize_with_report()?.0)
    }
}

/// Embedding provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Ollama base URL. Only required when using OllamaEmbedder.
    /// When using CandleEmbedder (default with `candle-embedder` feature),
    /// this field is ignored. Defaults to `http://localhost:11434`.
    pub ollama_url: String,

    /// Embedding model name.
    pub model: String,

    /// Expected embedding dimensions.
    pub dimensions: usize,

    /// Maximum texts to embed in a single API call.
    pub batch_size: usize,

    /// Timeout for embedding requests in seconds.
    pub timeout_secs: u64,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            ollama_url: "http://localhost:11434".to_string(),
            model: "nomic-embed-text".to_string(),
            dimensions: 768,
            batch_size: 32,
            timeout_secs: 30,
        }
    }
}

impl EmbeddingConfig {
    fn normalize_and_validate(&mut self) -> Result<(), MemoryError> {
        if self.dimensions == 0 {
            return Err(MemoryError::InvalidConfig {
                field: "embedding.dimensions",
                reason: "dimensions must be at least 1".to_string(),
            });
        }
        if self.batch_size == 0 {
            self.batch_size = 1;
        }
        if self.timeout_secs == 0 {
            self.timeout_secs = 1;
        }
        // Validate ollama_url only when it will be used. With the
        // candle-embedder feature, the default embedder is CandleEmbedder
        // which does not use Ollama, so a placeholder URL is fine.
        #[cfg(not(feature = "candle-embedder"))]
        {
            let parsed =
                reqwest::Url::parse(&self.ollama_url).map_err(|_| MemoryError::InvalidConfig {
                    field: "embedding.ollama_url",
                    reason: "must be an absolute http:// or https:// URL".to_string(),
                })?;
            match parsed.scheme() {
                "http" | "https" if parsed.host_str().is_some() => {}
                _ => {
                    return Err(MemoryError::InvalidConfig {
                        field: "embedding.ollama_url",
                        reason: "must be an absolute http:// or https:// URL".to_string(),
                    })
                }
            }
        }
        // With candle-embedder, skip URL validation — the field is ignored
        // by CandleEmbedder. If OllamaEmbedder is used explicitly via
        // open_with_embedder, it does its own URL handling.
        #[cfg(feature = "candle-embedder")]
        {
            let _ = &self.ollama_url; // suppress unused field warning
        }
        Ok(())
    }
}

/// Search tuning parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    /// Weight for BM25 score in RRF fusion.
    pub bm25_weight: f64,

    /// Weight for vector similarity in RRF fusion.
    pub vector_weight: f64,

    /// Weight for sparse dot-product ranking in RRF fusion.
    /// Defaults to 0.0 so existing BM25+dense behavior is unchanged.
    #[serde(default = "default_zero")]
    pub sparse_weight: f64,

    /// Maximum sparse candidates admitted to fusion.
    #[serde(default = "default_sparse_top_k")]
    pub sparse_top_k: usize,

    /// Minimum sparse dot-product score admitted to fusion.
    #[serde(default = "default_zero")]
    pub sparse_min_score: f64,

    /// Explicitly allow dense-only embedders to derive generic sparse weights.
    /// This is disabled by default and the result must not be described as SPLADE.
    #[serde(default)]
    pub derive_sparse_from_dense: bool,

    /// Maximum dense dimensions retained by explicit generic sparse derivation.
    #[serde(default = "default_sparse_derive_top_k")]
    pub sparse_derive_top_k: usize,

    /// Minimum absolute dense value retained by generic sparse derivation.
    #[serde(default = "default_sparse_derive_min_weight")]
    pub sparse_derive_min_weight: f32,

    /// Weight for late interaction (ColBERT MaxSim) in RRF fusion.
    /// Defaults to 0.0 (disabled). Set to 1.0 to enable as 3rd RRF signal.
    #[serde(default = "default_zero")]
    pub late_interaction_weight: f64,

    /// BM25 k1 parameter. Controls term frequency saturation.
    /// Default: 1.2 (FTS5 standard). Lower (0.8-1.0) helps with technical content.
    pub bm25_k1: f64,

    /// BM25 b parameter. Controls document length normalization.
    /// Default: 0.75 (FTS5 standard).
    pub bm25_b: f64,

    /// Optional per-namespace weight multipliers.
    /// Empty = no weighting (all namespaces scored equally).
    pub namespace_weights: std::collections::HashMap<String, f64>,

    /// RRF constant (k). Controls rank importance decay.
    pub rrf_k: f64,

    /// Number of candidates from each search method before fusion.
    pub candidate_pool_size: usize,

    /// Default number of results to return.
    pub default_top_k: usize,

    /// Minimum cosine similarity threshold for vector candidates.
    pub min_similarity: f64,

    /// Optional recency boost. If enabled, results are boosted based on how
    /// recently they were created/updated. The value is the half-life in days —
    /// a fact that is `recency_half_life_days` old gets 50% of the recency boost.
    /// None = no recency weighting (current behavior, default).
    pub recency_half_life_days: Option<f64>,

    /// Weight of the recency boost relative to BM25 and vector scores in RRF.
    /// Only used when recency_half_life_days is Some.
    /// Default: 0.5
    pub recency_weight: f64,

    /// When true, rerank top HNSW candidates using exact f32 cosine similarity
    /// from SQLite. Improves recall at the cost of one batched SQL query.
    /// Only applies when HNSW feature is enabled.
    /// Default: true
    pub rerank_from_f32: bool,

    /// Optional derived-vector candidate backend. Disabled by default because
    /// raw f32 embeddings remain authoritative.
    #[serde(default)]
    pub derived_vector_backend: DerivedVectorBackendPolicy,

    /// TurboQuant polar angle bits when the TurboQuant candidate backend is enabled.
    #[serde(default = "default_turbo_quant_bits")]
    pub turbo_quant_bits: u8,

    /// TurboQuant QJL projection count when the TurboQuant candidate backend is enabled.
    #[serde(default = "default_turbo_quant_projections")]
    pub turbo_quant_projections: usize,

    /// TurboQuant profile seed when the TurboQuant candidate backend is enabled.
    #[serde(default)]
    pub turbo_quant_seed: u64,

    /// Require exact f32 rerank for TurboQuant candidates. Defaults to true.
    #[serde(default = "default_true")]
    pub turbo_quant_require_exact_rerank: bool,

    /// Matryoshka candidate-stage embedding dimensions for 2-stage search.
    /// When set to Some(dim) and the `matryoshka` feature is enabled, the query
    /// embedding is truncated to `dim` dimensions for candidate retrieval, then
    /// reranked with the full embedding. Disabled by default because it requires
    /// a compatible truncated-vector index; callers opt in explicitly.
    #[serde(default = "default_candidate_dims")]
    pub candidate_dims: Option<usize>,

    /// When true, compress search result content using SimpleMem-style semantic
    /// compression (first sentence + key terms, capped at 150 chars).
    /// Defaults to false.
    #[serde(default)]
    pub compress_results: bool,

    /// When true, use q8-quantized embeddings for initial candidate selection
    /// in vector search, then rerank top candidates with exact f32 cosine.
    /// Reduces f32 computations by ~20x on large stores with zero recall loss.
    /// Defaults to false (existing brute-force behavior).
    #[serde(default)]
    pub use_compressed_candidates: bool,
}

/// Candidate backend policy for rebuildable derived vector artifacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DerivedVectorBackendPolicy {
    /// Use authoritative raw f32 embeddings for vector candidate generation.
    #[default]
    Disabled,
    /// Use TurboQuant only to generate candidates, then exact rerank by default.
    TurboQuantCandidateOnly,
    /// Use a generation-level proveKV/poly-kv shared pool only to generate candidates,
    /// then exact-rerank against authoritative f32 embeddings.
    ///
    /// This is deliberately not a replacement for SQLite f32 storage or for prompt/KV
    /// prefix reuse. It is a rebuildable derived artifact over an embedding snapshot.
    ProveKvPoolCandidateOnly,
}

const fn default_turbo_quant_bits() -> u8 {
    8
}

const fn default_turbo_quant_projections() -> usize {
    64
}

const fn default_true() -> bool {
    true
}

const fn default_zero() -> f64 {
    0.0
}

const fn default_sparse_top_k() -> usize {
    50
}

const fn default_sparse_derive_top_k() -> usize {
    128
}

const fn default_sparse_derive_min_weight() -> f32 {
    0.01
}

const fn default_candidate_dims() -> Option<usize> {
    None
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            bm25_weight: 1.0,
            vector_weight: 1.0,
            sparse_weight: 0.0,
            sparse_top_k: default_sparse_top_k(),
            sparse_min_score: 0.0,
            derive_sparse_from_dense: false,
            sparse_derive_top_k: default_sparse_derive_top_k(),
            sparse_derive_min_weight: default_sparse_derive_min_weight(),
            late_interaction_weight: 0.15,
            bm25_k1: 1.2,
            bm25_b: 0.75,
            namespace_weights: std::collections::HashMap::new(),
            rrf_k: 60.0,
            candidate_pool_size: 50,
            default_top_k: 5,
            min_similarity: 0.3,
            recency_half_life_days: None,
            recency_weight: 0.5,
            rerank_from_f32: true,
            derived_vector_backend: DerivedVectorBackendPolicy::Disabled,
            turbo_quant_bits: default_turbo_quant_bits(),
            turbo_quant_projections: default_turbo_quant_projections(),
            turbo_quant_seed: 0,
            turbo_quant_require_exact_rerank: true,
            candidate_dims: default_candidate_dims(),
            compress_results: false,
            use_compressed_candidates: false,
        }
    }
}

impl SearchConfig {
    pub(crate) fn uses_turbo_quant_backend(&self) -> bool {
        self.derived_vector_backend == DerivedVectorBackendPolicy::TurboQuantCandidateOnly
    }

    pub(crate) fn uses_provekv_pool_backend(&self) -> bool {
        self.derived_vector_backend == DerivedVectorBackendPolicy::ProveKvPoolCandidateOnly
    }

    pub(crate) fn uses_derived_vector_backend(&self) -> bool {
        self.uses_turbo_quant_backend() || self.uses_provekv_pool_backend()
    }

    fn normalize_and_validate(&mut self, embedding_dimensions: usize) -> Result<(), MemoryError> {
        const MAX_WEIGHT: f64 = 1_000_000.0;
        const MAX_CANDIDATE_POOL: usize = 1_000_000;
        const MAX_TOP_K: usize = 10_000;
        const MAX_SPARSE_DIMENSIONS: usize = 1_000_000;
        const MAX_RRF_K: f64 = 1_000_000.0;
        const MAX_RECENCY_DAYS: f64 = 365_000.0;
        #[cfg(not(feature = "turbo-quant-codec"))]
        let _ = embedding_dimensions;
        if self.candidate_pool_size == 0 {
            self.candidate_pool_size = 1;
        }
        if self.default_top_k == 0 {
            self.default_top_k = 1;
        }
        self.candidate_pool_size = self.candidate_pool_size.max(self.default_top_k);
        if self.sparse_top_k == 0 {
            self.sparse_top_k = 1;
        }
        if self.sparse_derive_top_k == 0 {
            self.sparse_derive_top_k = 1;
        }
        if self.candidate_pool_size > MAX_CANDIDATE_POOL {
            return Err(MemoryError::InvalidConfig {
                field: "search.candidate_pool_size",
                reason: format!("candidate_pool_size must be <= {MAX_CANDIDATE_POOL}"),
            });
        }
        if self.default_top_k > MAX_TOP_K {
            return Err(MemoryError::InvalidConfig {
                field: "search.default_top_k",
                reason: format!("default_top_k must be <= {MAX_TOP_K}"),
            });
        }
        if self.sparse_top_k > MAX_CANDIDATE_POOL {
            return Err(MemoryError::InvalidConfig {
                field: "search.sparse_top_k",
                reason: format!("sparse_top_k must be <= {MAX_CANDIDATE_POOL}"),
            });
        }
        if self.sparse_derive_top_k > MAX_SPARSE_DIMENSIONS {
            return Err(MemoryError::InvalidConfig {
                field: "search.sparse_derive_top_k",
                reason: format!("sparse_derive_top_k must be <= {MAX_SPARSE_DIMENSIONS}"),
            });
        }
        if !self.rrf_k.is_finite() || self.rrf_k <= 0.0 {
            return Err(MemoryError::InvalidConfig {
                field: "search.rrf_k",
                reason: "rrf_k must be finite and > 0".to_string(),
            });
        }
        if self.rrf_k > MAX_RRF_K {
            return Err(MemoryError::InvalidConfig {
                field: "search.rrf_k",
                reason: format!("rrf_k must be <= {MAX_RRF_K}"),
            });
        }
        if !self.bm25_weight.is_finite() || !(0.0..=MAX_WEIGHT).contains(&self.bm25_weight) {
            return Err(MemoryError::InvalidConfig {
                field: "search.bm25_weight",
                reason: format!("bm25_weight must be finite and within [0, {MAX_WEIGHT}]"),
            });
        }
        if !self.vector_weight.is_finite() || !(0.0..=MAX_WEIGHT).contains(&self.vector_weight) {
            return Err(MemoryError::InvalidConfig {
                field: "search.vector_weight",
                reason: format!("vector_weight must be finite and within [0, {MAX_WEIGHT}]"),
            });
        }
        if !self.sparse_weight.is_finite() || !(0.0..=MAX_WEIGHT).contains(&self.sparse_weight) {
            return Err(MemoryError::InvalidConfig {
                field: "search.sparse_weight",
                reason: format!("sparse_weight must be finite and within [0, {MAX_WEIGHT}]"),
            });
        }
        if !self.sparse_min_score.is_finite() || self.sparse_min_score.abs() > MAX_WEIGHT {
            return Err(MemoryError::InvalidConfig {
                field: "search.sparse_min_score",
                reason: format!("sparse_min_score must be finite and within ±{MAX_WEIGHT}"),
            });
        }
        if !self.sparse_derive_min_weight.is_finite()
            || !(0.0..=MAX_WEIGHT as f32).contains(&self.sparse_derive_min_weight)
        {
            return Err(MemoryError::InvalidConfig {
                field: "search.sparse_derive_min_weight",
                reason: format!(
                    "sparse_derive_min_weight must be finite and within [0, {MAX_WEIGHT}]"
                ),
            });
        }
        if !self.late_interaction_weight.is_finite()
            || !(0.0..=MAX_WEIGHT).contains(&self.late_interaction_weight)
        {
            return Err(MemoryError::InvalidConfig {
                field: "search.late_interaction_weight",
                reason: format!(
                    "late_interaction_weight must be finite and within [0, {MAX_WEIGHT}]"
                ),
            });
        }
        if !self.bm25_k1.is_finite() || !(0.0..=100.0).contains(&self.bm25_k1) {
            return Err(MemoryError::InvalidConfig {
                field: "search.bm25_k1",
                reason: "bm25_k1 must be finite and within [0, 100]".to_string(),
            });
        }
        if !self.bm25_b.is_finite() || !(0.0..=1.0).contains(&self.bm25_b) {
            return Err(MemoryError::InvalidConfig {
                field: "search.bm25_b",
                reason: "bm25_b must be finite and within [0, 1]".to_string(),
            });
        }
        if let Some((_, weight)) = self
            .namespace_weights
            .iter()
            .find(|(_, weight)| !weight.is_finite() || !(0.0..=MAX_WEIGHT).contains(weight))
        {
            return Err(MemoryError::InvalidConfig {
                field: "search.namespace_weights",
                reason: format!(
                    "namespace weights must be finite and within [0, {MAX_WEIGHT}]; found {weight}"
                ),
            });
        }
        if !self.recency_weight.is_finite() || !(0.0..=MAX_WEIGHT).contains(&self.recency_weight) {
            return Err(MemoryError::InvalidConfig {
                field: "search.recency_weight",
                reason: format!("recency_weight must be finite and within [0, {MAX_WEIGHT}]"),
            });
        }
        if !self.min_similarity.is_finite() || !(-1.0..=1.0).contains(&self.min_similarity) {
            return Err(MemoryError::InvalidConfig {
                field: "search.min_similarity",
                reason: "min_similarity must be finite and within [-1.0, 1.0]".to_string(),
            });
        }
        if matches!(self.recency_half_life_days, Some(v) if !v.is_finite()) {
            return Err(MemoryError::InvalidConfig {
                field: "search.recency_half_life_days",
                reason: "recency_half_life_days must be finite".to_string(),
            });
        }
        if matches!(self.recency_half_life_days, Some(v) if v <= 0.0) {
            return Err(MemoryError::InvalidConfig {
                field: "search.recency_half_life_days",
                reason: "recency_half_life_days must be > 0 when enabled".to_string(),
            });
        }
        if matches!(self.recency_half_life_days, Some(v) if v > MAX_RECENCY_DAYS) {
            return Err(MemoryError::InvalidConfig {
                field: "search.recency_half_life_days",
                reason: format!("recency_half_life_days must be <= {MAX_RECENCY_DAYS}"),
            });
        }
        if !(2..=16).contains(&self.turbo_quant_bits) {
            return Err(MemoryError::InvalidConfig {
                field: "search.turbo_quant_bits",
                reason: "TurboQuant bits must be within 2..=16".to_string(),
            });
        }
        if self.turbo_quant_projections == 0 || self.turbo_quant_projections > MAX_CANDIDATE_POOL {
            return Err(MemoryError::InvalidConfig {
                field: "search.turbo_quant_projections",
                reason: format!("TurboQuant projections must be within 1..={MAX_CANDIDATE_POOL}"),
            });
        }
        if matches!(self.candidate_dims, Some(0))
            || matches!(self.candidate_dims, Some(dim) if dim > embedding_dimensions)
        {
            return Err(MemoryError::InvalidConfig {
                field: "search.candidate_dims",
                reason: format!(
                    "candidate_dims must be within 1..={embedding_dimensions} when enabled"
                ),
            });
        }
        if self.uses_turbo_quant_backend() {
            #[cfg(not(feature = "turbo-quant-codec"))]
            {
                return Err(MemoryError::InvalidConfig {
                    field: "search.derived_vector_backend",
                    reason: "turbo_quant_candidate_only requires the turbo-quant-codec feature"
                        .to_string(),
                });
            }
            #[cfg(feature = "turbo-quant-codec")]
            {
                if embedding_dimensions % 2 != 0 {
                    return Err(MemoryError::InvalidConfig {
                        field: "embedding.dimensions",
                        reason: "TurboQuant requires even embedding dimensions".to_string(),
                    });
                }
                if self.turbo_quant_projections == 0 {
                    return Err(MemoryError::InvalidConfig {
                        field: "search.turbo_quant_projections",
                        reason: "TurboQuant projections must be at least 1".to_string(),
                    });
                }
                if !(2..=16).contains(&self.turbo_quant_bits) {
                    return Err(MemoryError::InvalidConfig {
                        field: "search.turbo_quant_bits",
                        reason: "TurboQuant bits must be within 2..=16".to_string(),
                    });
                }
            }
        }
        if self.uses_derived_vector_backend() && !self.turbo_quant_require_exact_rerank {
            return Err(MemoryError::InvalidConfig {
                field: "search.turbo_quant_require_exact_rerank",
                reason: "derived vector candidate backends require exact f32 rerank".to_string(),
            });
        }
        Ok(())
    }
}

/// Chunking strategy to use when splitting text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChunkingStrategy {
    /// Plain recursive splitting (current/default behavior).
    #[default]
    Plain,
    /// Sentence-boundary-aware chunking with configurable overlap.
    Sentence,
    /// Code-aware chunking that avoids splitting inside function bodies.
    /// Detects Rust, Python, and TypeScript blocks.
    Code,
    /// Markdown-header-based chunking that splits on header boundaries.
    Markdown,
}

/// Text chunking parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkingConfig {
    /// Target chunk size in characters.
    pub target_size: usize,

    /// Minimum chunk size. Chunks smaller than this are merged with neighbors.
    pub min_size: usize,

    /// Maximum chunk size. Chunks larger than this are force-split.
    pub max_size: usize,

    /// Overlap between adjacent chunks in characters.
    pub overlap: usize,

    /// Chunking strategy to use. Defaults to [`ChunkingStrategy::Plain`]
    /// for backward compatibility.
    #[serde(default)]
    pub strategy: ChunkingStrategy,
}

impl Default for ChunkingConfig {
    fn default() -> Self {
        Self {
            target_size: 1000,
            min_size: 100,
            max_size: 2000,
            overlap: 200,
            strategy: ChunkingStrategy::default(),
        }
    }
}

impl ChunkingConfig {
    fn normalize_and_validate(&mut self) -> Result<(), MemoryError> {
        if self.min_size == 0 {
            self.min_size = 1;
        }
        if self.max_size == 0 {
            return Err(MemoryError::InvalidConfig {
                field: "chunking.max_size",
                reason: "max_size must be at least 1".to_string(),
            });
        }
        if self.max_size < self.min_size {
            return Err(MemoryError::InvalidConfig {
                field: "chunking.max_size",
                reason: "max_size must be >= min_size".to_string(),
            });
        }
        if self.target_size < self.min_size {
            self.target_size = self.min_size;
        }
        if self.target_size > self.max_size {
            self.target_size = self.max_size;
        }
        if self.overlap >= self.min_size {
            self.overlap = self.min_size.saturating_sub(1);
        }
        Ok(())
    }
}

/// Connection pool configuration for SQLite.
///
/// Controls busy timeout and WAL checkpoint behavior. These defaults
/// are tuned for a single-process server on local SSD storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolConfig {
    /// SQLite busy timeout in milliseconds.
    /// Default: 5000 (5 seconds).
    pub busy_timeout_ms: u32,

    /// WAL auto-checkpoint threshold in pages.
    /// Default: 1000 (~4 MB with 4KB pages).
    pub wal_autocheckpoint: u32,

    /// Enable WAL mode. Should almost always be true.
    /// Default: true.
    pub enable_wal: bool,

    /// Number of reader connections kept in the pool.
    /// Writes still flow through a single writer connection because SQLite
    /// allows only one concurrent writer, but readers can proceed concurrently
    /// under WAL semantics.
    pub max_read_connections: usize,

    /// Timeout in seconds for acquiring a reader connection from the pool.
    /// Default: 30 seconds.
    pub reader_timeout_secs: u64,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            busy_timeout_ms: 5000,
            wal_autocheckpoint: 1000,
            enable_wal: true,
            max_read_connections: 4,
            reader_timeout_secs: 30,
        }
    }
}

impl PoolConfig {
    fn normalize_and_validate(&mut self) -> Result<(), MemoryError> {
        if self.busy_timeout_ms == 0 {
            self.busy_timeout_ms = 1;
        }
        if self.wal_autocheckpoint == 0 {
            self.wal_autocheckpoint = 1;
        }
        if self.max_read_connections == 0 {
            return Err(MemoryError::InvalidConfig {
                field: "pool.max_read_connections",
                reason: "set pool.max_read_connections to at least 1".to_string(),
            });
        }
        if self.reader_timeout_secs == 0 {
            self.reader_timeout_secs = 1;
        }
        self.reader_timeout_secs = self.reader_timeout_secs.min(300);
        Ok(())
    }
}

/// Resource limits for the memory system.
///
/// Prevents runaway resource usage. All limits have defaults tuned for
/// a laptop-class server (8GB RAM, SSD storage).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryLimits {
    /// Maximum number of facts per namespace.
    /// Default: 100_000.
    pub max_facts_per_namespace: usize,

    /// Maximum number of chunks per document.
    /// Default: 1_000.
    pub max_chunks_per_document: usize,

    /// Maximum content size in bytes for a single fact or message.
    /// Default: 1 MB (1_048_576 bytes).
    pub max_content_bytes: usize,

    /// Maximum number of concurrent embedding requests.
    /// Hard-capped at 32 regardless of config.
    /// Default: 8.
    pub max_embedding_concurrency: usize,

    /// Maximum total database size in bytes. 0 = unlimited.
    /// Default: 0 (unlimited).
    pub max_db_size_bytes: u64,

    /// Embedding request timeout.
    /// Default: 30 seconds.
    #[serde(with = "duration_secs")]
    pub embedding_timeout: Duration,
}

impl Default for MemoryLimits {
    fn default() -> Self {
        Self {
            max_facts_per_namespace: 100_000,
            max_chunks_per_document: 1_000,
            max_content_bytes: 1_048_576,
            max_embedding_concurrency: 8,
            max_db_size_bytes: 0,
            embedding_timeout: Duration::from_secs(30),
        }
    }
}

impl MemoryLimits {
    /// Normalize every resource field independently and return a correction report.
    /// Valid fields are never replaced because an unrelated field is invalid.
    pub fn normalize_with_report(mut self) -> (Self, ConfigCorrectionReport) {
        const MAX_FACTS_PER_NAMESPACE: usize = 10_000_000;
        const MAX_CHUNKS_PER_DOCUMENT: usize = 1_000_000;
        const MAX_CONTENT_BYTES: usize = 64 * 1024 * 1024;
        const MAX_EMBEDDING_TIMEOUT_SECS: u64 = 300;
        const MAX_DB_SIZE_BYTES: u64 = 1 << 50;

        let defaults = Self::default();
        let mut report = ConfigCorrectionReport::default();
        if self.max_facts_per_namespace == 0 {
            self.max_facts_per_namespace = defaults.max_facts_per_namespace;
            report.corrected(
                "limits.max_facts_per_namespace",
                "must be at least 1",
                format!("replaced with default {}", self.max_facts_per_namespace),
            );
        } else if self.max_facts_per_namespace > MAX_FACTS_PER_NAMESPACE {
            self.max_facts_per_namespace = MAX_FACTS_PER_NAMESPACE;
            report.corrected(
                "limits.max_facts_per_namespace",
                "exceeds hard upper bound",
                format!("clamped to {MAX_FACTS_PER_NAMESPACE}"),
            );
        }
        if self.max_chunks_per_document == 0 {
            self.max_chunks_per_document = defaults.max_chunks_per_document;
            report.corrected(
                "limits.max_chunks_per_document",
                "must be at least 1",
                format!("replaced with default {}", self.max_chunks_per_document),
            );
        } else if self.max_chunks_per_document > MAX_CHUNKS_PER_DOCUMENT {
            self.max_chunks_per_document = MAX_CHUNKS_PER_DOCUMENT;
            report.corrected(
                "limits.max_chunks_per_document",
                "exceeds hard upper bound",
                format!("clamped to {MAX_CHUNKS_PER_DOCUMENT}"),
            );
        }
        if self.max_content_bytes == 0 {
            self.max_content_bytes = defaults.max_content_bytes;
            report.corrected(
                "limits.max_content_bytes",
                "must be at least 1",
                format!("replaced with default {}", self.max_content_bytes),
            );
        } else if self.max_content_bytes > MAX_CONTENT_BYTES {
            self.max_content_bytes = MAX_CONTENT_BYTES;
            report.corrected(
                "limits.max_content_bytes",
                "exceeds hard upper bound",
                format!("clamped to {MAX_CONTENT_BYTES}"),
            );
        }
        if self.max_embedding_concurrency == 0 {
            self.max_embedding_concurrency = 1;
            report.corrected(
                "limits.max_embedding_concurrency",
                "must be at least 1",
                "clamped to 1".to_string(),
            );
        } else if self.max_embedding_concurrency > 32 {
            self.max_embedding_concurrency = 32;
            report.corrected(
                "limits.max_embedding_concurrency",
                "exceeds hard upper bound",
                "clamped to 32".to_string(),
            );
        }
        if self.max_db_size_bytes > MAX_DB_SIZE_BYTES {
            self.max_db_size_bytes = MAX_DB_SIZE_BYTES;
            report.corrected(
                "limits.max_db_size_bytes",
                "exceeds hard upper bound",
                format!("clamped to {MAX_DB_SIZE_BYTES}"),
            );
        }
        if self.embedding_timeout.is_zero() {
            self.embedding_timeout = Duration::from_secs(1);
            report.corrected(
                "limits.embedding_timeout",
                "must be at least one second",
                "clamped to 1 second".to_string(),
            );
        } else if self.embedding_timeout.as_secs() > MAX_EMBEDDING_TIMEOUT_SECS {
            self.embedding_timeout = Duration::from_secs(MAX_EMBEDDING_TIMEOUT_SECS);
            report.corrected(
                "limits.embedding_timeout",
                "exceeds hard upper bound",
                format!("clamped to {MAX_EMBEDDING_TIMEOUT_SECS} seconds"),
            );
        }
        (self, report)
    }

    /// Normalize and validate limits to hard caps.
    pub fn normalize_and_validate(self) -> Result<Self, MemoryError> {
        Ok(self.normalize_with_report().0)
    }

    /// Backward-compatible alias for callers that only need clamped limits.
    ///
    /// Falls back to defaults if the caller-provided limits are invalid.
    /// Default limits are infallible so the fallback path cannot fail.
    pub fn validated(self) -> Self {
        let (limits, report) = self.normalize_with_report();
        for correction in report.corrections {
            tracing::warn!(
                field = %correction.field,
                reason = %correction.reason,
                action = %correction.action,
                "corrected invalid memory limit"
            );
        }
        limits
    }
}

mod duration_secs {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_secs())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let secs = u64::deserialize(d)?;
        Ok(Duration::from_secs(secs))
    }
}

#[cfg(test)]
mod hardening_tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn one_invalid_limit_preserves_every_other_valid_limit() {
        let original = MemoryLimits {
            max_facts_per_namespace: 0,
            max_chunks_per_document: 321,
            max_content_bytes: 654_321,
            max_embedding_concurrency: 7,
            max_db_size_bytes: 987_654_321,
            embedding_timeout: Duration::from_secs(19),
        };

        let corrected = original.validated();
        assert_eq!(
            corrected.max_facts_per_namespace,
            MemoryLimits::default().max_facts_per_namespace
        );
        assert_eq!(corrected.max_chunks_per_document, 321);
        assert_eq!(corrected.max_content_bytes, 654_321);
        assert_eq!(corrected.max_embedding_concurrency, 7);
        assert_eq!(corrected.max_db_size_bytes, 987_654_321);
        assert_eq!(corrected.embedding_timeout, Duration::from_secs(19));
    }

    #[test]
    fn memory_config_returns_structured_correction_report() {
        let mut config = MemoryConfig::default();
        config.limits.max_facts_per_namespace = 0;
        let (normalized, report) = config.normalize_with_report().unwrap();
        assert_eq!(
            normalized.limits.max_facts_per_namespace,
            MemoryLimits::default().max_facts_per_namespace
        );
        assert_eq!(report.corrections.len(), 1);
        assert_eq!(
            report.corrections[0].field,
            "limits.max_facts_per_namespace"
        );
    }

    proptest! {
        #[test]
        fn arbitrary_single_bad_limit_preserves_other_valid_fields(
            chunks in 1usize..10_000,
            content in 1usize..10_000_000,
            concurrency in 1usize..=32,
            db_size in 0u64..=(1u64 << 50),
            timeout in 1u64..=300,
        ) {
            let original = MemoryLimits {
                max_facts_per_namespace: 0,
                max_chunks_per_document: chunks,
                max_content_bytes: content,
                max_embedding_concurrency: concurrency,
                max_db_size_bytes: db_size,
                embedding_timeout: Duration::from_secs(timeout),
            };
            let (corrected, report) = original.normalize_with_report();
            prop_assert_eq!(corrected.max_chunks_per_document, chunks);
            prop_assert_eq!(corrected.max_content_bytes, content);
            prop_assert_eq!(corrected.max_embedding_concurrency, concurrency);
            prop_assert_eq!(corrected.max_db_size_bytes, db_size);
            prop_assert_eq!(corrected.embedding_timeout, Duration::from_secs(timeout));
            prop_assert_eq!(report.corrections.len(), 1);
            prop_assert_eq!(report.corrections[0].field.as_str(), "limits.max_facts_per_namespace");
        }
    }

    #[test]
    fn search_rejects_every_unbounded_or_non_finite_numeric_family() {
        let mut cases = Vec::new();

        let mut config = SearchConfig::default();
        config.late_interaction_weight = f64::NAN;
        cases.push(config);

        let mut config = SearchConfig::default();
        config.bm25_k1 = f64::INFINITY;
        cases.push(config);

        let mut config = SearchConfig::default();
        config.bm25_b = 1.1;
        cases.push(config);

        let mut config = SearchConfig::default();
        config.namespace_weights.insert("bad".into(), f64::INFINITY);
        cases.push(config);

        let mut config = SearchConfig::default();
        config.candidate_pool_size = usize::MAX;
        cases.push(config);

        let mut config = SearchConfig::default();
        config.sparse_top_k = usize::MAX;
        cases.push(config);

        let mut config = SearchConfig::default();
        config.default_top_k = usize::MAX;
        cases.push(config);

        for mut config in cases {
            assert!(
                config.normalize_and_validate(768).is_err(),
                "invalid numeric config was accepted: {config:?}"
            );
        }
    }

    #[test]
    fn search_config_nan_weight_preserves_other_numeric_fields() {
        let mut config = SearchConfig::default();
        let expected_vector_weight = config.vector_weight;
        let expected_sparse_weight = config.sparse_weight;
        let expected_candidate_pool_size = config.candidate_pool_size;
        let expected_rerank_flag = config.rerank_from_f32;
        let expected_derive_sparse = config.derive_sparse_from_dense;

        config.bm25_weight = f64::NAN;

        let err = config.normalize_and_validate(768).unwrap_err();
        match err {
            MemoryError::InvalidConfig { field, reason: _ } => {
                assert_eq!(field, "search.bm25_weight")
            }
            _ => panic!("expected InvalidConfig for non-finite numeric field"),
        }

        assert_eq!(config.vector_weight, expected_vector_weight);
        assert_eq!(config.sparse_weight, expected_sparse_weight);
        assert_eq!(config.candidate_pool_size, expected_candidate_pool_size);
        assert_eq!(config.rerank_from_f32, expected_rerank_flag);
        assert_eq!(config.derive_sparse_from_dense, expected_derive_sparse);
    }
}
