use crate::error::GlossError;
use crate::redaction::redact_path;
use fastembed::{
    EmbeddingModel, InitOptions, RerankInitOptions, RerankerModel, TextEmbedding, TextRerank,
};
use reqwest;
use std::path::Path;
use usearch::ffi::{IndexOptions, MetricKind, ScalarKind};

pub fn fastembed_cache_has_entries(cache_dir: &Path) -> bool {
    cache_dir
        .read_dir()
        .map(|mut entries| entries.any(|entry| entry.is_ok()))
        .unwrap_or(false)
}

pub fn require_fastembed_download_consent(
    cache_dir: &Path,
    download_consent: bool,
) -> Result<(), GlossError> {
    if fastembed_cache_has_entries(cache_dir) || download_consent {
        return Ok(());
    }
    let redacted = redact_path(cache_dir);
    Err(GlossError::Embedding(format!(
        "FastEmbed model cache is empty at {}; enable FastEmbed download consent or switch semantic-memory embeddings to Ollama before initializing local embeddings",
        redacted
    )))
}

/// HNSW vector index using usearch (C++ via FFI, but only for add/search/save —
/// no model inference here, so heap corruption from ONNX batch embed is isolated).
pub struct HnswIndex {
    index: usearch::Index,
}

impl HnswIndex {
    pub fn new(dims: usize) -> Result<Self, GlossError> {
        let options = IndexOptions {
            metric: MetricKind::IP,
            connectivity: 16,
            dimensions: dims,
            quantization: ScalarKind::F32,
            ..Default::default()
        };
        let index = usearch::Index::new(&options)
            .map_err(|e| GlossError::Embedding(format!("Failed to create HNSW index: {e}")))?;
        Ok(Self { index })
    }

    pub fn load_with_hwm(path: &Path, hwm: i64, dims: usize) -> Result<Self, GlossError> {
        let mut index = Self::new(dims)?;
        if path.exists() {
            index
                .index
                .load(path.to_str().unwrap_or(""))
                .map_err(|e| GlossError::Embedding(format!("Failed to load HNSW index: {e}")))?;
            // Reserve labels up to hwm to avoid collision on next add()
            let _ = index.index.reserve(hwm as usize);
        }
        Ok(index)
    }

    pub fn add(&mut self, vector: &[f32]) -> Result<u64, GlossError> {
        let label = self.index.size() as u64;
        self.index
            .add(label, vector)
            .map_err(|e| GlossError::Embedding(format!("HNSW add failed: {e}")))?;
        Ok(label)
    }

    pub fn remove(&mut self, label: u64) -> Result<bool, GlossError> {
        let removed = self
            .index
            .remove(label)
            .map_err(|e| GlossError::Embedding(format!("HNSW remove failed: {e}")))?;
        Ok(removed > 0)
    }

    pub fn search(&self, query: &[f32], count: usize) -> Result<Vec<(u64, f32)>, GlossError> {
        let results = self
            .index
            .search(query, count)
            .map_err(|e| GlossError::Embedding(format!("HNSW search failed: {e}")))?;
        Ok(results
            .keys
            .into_iter()
            .zip(results.distances.into_iter())
            .collect())
    }

    pub fn save(&self, path: &Path) -> Result<(), GlossError> {
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new("")))?;
        self.index
            .save(path.to_str().unwrap_or(""))
            .map_err(|e| GlossError::Embedding(format!("HNSW save failed: {e}")))
    }

    pub fn size(&self) -> usize {
        self.index.size()
    }
}

/// Embedding backend: FastEmbed (in-process ONNX, CPU-only, may crash) or
/// Ollama (separate process via HTTP, crash-isolated).
pub enum EmbeddingBackend {
    FastEmbed(TextEmbedding),
    Ollama {
        client: reqwest::blocking::Client,
        url: String,
        model: String,
    },
}

/// Wrapper around an embedding backend. All heavy inference happens outside
/// Gloss when the Ollama backend is active.
pub struct EmbeddingService {
    backend: EmbeddingBackend,
    #[allow(dead_code)]
    reranker: Option<TextRerank>,
    dims: usize,
}

impl EmbeddingService {
    /// FastEmbed path — kept for backward compatibility / offline fallback.
    pub fn new(cache_dir: &Path) -> Result<Self, GlossError> {
        Self::new_with_download_policy(cache_dir, false, true)
    }

    pub fn new_with_download_policy(
        cache_dir: &Path,
        _use_gpu: bool,
        download_consent: bool,
    ) -> Result<Self, GlossError> {
        require_fastembed_download_consent(cache_dir, download_consent)?;

        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::BGESmallENV15)
                .with_cache_dir(cache_dir.into())
                .with_show_download_progress(true),
        )
        .map_err(|e| GlossError::Embedding(format!("FastEmbed init failed: {e}")))?;

        Ok(Self {
            backend: EmbeddingBackend::FastEmbed(model),
            reranker: None,
            dims: 384,
        })
    }

    /// Ollama path — crash-isolated, preferred.
    pub fn new_ollama(url: &str, model: &str) -> Result<Self, GlossError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| GlossError::Embedding(format!("HTTP client build failed: {e}")))?;

        Ok(Self {
            backend: EmbeddingBackend::Ollama {
                client,
                url: url.trim_end_matches('/').to_string(),
                model: model.to_string(),
            },
            reranker: None,
            dims: 384,
        })
    }

    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, GlossError> {
        match &self.backend {
            EmbeddingBackend::FastEmbed(model) => {
                let embeddings = model
                    .embed(texts.to_vec(), None)
                    .map_err(|e| GlossError::Embedding(format!("FastEmbed embed failed: {e}")))?;
                Ok(embeddings)
            }
            EmbeddingBackend::Ollama { client, url, model } => {
                let body = serde_json::json!({
                    "model": model,
                    "input": texts,
                });
                let response = client
                    .post(format!("{}/api/embed", url))
                    .json(&body)
                    .send()
                    .map_err(|e| {
                        GlossError::Embedding(format!("Ollama embed request failed: {e}"))
                    })?;

                if !response.status().is_success() {
                    let status = response.status();
                    let body_text = response
                        .text()
                        .unwrap_or_else(|_| "<could not read body>".to_string());
                    return Err(GlossError::Embedding(format!(
                        "Ollama embed returned HTTP {}: {}",
                        status, body_text
                    )));
                }

                let parsed: serde_json::Value = response.json().map_err(|e| {
                    GlossError::Embedding(format!("Ollama embed JSON parse failed: {e}"))
                })?;

                let embeddings_array = parsed
                    .get("embeddings")
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| {
                        GlossError::Embedding(
                            "Ollama embed response missing 'embeddings' array".into(),
                        )
                    })?;

                let mut result = Vec::with_capacity(embeddings_array.len());
                for emb_val in embeddings_array {
                    let emb_vec = emb_val
                        .as_array()
                        .ok_or_else(|| {
                            GlossError::Embedding(
                                "Ollama embed: non-array element in embeddings".into(),
                            )
                        })?
                        .iter()
                        .map(|v| {
                            v.as_f64()
                                .ok_or_else(|| {
                                    GlossError::Embedding(
                                        "Ollama embed: non-numeric value in embedding".into(),
                                    )
                                })
                                .map(|f| f as f32)
                        })
                        .collect::<Result<Vec<f32>, _>>()?;
                    result.push(emb_vec);
                }
                Ok(result)
            }
        }
    }

    pub fn dims(&self) -> usize {
        self.dims
    }

    /// Convenience: embed a single text by wrapping it in a batch of 1.
    pub fn embed_one(&self, text: &str) -> Result<Vec<f32>, GlossError> {
        let mut batch = self.embed_batch(&[text])?;
        batch
            .pop()
            .ok_or_else(|| GlossError::Embedding("embed_one returned empty batch".into()))
    }

    /// Cross-encoder reranker — not implemented for Ollama backend yet.
    pub fn has_reranker(&self) -> bool {
        // FastEmbed path could have a reranker; Ollama path does not.
        matches!(&self.backend,
            EmbeddingBackend::FastEmbed(_) if self.reranker.is_some()
        )
    }

    /// Rerank documents against a query using the cross-encoder.
    pub fn rerank(
        &self,
        _query: &str,
        documents: &[String],
        top_k: usize,
    ) -> Result<Vec<(usize, f32)>, GlossError> {
        match &self.backend {
            EmbeddingBackend::FastEmbed(_) => {
                if let Some(ref reranker) = self.reranker {
                    let results = reranker
                        .rerank(_query.to_string(), documents.to_vec(), false, Some(top_k))
                        .map_err(|e| GlossError::Embedding(format!("Rerank failed: {e}")))?;
                    Ok(results.into_iter().map(|r| (r.index, r.score)).collect())
                } else {
                    Err(GlossError::Embedding("No reranker available".into()))
                }
            }
            EmbeddingBackend::Ollama { .. } => Err(GlossError::Embedding(
                "Ollama backend does not support cross-encoder reranking".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "usearch FFI SIGSEGV in HnswIndex teardown (pre-existing); re-enable when vendored usearch is updated"]
    fn hnsw_index_create_and_search() {
        let mut index = HnswIndex::new(3).unwrap();
        let v1 = vec![1.0f32, 0.0, 0.0];
        let v2 = vec![0.0f32, 1.0, 0.0];
        let l1 = index.add(&v1).unwrap();
        let l2 = index.add(&v2).unwrap();
        assert_eq!(l1, 0);
        assert_eq!(l2, 1);

        let results = index.search(&v1, 2).unwrap();
        assert_eq!(results[0].0, 0);
    }

    #[test]
    fn ollama_embed_batch_parsing() {
        // Mock JSON response
        let json = serde_json::json!({
            "model": "all-minilm",
            "embeddings": [
                [0.1, 0.2, 0.3],
                [0.4, 0.5, 0.6]
            ]
        });
        let arr = json.get("embeddings").unwrap().as_array().unwrap();
        let parsed: Vec<Vec<f32>> = arr
            .iter()
            .map(|v| {
                v.as_array()
                    .unwrap()
                    .iter()
                    .map(|n| n.as_f64().unwrap() as f32)
                    .collect()
            })
            .collect();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], vec![0.1f32, 0.2, 0.3]);
    }

    #[test]
    fn empty_fastembed_cache_requires_explicit_download_consent() {
        // Create a unique empty temp dir — fastembed_cache_has_entries() returns false.
        let tmp = std::env::temp_dir().join(format!(
            "gloss_empty_cache_test_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let cache = tmp.as_path();

        // Sanity: empty cache dir reports no entries.
        assert!(!fastembed_cache_has_entries(cache));

        // Without explicit download consent, the helper must reject.
        let err = require_fastembed_download_consent(cache, false)
            .expect_err("empty cache without download consent must error");
        let msg = format!("{err}");
        assert!(
            msg.contains("FastEmbed model cache is empty"),
            "error message should explain the empty-cache block; got: {msg}"
        );

        // With explicit download consent, the helper must allow.
        require_fastembed_download_consent(cache, true)
            .expect("explicit download consent must permit empty cache");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
