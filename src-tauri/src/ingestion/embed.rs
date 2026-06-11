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
        let index = Self::new(dims)?;
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
        Ok(results.keys.into_iter().zip(results.distances).collect())
    }

    pub fn save(&self, path: &Path) -> Result<(), GlossError> {
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new("")))?;
        self.index
            .save(path.to_str().unwrap_or(""))
            .map_err(|e| GlossError::Embedding(format!("HNSW save failed: {e}")))
    }

    #[allow(dead_code)]
    pub fn size(&self) -> usize {
        self.index.size()
    }
}

/// Embedding backend: FastEmbed (in-process ONNX, CPU-only, may crash) or
/// Ollama (separate process via HTTP, crash-isolated).
pub enum EmbeddingBackend {
    FastEmbed(Box<TextEmbedding>),
    Ollama {
        client: reqwest::Client,
        url: String,
        model: String,
    },
}

/// Wrapper around an embedding backend. All heavy inference happens outside
/// Gloss when the Ollama backend is active.
pub struct EmbeddingService {
    backend: EmbeddingBackend,
    reranker: Option<TextRerank>,
    #[allow(dead_code)]
    dims: usize,
}

impl EmbeddingService {
    /// FastEmbed path — kept for backward compatibility / offline fallback.
    #[allow(dead_code)]
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

        // Non-fatal: fall back to RRF-only fusion if the reranker fails to load.
        let reranker = match TextRerank::try_new(
            RerankInitOptions::new(RerankerModel::BGERerankerBase)
                .with_cache_dir(cache_dir.into())
                .with_show_download_progress(true),
        ) {
            Ok(r) => {
                tracing::info!("Reranker (BGERerankerBase) loaded");
                Some(r)
            }
            Err(e) => {
                tracing::warn!("Reranker failed to load (falling back to RRF-only): {e}");
                None
            }
        };

        Ok(Self {
            backend: EmbeddingBackend::FastEmbed(Box::new(model)),
            reranker,
            dims: 384,
        })
    }

    /// Ollama path — crash-isolated, preferred.
    ///
    /// `timeout_secs` distinguishes the chat hot path (~8s — embed must return
    /// fast or we degrade to BM25) from the ingestion batch path (60s — a cold
    /// all-minilm model load can take 10-15s on the first call). The user
    /// sees a toast on chat-path timeout, not a 60-second frozen UI.
    ///
    /// Sync, but the underlying `reqwest::Client` is the async flavor rather
    /// than the blocking one. The previous code used `reqwest::blocking::Client`,
    /// whose `ClientBuilder::build()` lazily spins up an internal blocking-pool
    /// runtime. Constructing that from inside a `tauri::async_runtime::spawn`
    /// task panicked at `tokio::runtime::blocking::shutdown` ("Cannot drop a
    /// runtime in a context where blocking is not allowed"). The async
    /// `reqwest::Client` has no such blocking pool, so this constructor is
    /// safe to call from any context.
    pub fn new_ollama(url: &str, model: &str, timeout_secs: u64) -> Result<Self, GlossError> {
        let timeout = std::time::Duration::from_secs(timeout_secs.clamp(2, 300));
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|e| GlossError::Embedding(format!("HTTP client build failed: {e}")))?;

        // The embedding dim is model-specific (bge-m3=1024, all-minilm=384,
        // nomic-embed=768, etc.). Hardcoding 384 here caused the HNSW index to
        // be created with the wrong dim when the user picked a non-384 model,
        // which made every subsequent retrieval corrupt the C++ heap or
        // silently return no results. We probe ollama for the actual dim
        // before constructing the service so the HNSW matches.
        //
        // We try two paths in order:
        //   1. POST /api/embed with a single token and read the response
        //      vector's length. This is authoritative.
        //   2. POST /api/show and look for `bert.embedding_length` in
        //      `model_info`. Less reliable (older ollama builds, custom
        //      models), but a useful fallback.
        // If both fail, we default to 384 and log a warning. The first
        // retrieval that produces a vector of a different length will trip a
        // dim-mismatch panic from usearch, which is loud and fixable — much
        // better than silently searching a 384-dim HNSW with a 1024-dim vector.
        let dims = probe_ollama_dims(&client, url, model).unwrap_or_else(|| {
            tracing::warn!(
                model = %model,
                url = %url,
                "Could not probe embedding dim from ollama; defaulting to 384. \
                 HNSW dim may not match the model's actual dim. \
                 First embed call will surface the mismatch as a usearch error."
            );
            384
        });

        Ok(Self {
            backend: EmbeddingBackend::Ollama {
                client,
                url: url.trim_end_matches('/').to_string(),
                model: model.to_string(),
            },
            reranker: None,
            dims,
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
                // The async `reqwest::Client` requires a tokio runtime to
                // drive `.send()`. The previous code used `reqwest::blocking
                // ::Client`, whose `ClientBuilder::build()` lazily spins up an
                // internal blocking-pool runtime; that panicked at
                // `tokio::runtime::blocking::shutdown` when the constructor
                // was called from inside a `tauri::async_runtime::spawn` task.
                // We bridge to the async client from this sync method by
                // either borrowing the existing tokio runtime (if we are on a
                // runtime thread) or running a fresh, single-threaded runtime
                // for the duration of the call. This keeps the public API sync
                // while avoiding the blocking-pool-panic root cause.
                let post = client
                    .post(format!("{}/api/embed", url))
                    .json(&body);
                let send_fut = post.send();
                let response = match tokio::runtime::Handle::try_current() {
                    Ok(handle) => tokio::task::block_in_place(|| handle.block_on(send_fut)),
                    Err(_) => {
                        let runtime = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .map_err(|e| {
                                GlossError::Embedding(format!("runtime build failed: {e}"))
                            })?;
                        runtime.block_on(send_fut)
                    }
                }
                .map_err(|e| GlossError::Embedding(format!("Ollama embed request failed: {e}")))?;

                if !response.status().is_success() {
                    let status = response.status();
                    let text_fut = response.text();
                    let body_text = match tokio::runtime::Handle::try_current() {
                        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(text_fut)),
                        Err(_) => {
                            let runtime = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                                .map_err(|e| {
                                    GlossError::Embedding(format!("runtime build failed: {e}"))
                                })?;
                            runtime.block_on(text_fut)
                        }
                    }
                    .unwrap_or_else(|_| "<could not read body>".to_string());
                    return Err(GlossError::Embedding(format!(
                        "Ollama embed returned HTTP {}: {}",
                        status, body_text
                    )));
                }

                let json_fut = response.json();
                let parsed: serde_json::Value = match tokio::runtime::Handle::try_current() {
                    Ok(handle) => tokio::task::block_in_place(|| handle.block_on(json_fut)),
                    Err(_) => {
                        let runtime = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .map_err(|e| {
                                GlossError::Embedding(format!("runtime build failed: {e}"))
                            })?;
                        runtime.block_on(json_fut)
                    }
                }
                .map_err(|e| {
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

    #[allow(dead_code)]
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

/// Probe ollama for the embedding dim of the given model. Returns `None` if
/// the probe fails for any reason (network, parse, model not found). Caller
/// decides the fallback.
///
/// This is used during `EmbeddingService::new_ollama` so the HNSW index dim
/// matches the model. The probe runs a single-token embed through the same
/// async bridge as a normal embed call.
fn probe_ollama_dims(client: &reqwest::Client, url: &str, model: &str) -> Option<usize> {
    let url = url.trim_end_matches('/');
    let body = serde_json::json!({ "model": model, "input": "dim-probe" });
    let send_fut = client
        .post(format!("{}/api/embed", url))
        .json(&body)
        .send();
    let response = match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(send_fut)),
        Err(_) => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .ok()?;
            runtime.block_on(send_fut)
        }
    }
    .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let json_fut = response.json::<serde_json::Value>();
    let parsed = match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(json_fut)),
        Err(_) => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .ok()?;
            runtime.block_on(json_fut)
        }
    }
    .ok()?;
    parsed
        .get("embeddings")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_array())
        .map(|a| a.len())
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
