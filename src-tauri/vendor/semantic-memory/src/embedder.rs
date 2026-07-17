//! Embedding trait and implementations.
//!
//! Provides the [`Embedder`] trait for text-to-vector conversion,
//! with [`CandleEmbedder`] (default, in-process pure-Rust), [`OllamaEmbedder`]
//! (external Ollama server), and [`MockEmbedder`] (testing).

use crate::config::EmbeddingConfig;
use crate::error::MemoryError;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::pin::Pin;

/// Boxed future type alias for an embedder's optional multi-function output.
pub type OptionalMultiEmbedFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<MultiFunctionEmbedding>, MemoryError>> + Send + 'a>>;

/// Boxed future type alias for optional batched multi-function output.
pub type OptionalMultiEmbedBatchFuture<'a> = Pin<
    Box<dyn Future<Output = Result<Option<Vec<MultiFunctionEmbedding>>, MemoryError>> + Send + 'a>,
>;

/// Boxed future type alias for single embedding results.
pub type EmbedFuture<'a> = Pin<Box<dyn Future<Output = Result<Vec<f32>, MemoryError>> + Send + 'a>>;

/// Boxed future type alias for batch embedding results.
pub type EmbedBatchFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<Vec<f32>>, MemoryError>> + Send + 'a>>;

/// Trait for embedding text into vectors.
///
/// Implement this to swap embedding providers.
pub trait Embedder: Send + Sync {
    /// Embed a single text. Returns a vector of f32.
    fn embed<'a>(&'a self, text: &'a str) -> EmbedFuture<'a>;

    /// Embed multiple texts in a batch.
    ///
    /// Takes owned strings to avoid lifetime issues across async boundaries.
    fn embed_batch<'a>(&'a self, texts: Vec<String>) -> EmbedBatchFuture<'a>;

    /// The model name this embedder uses.
    fn model_name(&self) -> &str;

    /// Expected embedding dimensions.
    fn dimensions(&self) -> usize;

    /// Optionally produce dense and sparse representations in one model call.
    ///
    /// Existing dense-only embedders remain source-compatible and report no
    /// sparse capability. Callers may derive a generic sparse vector from the
    /// dense output only when explicitly configured to do so.
    fn embed_multi_optional<'a>(&'a self, _text: &'a str) -> OptionalMultiEmbedFuture<'a> {
        Box::pin(async { Ok(None) })
    }

    /// Optionally produce batched dense and sparse representations.
    fn embed_batch_multi_optional<'a>(
        &'a self,
        texts: Vec<String>,
    ) -> OptionalMultiEmbedBatchFuture<'a> {
        Box::pin(async move {
            let mut output = Vec::with_capacity(texts.len());
            for text in &texts {
                let Some(multi) = self.embed_multi_optional(text).await? else {
                    return Ok(None);
                };
                output.push(multi);
            }
            Ok(Some(output))
        })
    }
}

// ─── OllamaEmbedder ─────────────────────────────────────────────

/// Embedding provider that calls Ollama's `/api/embed` endpoint.
pub struct OllamaEmbedder {
    client: reqwest::Client,
    base_url: String,
    model: String,
    dimensions: usize,
    batch_size: usize,
}

impl OllamaEmbedder {
    /// Create a new OllamaEmbedder from config.
    ///
    /// Returns an error if the HTTP client cannot be constructed (e.g. TLS backend
    /// is unavailable).
    pub fn try_new(config: &EmbeddingConfig) -> Result<Self, MemoryError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|e| {
                MemoryError::EmbedderUnavailable(format!("failed to build HTTP client: {e}"))
            })?;

        Ok(Self {
            client,
            base_url: config.ollama_url.trim_end_matches('/').to_string(),
            model: config.model.clone(),
            dimensions: config.dimensions,
            batch_size: config.batch_size,
        })
    }

    // GOV-005: Deprecated `new()` method removed — all consumers should use `try_new()`.
}

impl Embedder for OllamaEmbedder {
    fn embed<'a>(&'a self, text: &'a str) -> EmbedFuture<'a> {
        Box::pin(async move {
            let mut results = self.embed_batch(vec![text.to_string()]).await?;
            results.pop().ok_or_else(|| {
                MemoryError::Other("Ollama returned empty embeddings for single text".to_string())
            })
        })
    }

    fn embed_batch<'a>(&'a self, texts: Vec<String>) -> EmbedBatchFuture<'a> {
        Box::pin(async move {
            let mut all_embeddings = Vec::with_capacity(texts.len());

            for batch in texts.chunks(self.batch_size) {
                let input: Vec<&str> = batch.iter().map(|s| s.as_str()).collect();
                let body = serde_json::json!({
                    "model": self.model,
                    "input": input
                });

                let url = format!("{}/api/embed", self.base_url);
                let response = self
                    .client
                    .post(&url)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| {
                        if e.is_connect() {
                            MemoryError::EmbedderUnavailable(format!(
                                "Ollama not running at {}",
                                self.base_url
                            ))
                        } else if e.is_timeout() {
                            MemoryError::EmbedderUnavailable(format!(
                                "Ollama embedding timed out: {}",
                                e
                            ))
                        } else {
                            MemoryError::EmbeddingRequest(e)
                        }
                    })?;

                if response.status() == reqwest::StatusCode::NOT_FOUND {
                    return Err(MemoryError::EmbedderUnavailable(format!(
                        "Model '{}' not available in Ollama. Run: ollama pull {}",
                        self.model, self.model
                    )));
                }

                if !response.status().is_success() {
                    let status = response.status();
                    let body = response
                        .text()
                        .await
                        .map_err(|err| format!("failed to read Ollama error body: {err}"));
                    return Err(format_ollama_http_error(status, body));
                }

                let resp_body: serde_json::Value = response.json().await?;
                let batch_embeddings = parse_embedding_response(&resp_body, self.dimensions)?;
                all_embeddings.extend(batch_embeddings);
            }

            Ok(all_embeddings)
        })
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }
}

#[doc(hidden)]
pub fn format_ollama_http_error(
    status: reqwest::StatusCode,
    body: Result<String, String>,
) -> MemoryError {
    match body {
        Ok(body) => {
            let preview: String = body.chars().take(500).collect();
            MemoryError::Other(format!("Ollama returned HTTP {}: {}", status, preview))
        }
        Err(err) => MemoryError::Other(format!("Ollama returned HTTP {status}; {err}")),
    }
}

/// Parse an Ollama embedding response body into vectors.
///
/// Validates that all values are numeric and dimensions match.
#[doc(hidden)]
pub fn parse_embedding_response(
    body: &serde_json::Value,
    expected_dims: usize,
) -> Result<Vec<Vec<f32>>, MemoryError> {
    let embeddings = body["embeddings"].as_array().ok_or_else(|| {
        MemoryError::Other("Ollama response missing 'embeddings' field".to_string())
    })?;

    let mut result = Vec::with_capacity(embeddings.len());
    for embedding_val in embeddings {
        let raw_array = embedding_val
            .as_array()
            .ok_or_else(|| MemoryError::Other("Embedding is not an array".to_string()))?;

        let mut embedding = Vec::with_capacity(raw_array.len());
        for (i, v) in raw_array.iter().enumerate() {
            let val = v.as_f64().ok_or_else(|| {
                MemoryError::Other(format!(
                    "Embedding dimension {} contains non-numeric value: {}",
                    i, v
                ))
            })?;
            embedding.push(val as f32);
        }

        if embedding.len() != expected_dims {
            return Err(MemoryError::DimensionMismatch {
                expected: expected_dims,
                actual: embedding.len(),
            });
        }

        result.push(embedding);
    }

    Ok(result)
}

// ─── MockEmbedder ────────────────────────────────────────────────

/// Deterministic embedder for unit tests.
///
/// Generates consistent embeddings based on a hash of the input text.
/// Same text always produces the same embedding. Output is normalized.
pub struct MockEmbedder {
    dimensions: usize,
}

impl MockEmbedder {
    /// Create a new MockEmbedder with the given dimensions.
    pub fn new(dimensions: usize) -> Self {
        Self { dimensions }
    }
}

impl Embedder for MockEmbedder {
    fn embed<'a>(&'a self, text: &'a str) -> EmbedFuture<'a> {
        let embedding = deterministic_embedding(mock_semantic_text(text), self.dimensions);
        Box::pin(async move { Ok(embedding) })
    }

    fn embed_batch<'a>(&'a self, texts: Vec<String>) -> EmbedBatchFuture<'a> {
        let embeddings: Vec<Vec<f32>> = texts
            .iter()
            .map(|t| deterministic_embedding(mock_semantic_text(t), self.dimensions))
            .collect();
        Box::pin(async move { Ok(embeddings) })
    }

    fn model_name(&self) -> &str {
        "mock-embedder"
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }
}

fn mock_semantic_text(text: &str) -> &str {
    text.strip_prefix("search_query: ")
        .or_else(|| text.strip_prefix("search_document: "))
        .unwrap_or(text)
}

/// Generate a deterministic embedding from text using a hash-seeded xorshift RNG.
fn deterministic_embedding(text: &str, dimensions: usize) -> Vec<f32> {
    let mut hasher = std::hash::DefaultHasher::new();
    text.hash(&mut hasher);
    let mut state = hasher.finish();
    if state == 0 {
        state = 1;
    }

    let mut values = Vec::with_capacity(dimensions);
    for _ in 0..dimensions {
        // xorshift64
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let val = ((state as f64) / (u64::MAX as f64)) * 2.0 - 1.0;
        values.push(val as f32);
    }

    // Normalize to unit length
    let magnitude: f32 = values.iter().map(|v| v * v).sum::<f32>().sqrt();
    if magnitude > 0.0 {
        for v in &mut values {
            *v /= magnitude;
        }
    }

    values
}

// ─── Multi-Function Embedding (BGE-M3) ───────────────────────────

/// Boxed future type alias for single multi-function embedding results.
pub type MultiEmbedFuture<'a> =
    Pin<Box<dyn Future<Output = Result<MultiFunctionEmbedding, MemoryError>> + Send + 'a>>;

/// Boxed future type alias for batch multi-function embedding results.
pub type MultiEmbedBatchFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<MultiFunctionEmbedding>, MemoryError>> + Send + 'a>>;

/// Sparse weight representation for generic or model-native sparse retrieval.
///
/// Stores non-zero (dimension_index, weight) pairs, sorted by descending
/// absolute weight. This type does not imply that its values came from SPLADE.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SparseWeights {
    /// (index, weight) pairs, sorted by descending absolute weight.
    pub entries: Vec<(usize, f32)>,
}

impl SparseWeights {
    /// Build a sparse representation from a dense vector by keeping the top-k
    /// dimensions with the largest absolute values, filtered by a minimum threshold.
    ///
    /// This is a pragmatic derivation from the dense embedding. When a native
    /// model-native sparse output is available, use
    /// [`SparseWeights::from_entries`] instead.
    #[must_use]
    pub fn from_dense(vec: &[f32], top_k: usize, min_weight: f32) -> Self {
        let mut entries: Vec<(usize, f32)> = vec
            .iter()
            .enumerate()
            .map(|(i, &v)| (i, v))
            .filter(|(_, v)| v.abs() >= min_weight)
            .collect();
        entries.sort_by(|a, b| {
            b.1.abs()
                .partial_cmp(&a.1.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        entries.truncate(top_k);
        Self { entries }
    }

    /// Build directly from pre-computed (index, weight) entries.
    #[must_use]
    pub fn from_entries(mut entries: Vec<(usize, f32)>) -> Self {
        entries.sort_by(|a, b| {
            b.1.abs()
                .partial_cmp(&a.1.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Self { entries }
    }

    /// Number of non-zero entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the sparse vector is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Dot product between two sparse vectors.
    ///
    /// Efficiently computed by intersecting the sorted-by-index entries.
    /// Since entries are sorted by weight (not index), we use a HashMap-based
    /// merge for correctness.
    pub fn dot(&self, other: &SparseWeights) -> f32 {
        use std::collections::HashMap;
        let map: HashMap<usize, f32> = other.entries.iter().copied().collect();
        self.entries
            .iter()
            .map(|(idx, w)| w * map.get(idx).copied().unwrap_or(0.0))
            .sum()
    }
}

/// ColBERT-style multi-vector representation (per-token embeddings).
///
/// Each inner vector is the embedding of a single token. Used for late
/// interaction scoring via MaxSim.
#[derive(Debug, Clone, PartialEq)]
pub struct MultiVectorEmbedding {
    /// Per-token embedding vectors.
    pub token_vectors: Vec<Vec<f32>>,
}

impl MultiVectorEmbedding {
    /// Build a multi-vector representation by chunking a dense vector into
    /// `num_tokens` roughly-equal sub-vectors.
    ///
    /// Each sub-vector acts as a pseudo per-token embedding. When native
    /// per-token embeddings are available from the model, use
    /// [`MultiVectorEmbedding::from_token_vectors`] instead.
    #[must_use]
    pub fn from_dense_chunked(vec: &[f32], num_tokens: usize) -> Self {
        if vec.is_empty() || num_tokens == 0 {
            return Self {
                token_vectors: Vec::new(),
            };
        }
        let chunk_size = (vec.len() + num_tokens - 1) / num_tokens; // ceil div
        let token_vectors = vec
            .chunks(chunk_size)
            .map(|chunk| {
                // Pad last chunk to full dimensionality for uniform length.
                let mut v = chunk.to_vec();
                v.resize(chunk_size, 0.0);
                v
            })
            .collect();
        Self { token_vectors }
    }

    /// Build directly from pre-computed per-token embedding vectors.
    #[must_use]
    pub fn from_token_vectors(token_vectors: Vec<Vec<f32>>) -> Self {
        Self { token_vectors }
    }

    /// Number of token vectors.
    pub fn len(&self) -> usize {
        self.token_vectors.len()
    }

    /// Whether the multi-vector is empty.
    pub fn is_empty(&self) -> bool {
        self.token_vectors.is_empty()
    }
}

/// Result of a single multi-function embedding call containing all three
/// representations produced from one model invocation.
#[derive(Debug, Clone)]
pub struct MultiFunctionEmbedding {
    /// Dense vector (standard embedding for dense retrieval).
    pub dense: Vec<f32>,
    /// Sparse weights for BGE-M3/native or explicitly derived generic retrieval.
    pub sparse: SparseWeights,
    /// ColBERT-style multi-vector (per-token embeddings for late interaction).
    pub multi_vec: MultiVectorEmbedding,
}

/// Trait for embedders that produce multiple representations from a single
/// model call: dense, sparse, and multi-vector.
///
/// This is the BGE-M3 style multi-function interface. Implementations produce
/// all three representations so that downstream retrieval can fuse them via
/// Reciprocal Rank Fusion (RRF) or other methods.
pub trait MultiFunctionEmbedder: Send + Sync {
    /// Embed a single text, returning dense, sparse, and multi-vec representations.
    fn embed_multi<'a>(&'a self, text: &'a str) -> MultiEmbedFuture<'a>;

    /// Embed multiple texts in a batch, returning all three representations per text.
    ///
    /// Takes owned strings to avoid lifetime issues across async boundaries.
    fn embed_batch_multi<'a>(&'a self, texts: Vec<String>) -> MultiEmbedBatchFuture<'a>;

    /// The model name this embedder uses.
    fn model_name(&self) -> &str;

    /// Expected dense embedding dimensions.
    fn dimensions(&self) -> usize;
}

/// Configuration for deriving sparse and multi-vec representations from dense
/// embeddings when the backend (Ollama) does not natively expose them.
#[derive(Debug, Clone)]
pub struct BgeM3DeriveConfig {
    /// Number of top dimensions to keep for sparse representation.
    pub sparse_top_k: usize,
    /// Minimum absolute weight threshold for sparse entries.
    pub sparse_min_weight: f32,
    /// Number of pseudo per-token vectors for multi-vec representation.
    pub num_multi_vec_tokens: usize,
}

impl Default for BgeM3DeriveConfig {
    fn default() -> Self {
        Self {
            sparse_top_k: 128,
            sparse_min_weight: 0.01,
            num_multi_vec_tokens: 32,
        }
    }
}

/// BGE-M3 multi-function embedder via Ollama.
///
/// Produces three representations from a single Ollama model call:
/// 1. **Dense vector** — direct from Ollama's `/api/embed` endpoint.
/// 2. **Sparse weights** — derived from the BGE-M3 dense vector via explicit
///    top-k thresholding because Ollama does not expose native sparse output.
/// 3. **ColBERT-style multi-vector** — dense vector chunked into pseudo
///    per-token embeddings for late interaction scoring.
///
/// # Current Limitations
///
/// Ollama's embedding API (`/api/embed`) currently returns only dense vectors.
/// The sparse and multi-vec representations are derived from the dense output.
/// They are not SPLADE outputs. When Ollama (or another backend) exposes native
/// BGE-M3 sparse and per-token outputs, this implementation can use those
/// representations directly without changing the retrieval interface.
///
/// The model `bge-m3` produces 1024-dimensional dense embeddings.
pub struct BgeM3Embedder {
    client: reqwest::Client,
    base_url: String,
    model: String,
    dimensions: usize,
    batch_size: usize,
    derive_config: BgeM3DeriveConfig,
}

impl BgeM3Embedder {
    /// Create a new BgeM3Embedder from embedding config.
    ///
    /// Uses `bge-m3` as the model name with 1024 dimensions by default.
    /// The Ollama URL and timeout are taken from the config.
    pub fn try_new(config: &EmbeddingConfig) -> Result<Self, MemoryError> {
        Self::try_new_with_derive(config, BgeM3DeriveConfig::default())
    }

    /// Create a new BgeM3Embedder with custom derivation parameters.
    pub fn try_new_with_derive(
        config: &EmbeddingConfig,
        derive_config: BgeM3DeriveConfig,
    ) -> Result<Self, MemoryError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|e| {
                MemoryError::EmbedderUnavailable(format!("failed to build HTTP client: {e}"))
            })?;

        Ok(Self {
            client,
            base_url: config.ollama_url.trim_end_matches('/').to_string(),
            model: config.model.clone(),
            dimensions: config.dimensions,
            batch_size: config.batch_size,
            derive_config,
        })
    }

    /// Create a BgeM3Embedder with explicit parameters (useful for testing
    /// without an `EmbeddingConfig`).
    pub fn with_params(
        base_url: &str,
        model: &str,
        dimensions: usize,
        batch_size: usize,
        timeout_secs: u64,
        derive_config: BgeM3DeriveConfig,
    ) -> Result<Self, MemoryError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| {
                MemoryError::EmbedderUnavailable(format!("failed to build HTTP client: {e}"))
            })?;

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            dimensions,
            batch_size,
            derive_config,
        })
    }

    /// Derive all three representations from a batch of dense embeddings.
    fn derive_multi_function(
        &self,
        dense_embeddings: Vec<Vec<f32>>,
    ) -> Vec<MultiFunctionEmbedding> {
        dense_embeddings
            .into_iter()
            .map(|dense| {
                let sparse = SparseWeights::from_dense(
                    &dense,
                    self.derive_config.sparse_top_k,
                    self.derive_config.sparse_min_weight,
                );
                let multi_vec = MultiVectorEmbedding::from_dense_chunked(
                    &dense,
                    self.derive_config.num_multi_vec_tokens,
                );
                MultiFunctionEmbedding {
                    dense,
                    sparse,
                    multi_vec,
                }
            })
            .collect()
    }

    /// Fetch raw dense embeddings from Ollama for a batch of texts.
    async fn fetch_dense(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, MemoryError> {
        let mut all_embeddings = Vec::with_capacity(texts.len());

        for batch in texts.chunks(self.batch_size) {
            let input: Vec<&str> = batch.iter().map(|s| s.as_str()).collect();
            let body = serde_json::json!({
                "model": self.model,
                "input": input
            });

            let url = format!("{}/api/embed", self.base_url);
            let response = self
                .client
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| {
                    if e.is_connect() {
                        MemoryError::EmbedderUnavailable(format!(
                            "Ollama not running at {}",
                            self.base_url
                        ))
                    } else if e.is_timeout() {
                        MemoryError::EmbedderUnavailable(format!(
                            "Ollama embedding timed out: {}",
                            e
                        ))
                    } else {
                        MemoryError::EmbeddingRequest(e)
                    }
                })?;

            if response.status() == reqwest::StatusCode::NOT_FOUND {
                return Err(MemoryError::EmbedderUnavailable(format!(
                    "Model '{}' not available in Ollama. Run: ollama pull {}",
                    self.model, self.model
                )));
            }

            if !response.status().is_success() {
                let status = response.status();
                let body = response
                    .text()
                    .await
                    .map_err(|err| format!("failed to read Ollama error body: {err}"));
                return Err(format_ollama_http_error(status, body));
            }

            let resp_body: serde_json::Value = response.json().await?;
            let batch_embeddings = parse_embedding_response(&resp_body, self.dimensions)?;
            all_embeddings.extend(batch_embeddings);
        }

        Ok(all_embeddings)
    }
}

impl MultiFunctionEmbedder for BgeM3Embedder {
    fn embed_multi<'a>(&'a self, text: &'a str) -> MultiEmbedFuture<'a> {
        Box::pin(async move {
            let mut results = self.fetch_dense(&[text.to_string()]).await?;
            let dense = results.pop().ok_or_else(|| {
                MemoryError::Other("Ollama returned empty embeddings for single text".to_string())
            })?;
            let sparse = SparseWeights::from_dense(
                &dense,
                self.derive_config.sparse_top_k,
                self.derive_config.sparse_min_weight,
            );
            let multi_vec = MultiVectorEmbedding::from_dense_chunked(
                &dense,
                self.derive_config.num_multi_vec_tokens,
            );
            Ok(MultiFunctionEmbedding {
                dense,
                sparse,
                multi_vec,
            })
        })
    }

    fn embed_batch_multi<'a>(&'a self, texts: Vec<String>) -> MultiEmbedBatchFuture<'a> {
        Box::pin(async move {
            let dense_embeddings = self.fetch_dense(&texts).await?;
            Ok(self.derive_multi_function(dense_embeddings))
        })
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }
}

/// BgeM3Embedder also implements the standard `Embedder` trait for compatibility,
/// returning only the dense representation.
impl Embedder for BgeM3Embedder {
    fn embed<'a>(&'a self, text: &'a str) -> EmbedFuture<'a> {
        Box::pin(async move {
            let mut results = self.fetch_dense(&[text.to_string()]).await?;
            results.pop().ok_or_else(|| {
                MemoryError::Other("Ollama returned empty embeddings for single text".to_string())
            })
        })
    }

    fn embed_batch<'a>(&'a self, texts: Vec<String>) -> EmbedBatchFuture<'a> {
        Box::pin(async move { self.fetch_dense(&texts).await })
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn embed_multi_optional<'a>(&'a self, text: &'a str) -> OptionalMultiEmbedFuture<'a> {
        Box::pin(async move {
            MultiFunctionEmbedder::embed_multi(self, text)
                .await
                .map(Some)
        })
    }

    fn embed_batch_multi_optional<'a>(
        &'a self,
        texts: Vec<String>,
    ) -> OptionalMultiEmbedBatchFuture<'a> {
        Box::pin(async move {
            MultiFunctionEmbedder::embed_batch_multi(self, texts)
                .await
                .map(Some)
        })
    }
}

// ─── CandleEmbedder ─────────────────────────────────────────────

/// In-process embedder using Candle (pure-Rust ML framework, CPU-only).
///
/// Downloads the model from HuggingFace Hub on first use (cached in
/// `~/.cache/huggingface/hub`). No external process or server required.
///
/// Default model: `nomic-ai/nomic-embed-text-v1.5` (768 dimensions).
/// The model matches the Ollama `nomic-embed-text` embedding, so an
/// existing HNSW index built with Ollama's nomic-embed-text is compatible.
#[cfg(feature = "candle-embedder")]
pub struct CandleEmbedder {
    model: candle_transformers::models::nomic_bert::NomicBertModel,
    tokenizer: tokenizers::Tokenizer,
    device: candle_core::Device,
    model_id: String,
    dimensions: usize,
    max_seq_len: usize,
}

#[cfg(feature = "candle-embedder")]
impl CandleEmbedder {
    /// Create a new CandleEmbedder with the default model (nomic-embed-text-v1.5, 768d).
    pub fn try_new(config: &EmbeddingConfig) -> Result<Self, MemoryError> {
        Self::try_new_with_model("nomic-ai/nomic-embed-text-v1.5", config)
    }

    /// Create a CandleEmbedder with a specific HuggingFace model ID.
    ///
    /// The model must be a NomicBert architecture (nomic-embed-text-v1.5).
    /// For other architectures (BERT, MiniLM), use a different model loader.
    pub fn try_new_with_model(
        model_id: &str,
        config: &EmbeddingConfig,
    ) -> Result<Self, MemoryError> {
        let device = candle_core::Device::Cpu;
        let dimensions = config.dimensions;
        let max_seq_len = 8192; // nomic-embed-text supports up to 8192 tokens

        // Download model files from HuggingFace Hub (cached after first download).
        // We download individual files rather than a snapshot to keep the API
        // simple and avoid pulling unnecessary files.
        let (owner, name) = match model_id.split_once('/') {
            Some((o, n)) => (o, n),
            None => ("nomic-ai", model_id),
        };

        let api = hf_hub::HFClientSync::new().map_err(|e| {
            MemoryError::EmbedderUnavailable(format!("failed to create HF Hub client: {e}"))
        })?;
        let repo = api.model(owner, name);

        // Download required files. These are cached by hf-hub after first download.
        let config_path = download_hf_file(&repo, "config.json")?;
        let tokenizer_path = download_hf_file(&repo, "tokenizer.json")?;

        // Try safetensors first, fall back to pytorch_model.bin.
        let weights_path = download_hf_file(&repo, "model.safetensors")
            .or_else(|_| download_hf_file(&repo, "pytorch_model.bin"))?;

        // Load tokenizer.
        let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path).map_err(|e| {
            MemoryError::EmbedderUnavailable(format!(
                "failed to load tokenizer from {}: {e}",
                tokenizer_path.display()
            ))
        })?;

        // Load model config.
        let config_str = std::fs::read_to_string(&config_path).map_err(|e| {
            MemoryError::EmbedderUnavailable(format!("failed to read config.json: {e}"))
        })?;
        let model_config: candle_transformers::models::nomic_bert::Config =
            serde_json::from_str(&config_str).map_err(|e| {
                MemoryError::EmbedderUnavailable(format!("failed to parse model config: {e}"))
            })?;

        // Verify dimensions match.
        if model_config.n_embd != dimensions {
            return Err(MemoryError::DimensionMismatch {
                expected: dimensions,
                actual: model_config.n_embd,
            });
        }

        // Load model weights via mmap.
        let dtype = candle_core::DType::F32;
        // Read the safetensors file into memory and load from buffer.
        // This avoids the unsafe mmap API (workspace lints deny unsafe_code).
        let weights_bytes = std::fs::read(&weights_path).map_err(|e| {
            MemoryError::EmbedderUnavailable(format!(
                "failed to read weights file {}: {e}",
                weights_path.display()
            ))
        })?;
        let vb = candle_nn::VarBuilder::from_buffered_safetensors(weights_bytes, dtype, &device)
            .map_err(|e| {
                MemoryError::EmbedderUnavailable(format!("failed to load model weights: {e}"))
            })?;

        let model =
            candle_transformers::models::nomic_bert::NomicBertModel::load(vb, &model_config)
                .map_err(|e| {
                    MemoryError::EmbedderUnavailable(format!(
                        "failed to build NomicBert model: {e}"
                    ))
                })?;

        Ok(Self {
            model,
            tokenizer,
            device,
            model_id: model_id.to_string(),
            dimensions,
            max_seq_len,
        })
    }

    /// Tokenize and embed a batch of texts, returning f32 vectors.
    fn embed_batch_sync(
        &self,
        texts: &[String],
        _query_mode: bool,
    ) -> Result<Vec<Vec<f32>>, MemoryError> {
        use candle_core::Tensor;
        use candle_transformers::models::nomic_bert::{l2_normalize, mean_pooling};

        let mut all_embeddings = Vec::with_capacity(texts.len());

        // NOTE: Prefix handling (search_query: / search_document:) is done at
        // the library level in embed_text_internal / embed_batch_internal.
        // The embedder receives already-prefixed text and should NOT add
        // its own prefix. The _query_mode parameter is kept for backward
        // compatibility but is ignored.

        // Process one at a time to keep memory bounded (CPU-only).
        for text in texts {
            let prefixed: &str = text;

            let encoding = self
                .tokenizer
                .encode(prefixed, true)
                .map_err(|e| MemoryError::Other(format!("tokenizer error: {e}")))?;

            let input_ids = encoding.get_ids();
            let attention_mask = encoding.get_attention_mask();

            // Truncate to max_seq_len.
            let seq_len = input_ids.len().min(self.max_seq_len);
            let input_ids = &input_ids[..seq_len];
            let attention_mask = &attention_mask[..seq_len];

            let input_ids_tensor = Tensor::new(input_ids, &self.device)?.unsqueeze(0)?; // (1, seq_len)
            let attention_mask_tensor = Tensor::new(attention_mask, &self.device)?.unsqueeze(0)?; // (1, seq_len)

            // Run forward pass — NomicBertModel doesn't need token_type_ids
            // (it uses rotary embeddings, and type_vocab_size may be 0).
            let token_type_ids = input_ids_tensor.zeros_like()?;
            let hidden_states = self.model.forward(
                &input_ids_tensor,
                Some(&token_type_ids),
                Some(&attention_mask_tensor),
            )?;

            // Mean-pool using attention mask, then L2-normalize.
            let pooled = mean_pooling(&hidden_states, &attention_mask_tensor)?;
            let normalized = l2_normalize(&pooled)?;

            // Extract the single embedding (batch size 1).
            let embedding_vec = normalized.to_vec2::<f32>()?;
            let embedding = embedding_vec
                .into_iter()
                .next()
                .ok_or_else(|| MemoryError::Other("model returned empty embedding".to_string()))?;

            if embedding.len() != self.dimensions {
                return Err(MemoryError::DimensionMismatch {
                    expected: self.dimensions,
                    actual: embedding.len(),
                });
            }

            all_embeddings.push(embedding);
        }

        Ok(all_embeddings)
    }
}

/// Download a single file from a HuggingFace repo, returning the local path.
/// The file is cached by hf-hub after the first download.
#[cfg(feature = "candle-embedder")]
fn download_hf_file(
    repo: &hf_hub::HFRepositorySync<hf_hub::repository::RepoTypeModel>,
    filename: &str,
) -> Result<std::path::PathBuf, MemoryError> {
    repo.download_file()
        .filename(filename.to_string())
        .send()
        .map_err(|e| {
            MemoryError::EmbedderUnavailable(format!("failed to download '{filename}': {e}"))
        })
}

#[cfg(feature = "candle-embedder")]
impl Embedder for CandleEmbedder {
    fn embed<'a>(&'a self, text: &'a str) -> EmbedFuture<'a> {
        let result = self.embed_batch_sync(&[text.to_string()], true);
        Box::pin(async move {
            let mut results = result?;
            results.pop().ok_or_else(|| {
                MemoryError::Other("Candle embedder returned empty results".to_string())
            })
        })
    }

    fn embed_batch<'a>(&'a self, texts: Vec<String>) -> EmbedBatchFuture<'a> {
        let result = self.embed_batch_sync(&texts, false);
        Box::pin(async move { result })
    }

    fn model_name(&self) -> &str {
        &self.model_id
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod bge_m3_tests {
    use super::*;

    // ── SparseWeights tests ──

    #[test]
    fn sparse_from_dense_keeps_top_k_by_abs_weight() {
        let dense = vec![0.5, -0.9, 0.01, 0.8, -0.3, 0.001];
        let sparse = SparseWeights::from_dense(&dense, 3, 0.05);
        assert_eq!(sparse.len(), 3);
        // Sorted by descending absolute weight: 0.9, 0.8, 0.5
        assert_eq!(sparse.entries[0], (1, -0.9));
        assert_eq!(sparse.entries[1], (3, 0.8));
        assert_eq!(sparse.entries[2], (0, 0.5));
    }

    #[test]
    fn sparse_from_dense_filters_below_threshold() {
        let dense = vec![0.5, 0.001, 0.9, 0.002];
        let sparse = SparseWeights::from_dense(&dense, 100, 0.05);
        assert_eq!(sparse.len(), 2);
        assert_eq!(sparse.entries[0], (2, 0.9));
        assert_eq!(sparse.entries[1], (0, 0.5));
    }

    #[test]
    fn sparse_from_dense_empty_input() {
        let sparse = SparseWeights::from_dense(&[], 10, 0.0);
        assert!(sparse.is_empty());
    }

    #[test]
    fn sparse_from_dense_truncates_to_top_k() {
        let dense = vec![1.0; 100];
        let sparse = SparseWeights::from_dense(&dense, 10, 0.0);
        assert_eq!(sparse.len(), 10);
    }

    #[test]
    fn sparse_dot_product() {
        let a = SparseWeights::from_entries(vec![(0, 1.0), (2, 2.0), (5, 3.0)]);
        let b = SparseWeights::from_entries(vec![(0, 0.5), (2, 1.0), (3, 10.0)]);
        // dot = 0.5 + 2.0 + 0 = 2.5
        let result = a.dot(&b);
        assert!((result - 2.5).abs() < 1e-6);
    }

    #[test]
    fn sparse_dot_product_no_overlap() {
        let a = SparseWeights::from_entries(vec![(0, 1.0), (1, 2.0)]);
        let b = SparseWeights::from_entries(vec![(2, 3.0), (3, 4.0)]);
        assert_eq!(a.dot(&b), 0.0);
    }

    #[test]
    fn sparse_dot_product_self() {
        let a = SparseWeights::from_entries(vec![(0, 3.0), (1, 4.0)]);
        // dot = 9 + 16 = 25
        assert!((a.dot(&a) - 25.0).abs() < 1e-6);
    }

    #[test]
    fn sparse_from_entries_sorts_by_abs_weight() {
        let entries = vec![(0, 0.1), (1, -0.5), (2, 0.3)];
        let sparse = SparseWeights::from_entries(entries);
        assert_eq!(sparse.entries[0], (1, -0.5));
        assert_eq!(sparse.entries[1], (2, 0.3));
        assert_eq!(sparse.entries[2], (0, 0.1));
    }

    // ── MultiVectorEmbedding tests ──

    #[test]
    fn multi_vec_from_dense_chunked() {
        let dense = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let mv = MultiVectorEmbedding::from_dense_chunked(&dense, 3);
        assert_eq!(mv.len(), 3);
        // 6 / 3 = 2 per chunk
        assert_eq!(mv.token_vectors[0], vec![1.0, 2.0]);
        assert_eq!(mv.token_vectors[1], vec![3.0, 4.0]);
        assert_eq!(mv.token_vectors[2], vec![5.0, 6.0]);
    }

    #[test]
    fn multi_vec_from_dense_chunked_with_padding() {
        let dense = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let mv = MultiVectorEmbedding::from_dense_chunked(&dense, 2);
        assert_eq!(mv.len(), 2);
        // chunk_size = ceil(5/2) = 3
        assert_eq!(mv.token_vectors[0], vec![1.0, 2.0, 3.0]);
        assert_eq!(mv.token_vectors[1], vec![4.0, 5.0, 0.0]); // padded
    }

    #[test]
    fn multi_vec_from_dense_chunked_empty() {
        let mv = MultiVectorEmbedding::from_dense_chunked(&[], 4);
        assert!(mv.is_empty());
    }

    #[test]
    fn multi_vec_from_dense_chunked_zero_tokens() {
        let mv = MultiVectorEmbedding::from_dense_chunked(&[1.0, 2.0], 0);
        assert!(mv.is_empty());
    }

    #[test]
    fn multi_vec_from_token_vectors() {
        let tokens = vec![vec![1.0, 0.0], vec![0.0, 1.0], vec![1.0, 1.0]];
        let mv = MultiVectorEmbedding::from_token_vectors(tokens.clone());
        assert_eq!(mv.len(), 3);
        assert_eq!(mv.token_vectors, tokens);
    }

    #[test]
    fn multi_vec_consistent_chunk_sizes() {
        let dense: Vec<f32> = (0..1024).map(|i| i as f32).collect();
        let mv = MultiVectorEmbedding::from_dense_chunked(&dense, 32);
        assert_eq!(mv.len(), 32);
        // All token vectors should have the same length
        let len0 = mv.token_vectors[0].len();
        for tv in &mv.token_vectors {
            assert_eq!(tv.len(), len0);
        }
    }

    // ── MultiFunctionEmbedding integration tests ──

    #[test]
    fn multi_function_embedding_holds_all_three() {
        let dense = vec![0.1, 0.5, 0.9, 0.3];
        let sparse = SparseWeights::from_dense(&dense, 2, 0.1);
        let multi_vec = MultiVectorEmbedding::from_dense_chunked(&dense, 2);
        let mfe = MultiFunctionEmbedding {
            dense: dense.clone(),
            sparse: sparse.clone(),
            multi_vec: multi_vec.clone(),
        };
        assert_eq!(mfe.dense, dense);
        assert_eq!(mfe.sparse, sparse);
        assert_eq!(mfe.multi_vec, multi_vec);
    }

    // ── BgeM3DeriveConfig tests ──

    #[test]
    fn derive_config_default_values() {
        let cfg = BgeM3DeriveConfig::default();
        assert_eq!(cfg.sparse_top_k, 128);
        assert_eq!(cfg.sparse_min_weight, 0.01);
        assert_eq!(cfg.num_multi_vec_tokens, 32);
    }

    // ── BgeM3Embedder construction tests ──

    #[test]
    fn bge_m3_embedder_with_params_constructs() {
        let embedder = BgeM3Embedder::with_params(
            "http://localhost:11434/",
            "bge-m3",
            1024,
            32,
            30,
            BgeM3DeriveConfig::default(),
        );
        assert!(embedder.is_ok());
        let embedder = embedder.unwrap();
        assert_eq!(Embedder::model_name(&embedder), "bge-m3");
        assert_eq!(Embedder::dimensions(&embedder), 1024);
    }

    #[test]
    fn bge_m3_embedder_try_new_from_config() {
        let config = EmbeddingConfig {
            ollama_url: "http://localhost:11434".to_string(),
            model: "bge-m3".to_string(),
            dimensions: 1024,
            batch_size: 16,
            timeout_secs: 60,
        };
        let embedder = BgeM3Embedder::try_new(&config);
        assert!(embedder.is_ok());
        let embedder = embedder.unwrap();
        assert_eq!(Embedder::model_name(&embedder), "bge-m3");
        assert_eq!(Embedder::dimensions(&embedder), 1024);
    }

    #[test]
    fn bge_m3_embedder_try_new_with_custom_derive() {
        let config = EmbeddingConfig {
            ollama_url: "http://localhost:11434".to_string(),
            model: "bge-m3".to_string(),
            dimensions: 1024,
            batch_size: 16,
            timeout_secs: 60,
        };
        let derive = BgeM3DeriveConfig {
            sparse_top_k: 64,
            sparse_min_weight: 0.05,
            num_multi_vec_tokens: 16,
        };
        let embedder = BgeM3Embedder::try_new_with_derive(&config, derive);
        assert!(embedder.is_ok());
    }

    // ── Derivation logic tests (no network needed) ──

    #[test]
    fn derive_multi_function_produces_correct_lengths() {
        let embedder = BgeM3Embedder::with_params(
            "http://localhost:11434",
            "bge-m3",
            1024,
            32,
            30,
            BgeM3DeriveConfig {
                sparse_top_k: 64,
                sparse_min_weight: 0.0,
                num_multi_vec_tokens: 16,
            },
        )
        .unwrap();

        let dense_vec: Vec<f32> = (0..1024).map(|i| (i as f32) / 1024.0).collect();
        let results = embedder.derive_multi_function(vec![dense_vec.clone()]);
        assert_eq!(results.len(), 1);

        let mfe = &results[0];
        assert_eq!(mfe.dense.len(), 1024);
        assert_eq!(mfe.sparse.len(), 64); // sparse_top_k
        assert_eq!(mfe.multi_vec.len(), 16); // num_multi_vec_tokens
    }

    #[test]
    fn derive_multi_function_handles_multiple_inputs() {
        let embedder = BgeM3Embedder::with_params(
            "http://localhost:11434",
            "bge-m3",
            8,
            4,
            30,
            BgeM3DeriveConfig::default(),
        )
        .unwrap();

        let inputs: Vec<Vec<f32>> = (0..5).map(|i| vec![i as f32; 8]).collect();
        let results = embedder.derive_multi_function(inputs);
        assert_eq!(results.len(), 5);
        for mfe in &results {
            assert_eq!(mfe.dense.len(), 8);
        }
    }

    #[test]
    fn derive_multi_function_empty_input() {
        let embedder = BgeM3Embedder::with_params(
            "http://localhost:11434",
            "bge-m3",
            8,
            4,
            30,
            BgeM3DeriveConfig::default(),
        )
        .unwrap();

        let results = embedder.derive_multi_function(vec![]);
        assert!(results.is_empty());
    }

    // ── Ollama integration tests (require running Ollama with bge-m3) ──

    #[tokio::test]
    #[ignore = "requires Ollama running with bge-m3 model pulled"]
    async fn bge_m3_embed_multi_live() {
        let embedder = BgeM3Embedder::with_params(
            "http://127.0.0.1:11434",
            "bge-m3",
            1024,
            32,
            60,
            BgeM3DeriveConfig::default(),
        )
        .unwrap();

        let result = embedder.embed_multi("hello world").await;
        assert!(result.is_ok(), "Ollama call failed: {:?}", result.err());
        let mfe = result.unwrap();
        assert_eq!(mfe.dense.len(), 1024);
        assert!(!mfe.sparse.is_empty());
        assert!(!mfe.multi_vec.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires Ollama running with bge-m3 model pulled"]
    async fn bge_m3_embed_batch_multi_live() {
        let embedder = BgeM3Embedder::with_params(
            "http://127.0.0.1:11434",
            "bge-m3",
            1024,
            32,
            60,
            BgeM3DeriveConfig::default(),
        )
        .unwrap();

        let texts = vec!["hello".to_string(), "world".to_string(), "test".to_string()];
        let results = embedder.embed_batch_multi(texts).await;
        assert!(
            results.is_ok(),
            "Ollama batch call failed: {:?}",
            results.err()
        );
        let embeddings = results.unwrap();
        assert_eq!(embeddings.len(), 3);
        for mfe in &embeddings {
            assert_eq!(mfe.dense.len(), 1024);
        }
    }

    #[tokio::test]
    #[ignore = "requires Ollama running with bge-m3 model pulled"]
    async fn bge_m3_embedder_as_standard_embedder_live() {
        let embedder = BgeM3Embedder::with_params(
            "http://127.0.0.1:11434",
            "bge-m3",
            1024,
            32,
            60,
            BgeM3DeriveConfig::default(),
        )
        .unwrap();

        let result = embedder.embed("hello world").await;
        assert!(result.is_ok());
        let dense = result.unwrap();
        assert_eq!(dense.len(), 1024);
    }
}
