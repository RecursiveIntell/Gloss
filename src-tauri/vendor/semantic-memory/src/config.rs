use crate::error::MemoryError;
use crate::tokenizer::TokenCounter;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

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
    /// Normalize and validate configuration into a concrete runtime shape.
    ///
    /// This is the single canonical config entry point used by store creation.
    pub fn normalize_and_validate(mut self) -> Result<Self, MemoryError> {
        self.embedding.normalize_and_validate()?;
        self.limits = self.limits.normalize_and_validate()?;
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
        Ok(self)
    }
}

/// Embedding provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Ollama base URL.
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

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            bm25_weight: 1.0,
            vector_weight: 1.0,
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
        }
    }
}

impl SearchConfig {
    pub(crate) fn uses_turbo_quant_backend(&self) -> bool {
        self.derived_vector_backend == DerivedVectorBackendPolicy::TurboQuantCandidateOnly
    }

    fn normalize_and_validate(&mut self, embedding_dimensions: usize) -> Result<(), MemoryError> {
        #[cfg(not(feature = "turbo-quant-codec"))]
        let _ = embedding_dimensions;
        if self.candidate_pool_size == 0 {
            self.candidate_pool_size = 1;
        }
        if self.default_top_k == 0 {
            self.default_top_k = 1;
        }
        self.candidate_pool_size = self.candidate_pool_size.max(self.default_top_k);
        if !self.rrf_k.is_finite() || self.rrf_k <= 0.0 {
            return Err(MemoryError::InvalidConfig {
                field: "search.rrf_k",
                reason: "rrf_k must be finite and > 0".to_string(),
            });
        }
        if !self.bm25_weight.is_finite() || self.bm25_weight < 0.0 {
            return Err(MemoryError::InvalidConfig {
                field: "search.bm25_weight",
                reason: "bm25_weight must be finite and >= 0".to_string(),
            });
        }
        if !self.vector_weight.is_finite() || self.vector_weight < 0.0 {
            return Err(MemoryError::InvalidConfig {
                field: "search.vector_weight",
                reason: "vector_weight must be finite and >= 0".to_string(),
            });
        }
        if !self.recency_weight.is_finite() || self.recency_weight < 0.0 {
            return Err(MemoryError::InvalidConfig {
                field: "search.recency_weight",
                reason: "recency_weight must be finite and >= 0".to_string(),
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
                if !self.turbo_quant_require_exact_rerank {
                    return Err(MemoryError::InvalidConfig {
                        field: "search.turbo_quant_require_exact_rerank",
                        reason: "TurboQuant candidate backend requires exact f32 rerank"
                            .to_string(),
                    });
                }
            }
        }
        Ok(())
    }
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
}

impl Default for ChunkingConfig {
    fn default() -> Self {
        Self {
            target_size: 1000,
            min_size: 100,
            max_size: 2000,
            overlap: 200,
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
    /// Normalize and validate limits to hard caps.
    pub fn normalize_and_validate(mut self) -> Result<Self, MemoryError> {
        if self.max_facts_per_namespace == 0 {
            return Err(MemoryError::InvalidConfig {
                field: "limits.max_facts_per_namespace",
                reason: "must be at least 1".to_string(),
            });
        }
        if self.max_chunks_per_document == 0 {
            return Err(MemoryError::InvalidConfig {
                field: "limits.max_chunks_per_document",
                reason: "must be at least 1".to_string(),
            });
        }
        if self.max_content_bytes == 0 {
            return Err(MemoryError::InvalidConfig {
                field: "limits.max_content_bytes",
                reason: "must be at least 1".to_string(),
            });
        }
        // Hard cap: concurrency at 32
        if self.max_embedding_concurrency > 32 {
            self.max_embedding_concurrency = 32;
        }
        if self.max_embedding_concurrency == 0 {
            self.max_embedding_concurrency = 1;
        }
        if self.embedding_timeout.is_zero() {
            self.embedding_timeout = Duration::from_secs(1);
        }
        Ok(self)
    }

    /// Backward-compatible alias for callers that only need clamped limits.
    ///
    /// Falls back to defaults if the caller-provided limits are invalid.
    /// Default limits are infallible so the fallback path cannot fail.
    pub fn validated(self) -> Self {
        self.normalize_and_validate().unwrap_or_else(|err| {
            tracing::warn!(
                error = %err,
                "invalid MemoryLimits supplied to validated(); using defaults"
            );
            // Default limits are always valid — this path is infallible.
            let defaults = Self::default();
            Self {
                max_facts_per_namespace: defaults.max_facts_per_namespace,
                max_chunks_per_document: defaults.max_chunks_per_document,
                max_content_bytes: defaults.max_content_bytes,
                max_embedding_concurrency: defaults.max_embedding_concurrency.clamp(1, 32),
                max_db_size_bytes: defaults.max_db_size_bytes,
                embedding_timeout: if defaults.embedding_timeout.is_zero() {
                    std::time::Duration::from_secs(1)
                } else {
                    defaults.embedding_timeout
                },
            }
        })
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
