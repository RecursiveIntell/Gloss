//! Embedding trait and implementations.
//!
//! Provides the [`Embedder`] trait for text-to-vector conversion,
//! with [`OllamaEmbedder`] (production) and [`MockEmbedder`] (testing).

use crate::config::EmbeddingConfig;
use crate::error::MemoryError;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::pin::Pin;

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
        Ok(body) => MemoryError::Other(format!(
            "Ollama returned HTTP {}: {}",
            status,
            &body[..body.len().min(500)]
        )),
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
        let embedding = deterministic_embedding(text, self.dimensions);
        Box::pin(async move { Ok(embedding) })
    }

    fn embed_batch<'a>(&'a self, texts: Vec<String>) -> EmbedBatchFuture<'a> {
        let embeddings: Vec<Vec<f32>> = texts
            .iter()
            .map(|t| deterministic_embedding(t, self.dimensions))
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
